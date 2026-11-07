//! Паритет последовательного и параллельного путей (инвариант О-2).
//!
//! Один и тот же вызов прогоняется трижды: на пуле из одного потока (внутри —
//! последовательный путь), на пуле из четырёх (параллельный) и на общем пуле
//! процесса. Результаты обязаны совпасть байт в байт; расхождение означает
//! недетерминированный участок — например, аккумулятор с плавающей точкой,
//! сложенный в порядке планировщика.
//!
//! Порог `parallel::MIN_PIXELS` держится низким для тестов: фикстуры здесь
//! крупные, чтобы параллельная ветка действительно включалась.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use image::{Rgba, RgbaImage};
use pixel_image_core::clean::cut_faint_alpha;
use pixel_image_core::downscale::apply_alpha_threshold;
use pixel_image_core::grid::upscale_with_grid;
use pixel_image_core::palette::histogram;
use pixel_image_core::{BackgroundOptions, Filter, Options, convert_image, cutout_image, parallel};
use rayon::ThreadPoolBuilder;

/// Прогоняет `f` на пулах 1 и 4 потоков и на общем пуле, сверяя результаты.
fn assert_parity<T, F>(name: &str, f: F) -> T
where
    T: PartialEq + std::fmt::Debug + Send,
    F: Fn() -> T + Sync,
{
    let one = ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("пул из одного потока")
        .install(|| f());
    let four = ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .expect("пул из четырёх потоков")
        .install(|| f());
    let default = f();
    assert_eq!(one, four, "{name}: 1 поток и 4 потока дают разное");
    assert_eq!(one, default, "{name}: 1 поток и общий пул дают разное");
    one
}

/// Детерминированный шум: цвет пикселя, местами прозрачность и полупрозрачность.
fn noise_image(w: u32, h: u32, seed: u32) -> RgbaImage {
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = next();
            let alpha = match (x + y) % 9 {
                0 => 0,
                1 => 12, // слабая альфа
                2 => 128,
                _ => 255,
            };
            img.put_pixel(
                x,
                y,
                Rgba([(v & 0xff) as u8, ((v >> 8) & 0xff) as u8, ((v >> 16) & 0xff) as u8, alpha]),
            );
        }
    }
    img
}

fn temp_path(name: &str) -> PathBuf {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "pixel-image-parity-{}-{seq}-{name}.png",
        std::process::id()
    ))
}

/// Гистограмма: локальные карты потоков сливаются, порядок сортировки не зависит
/// от порядка слияния.
#[test]
fn parity_histogram() {
    let img = noise_image(768, 576, 5);
    let hist = assert_parity("histogram", || {
        histogram(&img)
            .into_iter()
            .map(|e| (e.color, e.count))
            .collect::<Vec<_>>()
    });
    assert!(!hist.is_empty(), "гистограмма не должна быть пустой");
    let mut sorted = hist.clone();
    sorted.sort_unstable_by_key(|e| e.0);
    assert_eq!(hist, sorted, "порядок записей должен быть отсортирован по цвету");
}

/// Сетка пишется полосами: полоса — все выходные строки одной строки сетки.
#[test]
fn parity_grid() {
    let sprite = noise_image(320, 240, 7);

    for &draw_grid in &[true, false] {
        let out = assert_parity("grid", || {
            upscale_with_grid(&sprite, 4, [17, 34, 51], 80, draw_grid).expect("сетка")
        });
        assert_eq!(out.dimensions(), (320 * 4, 240 * 4));
    }
}

/// Ремап и подсчёт прозрачных ячеек идут по строкам: таблица палитры только
/// читается, поэтому потоки не мешают друг другу.
#[test]
fn parity_remap_and_counts() {
    let src = noise_image(1000, 750, 11);
    let options = Options {
        width: 800,
        colors: 32,
        filter: Filter::Nearest,
        ..Options::default()
    };

    let (sprite, unique, palette_len, transparent, total) = assert_parity("convert_image", || {
        let conversion = convert_image(&src, &options).expect("преобразование");
        let stats = &conversion.stats;
        (
            conversion.sprite,
            stats.unique_colors,
            stats.palette_colors,
            stats.transparent_cells,
            stats.total_cells,
        )
    });

    assert_eq!(sprite.dimensions(), (800, 600));
    assert_eq!(total, 800 * 600);
    assert!(unique > 0 && palette_len > 0, "палитра должна быть непустой");
    assert!(transparent > 0, "в фикстуре есть прозрачные пиксели");
}

/// Блочные фильтры (`Box`, `Mode`) пишут по строкам блоков: блоки независимы,
/// поэтому результат не должен зависеть от числа потоков.
#[test]
fn parity_blocks() {
    let src = noise_image(1200, 900, 29);

    // Крупные блоки: в режиме `Mode` внутри блока реально соревнуются группы цветов.
    let mode = assert_parity("blocks mode", || Filter::Mode.downscale(&src, 300, 225, 16));
    assert_eq!(mode.dimensions(), (300, 225));

    // Мелкие блоки: почти попиксельное усреднение.
    let box_out = assert_parity("blocks box", || Filter::Box.downscale(&src, 800, 600, 16));
    assert_eq!(box_out.dimensions(), (800, 600));
}

/// Порог альфы по строкам: порядок обхода на байты не влияет.
#[test]
fn parity_threshold() {
    let src = noise_image(768, 576, 13);
    let out = assert_parity("threshold", || {
        let mut copy = src.clone();
        apply_alpha_threshold(&mut copy, 128);
        copy
    });

    for p in out.as_raw().as_chunks::<4>().0 {
        assert!(
            *p == [0, 0, 0, 0] || p[3] == 255,
            "альфа обязана стать бинарной: {p:?}"
        );
    }
}

/// Отсечение слабой альфы по кускам: счётчик складывается из независимых частей.
#[test]
fn parity_cut_faint_alpha() {
    let src = noise_image(768, 576, 17);
    let (img, removed) = assert_parity("cut_faint_alpha", || {
        let mut copy = src.clone();
        let removed = cut_faint_alpha(copy.as_mut(), 16);
        (copy, removed)
    });

    assert!(removed > 0, "в фикстуре есть слабая альфа");
    for p in img.as_raw().as_chunks::<4>().0 {
        assert!(p[3] == 0 || p[3] >= 16, "слабой альфы остаться не должно");
    }
}

/// Вырезание фона на крупной фикстуре: чистится тот же буфер, но параллельно
/// режется только отсечение слабой альфы, остальное — проверка на совместимость.
#[test]
fn parity_cutout() {
    let src = noise_image(768, 576, 19);
    let background = BackgroundOptions::default();

    let (image, faint, removed, defringe_changed) = assert_parity("cutout", || {
        let cutout = cutout_image(src.clone(), &background).expect("вырезание");
        let bg = cutout.stats.background.expect("статистика фона");
        (
            cutout.image,
            cutout.stats.faint_alpha_removed,
            bg.removed,
            bg.defringe_changed,
        )
    });

    assert_eq!(image.dimensions(), (768, 576));
    assert!(faint > 0);
    assert!(removed > 0);
    let _ = defringe_changed;
}

/// Запись PNG: композит и упаковка строк идут параллельно, сжатие — нет,
/// поэтому файлы обязаны совпасть побайтово.
#[test]
fn parity_save_png() {
    let img = noise_image(1024, 512, 23);

    // 1. Прозрачный спрайт без заливки — ветка RGBA.
    let rgba_bytes = assert_parity("save_png rgba", || {
        let path = temp_path("rgba");
        pixel_image_core::save_png(&img, None, &path).expect("запись");
        let bytes = std::fs::read(&path).expect("чтение");
        std::fs::remove_file(&path).ok();
        bytes
    });
    assert!(!rgba_bytes.is_empty());

    // 2. Заливка фона — ветка композита в RGB.
    let fill_bytes = assert_parity("save_png fill", || {
        let path = temp_path("fill");
        pixel_image_core::save_png(&img, Some([250, 240, 230]), &path).expect("запись");
        let bytes = std::fs::read(&path).expect("чтение");
        std::fs::remove_file(&path).ok();
        bytes
    });
    assert!(!fill_bytes.is_empty());

    // 3. Полностью непрозрачный спрайт — ветка упаковки RGB.
    let opaque = RgbaImage::from_fn(1000, 400, |x, y| {
        Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255])
    });
    let opaque_bytes = assert_parity("save_png opaque", || {
        let path = temp_path("opaque");
        pixel_image_core::save_png(&opaque, None, &path).expect("запись");
        let bytes = std::fs::read(&path).expect("чтение");
        std::fs::remove_file(&path).ok();
        bytes
    });
    assert!(!opaque_bytes.is_empty());
}

/// Пул один на процесс и не размножается от вызовов: число потоков стабильно.
#[test]
fn no_oversubscription() {
    parallel::init();
    parallel::init();
    let threads = parallel::num_threads();
    assert!(threads >= 1, "потоков {threads}");
    assert_eq!(parallel::num_threads(), threads);
    assert_eq!(rayon::current_num_threads(), threads);
    // Мелкая работа остаётся последовательной: параллель не должна включаться
    // на превью сетки и на фикстурах бюджета аллокаций.
    assert!(!parallel::should_parallel(128 * 85));
}
