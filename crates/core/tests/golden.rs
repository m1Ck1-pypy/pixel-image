//! Эталонные отпечатки конвейера.
//!
//! Фикстуры строятся в коде (детерминированно, без внешних файлов и без `rand`),
//! поэтому тест не зависит от содержимого `out/` или `test.jpg`. Сравниваем
//! отпечаток пикселей, а не байты PNG: смена компрессии на этапе 6 не должна
//! ломать эти тесты.
//!
//! Если отпечаток изменился осознанно (например, этап 4 меняет реализацию
//! ресэмплера и это разрешено планом), пересчитайте константы:
//!
//! ```text
//! GOLDEN_PRINT=1 cargo test -p pixel-image-core --test golden -- --nocapture
//! ```
//!
//! Тест печатает актуальные значения рядом с именами констант.

use image::{Rgba, RgbaImage};
use pixel_image_core::{
    BackgroundOptions, Filter, Options, convert_image, cutout_image, render_grid,
};

/// Детерминированная фикстура: градиент, «объект» с мягкой каймой, прозрачный
/// угол и немного шума внутри объекта.
fn fixture(w: u32, h: u32, seed: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };

    let cx = w as f32 * 0.5;
    let cy = h as f32 * 0.5;
    let radius = w.min(h) as f32 * 0.35;

    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            // Фон: светлый, почти однотонный (чтобы работало вырезание фона).
            let (mut r, mut g, mut b, mut a) = (242u8, 238u8, 226u8, 255u8);

            if dist < radius {
                // «Объект»: насыщенный цвет плюс шум, чтобы гистограмма и
                // палитра были нетривиальными.
                let noise = next() % 48;
                r = (40 + noise) as u8;
                g = (70 + next() % 40) as u8;
                b = (120 + next() % 40) as u8;
                let _ = noise;
            } else if dist < radius + 3.0 {
                // Мягкая кайма: именно она проверяет premultiplied alpha.
                a = ((((radius + 3.0 - dist) / 3.0) * 255.0) as u32).min(255) as u8;
            }

            // Прозрачный угол: проверяет «пропуск прозрачных ячеек» и RGBA-ветку.
            if x < (w / 8).max(1) && y < (h / 8).max(1) {
                a = 0;
            }

            img.put_pixel(x, y, Rgba([r, g, b, a]));
        }
    }
    img
}

/// FNV-1a по размерам и байтам изображения.
fn fingerprint(img: &RgbaImage) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |byte: u8| {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for byte in img.width().to_le_bytes() {
        mix(byte);
    }
    for byte in img.height().to_le_bytes() {
        mix(byte);
    }
    for &byte in img.as_raw() {
        mix(byte);
    }
    hash
}

fn report(name: &str, value: u64, expected: u64) {
    if std::env::var_os("GOLDEN_PRINT").is_some() {
        println!("{name} = 0x{value:016x};  // ожидалось 0x{expected:016x}");
    }
    assert_eq!(
        value, expected,
        "{name}: отпечаток изменился.\n\
         Если изменение осознанное, обновите константу (GOLDEN_PRINT=1 cargo test ... --nocapture)."
    );
}

fn options_small(width: u32, cell: u32) -> Options {
    Options {
        width,
        cell,
        colors: 16,
        ..Options::default()
    }
}

#[test]
fn golden_sprite_box() {
    let src = fixture(64, 48, 1);
    let options = options_small(24, 8);
    let conversion = convert_image(&src, &options).expect("преобразование");

    assert_eq!(conversion.sprite.dimensions(), (24, 18));
    report("SPRITE_BOX", fingerprint(&conversion.sprite), SPRITE_BOX);
}

/// Сетка строится отдельно от превью, но обязана совпадать с прежним
/// `Conversion::grid`: эталон снят ещё до разделения.
#[test]
fn golden_grid_box() {
    let src = fixture(64, 48, 1);
    let options = options_small(24, 8);
    let conversion = convert_image(&src, &options).expect("преобразование");
    let grid = render_grid(&conversion.sprite, &options).expect("сетка");

    assert_eq!(grid.dimensions(), (24 * 8, 18 * 8));
    report("GRID_BOX", fingerprint(&grid), GRID_BOX);
}

#[test]
fn golden_sprite_lanczos() {
    let src = fixture(96, 64, 2);
    let options = Options {
        width: 32,
        cell: 4,
        colors: 24,
        filter: Filter::Lanczos,
        ..Options::default()
    };
    let conversion = convert_image(&src, &options).expect("преобразование");

    report("SPRITE_LANCZOS", fingerprint(&conversion.sprite), SPRITE_LANCZOS);
}

#[test]
fn golden_stats() {
    let src = fixture(64, 48, 1);
    let options = options_small(24, 8);
    let conversion = convert_image(&src, &options).expect("преобразование");
    let stats = &conversion.stats;

    assert_eq!(stats.grid_width, 24);
    assert_eq!(stats.grid_height, 18);
    assert_eq!(stats.output_width, 192);
    assert_eq!(stats.output_height, 144);
    assert_eq!(stats.total_cells, 24 * 18);
    // Значения фиксируем как числа: они меняются только вместе с алгоритмом.
    assert_eq!(stats.palette_colors, 16);
}

#[test]
fn golden_cutout() {
    let src = fixture(64, 48, 3);
    let background = BackgroundOptions::default();
    let cutout = cutout_image(src, &background).expect("вырезание фона");

    assert_eq!(cutout.image.dimensions(), (64, 48));
    assert!(!cutout.fully_transparent, "объект обязан остаться");
    assert!(
        cutout.stats.background.is_some(),
        "статистика вырезания обязана заполниться"
    );
    report("CUTOUT", fingerprint(&cutout.image), CUTOUT);
}

// --- Эталоны ---------------------------------------------------------------
// Пересчёт: GOLDEN_PRINT=1 cargo test -p pixel-image-core --test golden -- --nocapture
const SPRITE_BOX: u64 = 0x78e72d6c0efd12e6;
const GRID_BOX: u64 = 0x8d9823cf0826601f;
const SPRITE_LANCZOS: u64 = 0xedfd12d60bef2bf5;
const CUTOUT: u64 = 0xafb05295317d801c;
