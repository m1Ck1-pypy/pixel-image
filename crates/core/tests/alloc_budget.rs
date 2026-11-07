//! Бюджет аллокаций конвейера.
//!
//! Отдельный тестовый бинарь: глобальный аллокатор видит только его, поэтому
//! счётчики не размываются другими тестами. Измеряем два числа — сколько раз
//! конвейер просит память у ОС и каков пик одновременно занятых байт. Это и
//! есть автоматическая защита от «лишних аллокаций и реаллокаций»: если кто-то
//! добавит копию буфера или рост `Vec` без `with_capacity`, тест покраснеет.
//!
//! Пороги не «красивые», а снятые с текущей реализации с небольшим запасом.
//! Уменьшать их — цель этапов 1–6; увеличивать — только осознанно, с записью в
//! `optimaze.md`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard};

use image::{Rgba, RgbaImage};
use pixel_image_core::{BackgroundOptions, Options, convert_image, cutout_image};

// --- Считающий аллокатор ---------------------------------------------------

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static REALLOCS: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(live, Relaxed);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(live, Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        REALLOCS.fetch_add(1, Relaxed);
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            let live = LIVE.fetch_add(new_size, Relaxed) + new_size - layout.size();
            PEAK.fetch_max(live, Relaxed);
        }
        new_ptr
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

// --- Измерение -------------------------------------------------------------

/// Что конвейер потратил за один вызов.
#[derive(Debug, Clone, Copy)]
struct Budget {
    allocs: usize,
    reallocs: usize,
    peak_bytes: usize,
}

/// Тесты внутри бинаря идут параллельно и делили бы счётчики между собой.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn measure<R>(f: impl FnOnce() -> R) -> (R, Budget) {
    let base = LIVE.load(Relaxed);
    ALLOCS.store(0, Relaxed);
    REALLOCS.store(0, Relaxed);
    PEAK.store(base, Relaxed);

    let out = f();

    let budget = Budget {
        allocs: ALLOCS.load(Relaxed),
        reallocs: REALLOCS.load(Relaxed),
        peak_bytes: PEAK.load(Relaxed).saturating_sub(base),
    };
    (out, budget)
}

/// Та же фикстура, что в `golden.rs`: детерминированная, без внешних файлов.
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
            let (mut r, mut g, mut b, mut a) = (242u8, 238u8, 226u8, 255u8);

            if dist < radius {
                r = (40 + next() % 48) as u8;
                g = (70 + next() % 40) as u8;
                b = (120 + next() % 40) as u8;
            } else if dist < radius + 3.0 {
                a = ((((radius + 3.0 - dist) / 3.0) * 255.0) as u32).min(255) as u8;
            }
            if x < (w / 8).max(1) && y < (h / 8).max(1) {
                a = 0;
            }
            img.put_pixel(x, y, Rgba([r, g, b, a]));
        }
    }
    img
}

fn kib(bytes: usize) -> usize {
    bytes / 1024
}

// --- Пороги ----------------------------------------------------------------
// 256×256 → сетка 128×128, cell 16 → выход 2048×2048 (16 МиБ RGBA).
// Пик = исходник (256 КиБ) + премультиплай (256 КиБ) + сетка (16 МиБ).

#[test]
fn pixel_art_allocation_budget() {
    let _guard = serial();
    let src = fixture(256, 256, 7);
    let options = Options::default();

    let (conversion, budget) = measure(|| convert_image(&src, &options).expect("преобразование"));
    println!("pixel_art: {budget:?}");

    assert_eq!(conversion.stats.grid_width, 128);
    assert_eq!(conversion.stats.grid_height, 128);

    assert!(
        budget.allocs <= PIXEL_ART_ALLOCS,
        "аллокаций {} при бюджете {}: {budget:?}",
        budget.allocs,
        PIXEL_ART_ALLOCS
    );
    assert!(
        budget.reallocs <= PIXEL_ART_REALLOCS,
        "реаллокаций {} при бюджете {}: {budget:?}",
        budget.reallocs,
        PIXEL_ART_REALLOCS
    );
    assert!(
        kib(budget.peak_bytes) <= PIXEL_ART_PEAK_KIB,
        "пик {} КиБ при бюджете {} КиБ: {budget:?}",
        kib(budget.peak_bytes),
        PIXEL_ART_PEAK_KIB
    );
}

#[test]
fn cutout_allocation_budget() {
    let _guard = serial();
    let src = fixture(256, 256, 11);
    let background = BackgroundOptions::default();

    let (cutout, budget) = measure(|| cutout_image(src, &background).expect("вырезание фона"));
    println!("cutout: {budget:?}");

    assert_eq!(cutout.image.dimensions(), (256, 256));
    assert!(!cutout.fully_transparent);

    assert!(
        budget.allocs <= CUTOUT_ALLOCS,
        "аллокаций {} при бюджете {}: {budget:?}",
        budget.allocs,
        CUTOUT_ALLOCS
    );
    assert!(
        budget.reallocs <= CUTOUT_REALLOCS,
        "реаллокаций {} при бюджете {}: {budget:?}",
        budget.reallocs,
        CUTOUT_REALLOCS
    );
    assert!(
        kib(budget.peak_bytes) <= CUTOUT_PEAK_KIB,
        "пик {} КиБ при бюджете {} КиБ: {budget:?}",
        kib(budget.peak_bytes),
        CUTOUT_PEAK_KIB
    );
}

/// Полосный ресэмплер без буферов `pre 3 + al 1` на весь исходник: на
/// 24 Мп-подобном входе (2000×1500) дополнительная память — только выход, две
/// таблицы весов и строка-аккумулятор, а не ещё один полный RGBA-буфер.
///
/// Меряем именно уменьшение, а не весь `convert_image`: на сетке 128×96
/// гистограмма и median cut аллоцируют больше самого фильтра, и замок на
/// ресэмплер в общем бюджете не был бы виден.
#[test]
fn resize_allocation_budget() {
    let _guard = serial();
    let src = fixture(2000, 1500, 13);

    // Замер на пуле из одного потока: иначе в пик попадают строки-аккумуляторы
    // всех потоков (`O(w × threads)`), а тест стал бы зависеть от числа ядер.
    // Сам пул создаём до замера.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("пул из одного потока");
    pool.install(|| {});

    let (sprite, budget) = measure(|| {
        pool.install(|| {
            pixel_image_core::Filter::Lanczos.downscale(&src, 128, 96, pixel_image_core::FAINT_ALPHA)
        })
    });
    println!("resize_2000x1500: {budget:?}");

    assert_eq!(sprite.dimensions(), (128, 96));
    assert!(
        budget.peak_bytes <= RESIZE_PEAK_KIB * 1024,
        "пик {} КиБ при бюджете {} КиБ: {budget:?}",
        kib(budget.peak_bytes),
        RESIZE_PEAK_KIB
    );
    assert!(
        budget.allocs <= RESIZE_ALLOCS,
        "аллокаций {} при бюджете {}: {budget:?}",
        budget.allocs,
        RESIZE_ALLOCS
    );
}

// Значения сняты на текущей реализации (этап 0) и держатся намеренно плотно:
// это замок регрессии, а не запас. Любая лишняя аллокация обязана его уронить,
// поэтому при осознанном улучшении числа уменьшают (этапы 2, 4, 5 плана),
// а при обоснованном ухудшении — увеличивают с записью в `optimaze.md`.
//
// 256×256, сетка 128×128, cell 16.
// Этап 2 убрал из `convert_image` растр сетки (2048×2048×4), поэтому пик
// pixel-art упал с ~16 500 КиБ до ~833 КиБ: остались исходник, премультиплай
// и спрайт. Cutout пока не трогали — там копия исходника обязательна.
//
// Этап 4 (полосный ресэмплер) убрал буферы `pre 3 Б/пиксель` и `al 1 Б/пиксель`,
// поэтому пик pixel-art упал ещё ниже (833 -> 512 КиБ), а аллокаций стало на 11
// меньше: вместо двух полных изображений остались только выход, таблицы весов и
// одна строка-аккумулятор f32.
//
// Замок на сам ресэмплер (2000×1500 -> 128×96, один поток): 8 аллокаций и 165 КиБ
// вместо ~15 МБ у старого пути (`pre` 9 МБ + `al` 3 МБ + f32-промежуток `image`).
const PIXEL_ART_ALLOCS: usize = 121;
const PIXEL_ART_REALLOCS: usize = 29;
const PIXEL_ART_PEAK_KIB: usize = 513;
const CUTOUT_ALLOCS: usize = 50;
const CUTOUT_REALLOCS: usize = 40;
const CUTOUT_PEAK_KIB: usize = 1_700;
const RESIZE_PEAK_KIB: usize = 170;
const RESIZE_ALLOCS: usize = 8;
