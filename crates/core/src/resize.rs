//! Разделимый (separable) ресэмплер с premultiplied alpha.
//!
//! Заменяет `image::imageops::resize` в конвейере и повторяет его арифметику
//! бит в бит:
//!
//! 1. вертикальный проход: `u8` -> `f32`, веса считаются и нормализуются так же,
//!    как в `image::imageops::sample`;
//! 2. горизонтальный проход уже по `f32`-строке, округление в `u8` — только в
//!    самом конце, как `FloatNearest(clamp(t, 0, 255))` у `image`.
//!
//! Порядок проходов именно такой: `image` делает `vertical_sample`, затем
//! `horizontal_sample`. Если поменять их местами, значения совпадут лишь
//! приблизительно (f32 копится по-другому), и байтовый паритет О-1 пропадёт.
//!
//! Чем отличается от `image`:
//!
//! - нет промежуточной копии всего изображения: строка `f32` живёт только внутри
//!   обработки одной выходной строки, поэтому пик памяти — исходник плюс выход;
//! - нет буфера премультиплицированного цвета на `3 Б/пиксель` и буфера альфы на
//!   `1 Б/пиксель`: премультиплицирование считается на лету при чтении тапа
//!   (это чистая функция от пикселя, значение то же самое);
//! - выходные строки считаются параллельно: каждая строка читает исходник и
//!   пишет только свой кусок выхода, поэтому результат не зависит от потоков.

use image::RgbaImage;
use rayon::prelude::*;

use crate::parallel;

/// Ядро фильтра. Радиус поддержки и сами функции — как у `image`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kernel {
    /// Ближайший сосед: у `image` это ядро-«коробка» с поддержкой 0.
    Nearest,
    /// Билинейный.
    Triangle,
    /// Catmull-Rom (кубический B=0, C=0.5).
    CatmullRom,
    /// Гауссов, sigma = 0.5.
    Gaussian,
    /// Lanczos с окном 3.
    Lanczos3,
}

impl Kernel {
    /// Радиус окна фильтра в пикселях источника (до масштабирования).
    pub fn support(self) -> f32 {
        match self {
            Kernel::Nearest => 0.0,
            Kernel::Triangle => 1.0,
            Kernel::CatmullRom => 2.0,
            Kernel::Gaussian | Kernel::Lanczos3 => 3.0,
        }
    }

    /// Значение ядра в точке `x` (центр пикселя — 0).
    fn eval(self, x: f32) -> f32 {
        match self {
            Kernel::Nearest => 1.0, // box: все попавшие в окно пиксели равны
            Kernel::Triangle => triangle_kernel(x),
            Kernel::CatmullRom => catmull_rom_kernel(x),
            Kernel::Gaussian => gaussian_kernel(x),
            Kernel::Lanczos3 => lanczos3_kernel(x),
        }
    }
}

// --- Ядра: буквальный перенос формул из `image::imageops::sample` -------------

fn sinc(t: f32) -> f32 {
    let a = t * std::f32::consts::PI;
    if t == 0.0 { 1.0 } else { a.sin() / a }
}

fn lanczos(x: f32, t: f32) -> f32 {
    if x.abs() < t {
        sinc(x) * sinc(x / t)
    } else {
        0.0
    }
}

fn bc_cubic_spline(x: f32, b: f32, c: f32) -> f32 {
    let a = x.abs();
    let k = if a < 1.0 {
        (12.0 - 9.0 * b - 6.0 * c) * a.powi(3)
            + (-18.0 + 12.0 * b + 6.0 * c) * a.powi(2)
            + (6.0 - 2.0 * b)
    } else if a < 2.0 {
        (-b - 6.0 * c) * a.powi(3)
            + (6.0 * b + 30.0 * c) * a.powi(2)
            + (-12.0 * b - 48.0 * c) * a
            + (8.0 * b + 24.0 * c)
    } else {
        0.0
    };
    k / 6.0
}

fn gaussian(x: f32, r: f32) -> f32 {
    ((2.0 * std::f32::consts::PI).sqrt() * r).recip() * (-x.powi(2) / (2.0 * r.powi(2))).exp()
}

fn lanczos3_kernel(x: f32) -> f32 {
    lanczos(x, 3.0)
}

fn gaussian_kernel(x: f32) -> f32 {
    gaussian(x, 0.5)
}

fn catmull_rom_kernel(x: f32) -> f32 {
    bc_cubic_spline(x, 0.0, 0.5)
}

fn triangle_kernel(x: f32) -> f32 {
    if x.abs() < 1.0 { 1.0 - x.abs() } else { 0.0 }
}

// --- Вспомогательная арифметика каналов --------------------------------------

/// Альфа с отсечением мягкой маски: `0 < a < faint` -> 0.
#[inline]
pub(crate) fn clean_alpha(a: u8, faint: u8) -> u8 {
    if a < faint { 0 } else { a }
}

/// Премультиплицирование канала: вклад цвета с весом альфы.
#[inline]
pub(crate) fn premultiply(c: u8, a: u32) -> u8 {
    ((c as u32 * a + 127) / 255) as u8
}

/// Обратное деление премультиплицированного цвета на альфу.
#[inline]
pub(crate) fn unpremultiply(v: u8, a: u32) -> u8 {
    ((v as u32 * 255 + a / 2).checked_div(a).unwrap_or_default()).min(255) as u8
}

/// Округление накопленного `f32` в канал: ровно `round(clamp(t, 0, 255))`.
#[inline]
fn unit(t: f32) -> u8 {
    t.clamp(0.0, 255.0).round() as u8
}

// --- Веса -------------------------------------------------------------------

/// Таблица весов одной оси: для каждого выходного индекса — `left` и
/// нормализованные веса тапов (`weights[offset[i]..offset[i + 1]]`).
///
/// Считается ровно как в `image`: `ratio`, `sratio = max(1, ratio)`,
/// `src_support = support * sratio`, окно `[floor(x - support), ceil(x + support)]`
/// с зажимом в границы, аргумент ядра `(i - (x - 0.5)) / sratio`, сумма в f32 в
/// порядке тапов, затем деление каждого веса на сумму.
struct Axis {
    left: Vec<u32>,
    offset: Vec<u32>,
    weights: Vec<f32>,
}

impl Axis {
    fn new(in_len: usize, out_len: usize, kernel: Kernel) -> Self {
        let ratio = in_len as f32 / out_len as f32;
        let sratio = if ratio < 1.0 { 1.0 } else { ratio };
        let src_support = kernel.support() * sratio;
        let last = in_len as i64 - 1;

        let mut left = Vec::with_capacity(out_len);
        let mut offset = Vec::with_capacity(out_len + 1);
        // Оценка сверху на число тапов: окно `[x - support, x + support]` плюс
        // запас на ceil/floor. Точная ёмкость не нужна — важно не расти удвоением.
        let taps = (2.0 * src_support).ceil() as usize + 2;
        let mut weights: Vec<f32> = Vec::with_capacity(out_len * taps);
        offset.push(0u32);

        for out in 0..out_len {
            // Центр текущего пикселя выхода в координатах источника.
            let input = (out as f32 + 0.5) * ratio;
            let l = ((input - src_support).floor() as i64).clamp(0, last) as u32;
            let r = ((input + src_support).ceil() as i64).clamp(l as i64 + 1, in_len as i64) as u32;
            // В координаты центров пикселей: центр пикселя i — это i + 0.5.
            let centre = input - 0.5;

            let start = weights.len();
            let mut sum = 0.0f32;
            for i in l..r {
                let w = kernel.eval((i as f32 - centre) / sratio);
                weights.push(w);
                sum += w;
            }
            for w in &mut weights[start..] {
                *w /= sum;
            }

            left.push(l);
            offset.push(weights.len() as u32);
        }

        Self {
            left,
            offset,
            weights,
        }
    }

    #[inline]
    fn entry(&self, out: usize) -> (usize, &[f32]) {
        let start = self.offset[out] as usize;
        let end = self.offset[out + 1] as usize;
        (self.left[out] as usize, &self.weights[start..end])
    }
}

// --- Ресэмплер ---------------------------------------------------------------

/// Уменьшает (или увеличивает) RGBA-изображение до `gw` x `gh`.
///
/// Цвет сначала премультиплицируется, фильтруется и делится обратно на альфу —
/// поэтому фон «под» прозрачностью не подмешивается в контур. Пиксели с
/// `0 < a < faint` считаются полностью прозрачными.
///
/// Результат побайтово совпадает с прежним путём
/// (`image::imageops::resize` по premultiplied RGB и отдельной альфе), включая
/// случай равных размеров — там `image` копирует без фильтрации, и здесь тоже.
pub fn resize_premultiplied(
    src: &RgbaImage,
    gw: u32,
    gh: u32,
    kernel: Kernel,
    faint: u8,
) -> RgbaImage {
    let (w, h) = src.dimensions();
    if w == 0 || h == 0 || gw == 0 || gh == 0 {
        return RgbaImage::new(gw, gh);
    }
    let raw = src.as_raw();
    if (gw, gh) == (w, h) {
        return same_size(raw, w, h, faint);
    }

    let w_us = w as usize;
    let row_bytes = gw as usize * 4;
    let x = Axis::new(w_us, gw as usize, kernel);
    let y = Axis::new(h as usize, gh as usize, kernel);
    let mut out = vec![0u8; row_bytes * gh as usize];

    // Одна выходная строка: вертикальный проход в f32, сразу горизонтальный.
    let write_row = |oy: usize, dst: &mut [u8], vrow: &mut [f32]| {
        let (yl, yw) = y.entry(oy);
        vrow.fill(0.0);

        // Строки-источники читаются подряд (по одной строке на тап), накопление
        // идёт в f32 в порядке тапов — это и есть порядок `image`.
        for (i, &wy) in yw.iter().enumerate() {
            let start = (yl + i) * w_us * 4;
            let src_row = &raw[start..start + w_us * 4];
            for (v, p) in vrow.chunks_exact_mut(4).zip(src_row.as_chunks::<4>().0) {
                let a = clean_alpha(p[3], faint) as u32;
                v[0] += premultiply(p[0], a) as f32 * wy;
                v[1] += premultiply(p[1], a) as f32 * wy;
                v[2] += premultiply(p[2], a) as f32 * wy;
                v[3] += a as f32 * wy;
            }
        }

        for (ox, px) in dst.chunks_exact_mut(4).enumerate() {
            let (xl, xw) = x.entry(ox);
            let mut t = [0f32; 4];
            for (j, &wx) in xw.iter().enumerate() {
                let at = (xl + j) * 4;
                t[0] += vrow[at] * wx;
                t[1] += vrow[at + 1] * wx;
                t[2] += vrow[at + 2] * wx;
                t[3] += vrow[at + 3] * wx;
            }
            let a = unit(t[3]);
            let a32 = a as u32;
            px[0] = unpremultiply(unit(t[0]), a32);
            px[1] = unpremultiply(unit(t[1]), a32);
            px[2] = unpremultiply(unit(t[2]), a32);
            px[3] = a;
        }
    };

    if gh > 1 && parallel::should_parallel(w_us * h as usize) {
        out.par_chunks_mut(row_bytes)
            .enumerate()
            .map_init(
                || vec![0f32; w_us * 4],
                |vrow, (oy, dst)| write_row(oy, dst, vrow),
            )
            .for_each(|()| ());
    } else {
        let mut vrow = vec![0f32; w_us * 4];
        for (oy, dst) in out.chunks_mut(row_bytes).enumerate() {
            write_row(oy, dst, &mut vrow);
        }
    }

    RgbaImage::from_raw(gw, gh, out).expect("размер буфера")
}

/// Случай «размеры совпадают»: `image::imageops::resize` копирует вход без
/// фильтрации, а конвейер всё равно делает цикл «премультиплицировать — снова
/// разделить», поэтому повторяем именно его, а не голую копию.
fn same_size(raw: &[u8], w: u32, h: u32, faint: u8) -> RgbaImage {
    let mut out = vec![0u8; raw.len()];
    for (dst, p) in out.chunks_exact_mut(4).zip(raw.as_chunks::<4>().0) {
        let a = clean_alpha(p[3], faint);
        let a32 = a as u32;
        dst[0] = unpremultiply(premultiply(p[0], a32), a32);
        dst[1] = unpremultiply(premultiply(p[1], a32), a32);
        dst[2] = unpremultiply(premultiply(p[2], a32), a32);
        dst[3] = a;
    }
    RgbaImage::from_raw(w, h, out).expect("размер буфера")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downscale::Filter;
    use image::{GrayImage, RgbImage};
    use rayon::ThreadPoolBuilder;

    const ALL_KERNELS: [Kernel; 5] = [
        Kernel::Nearest,
        Kernel::Triangle,
        Kernel::CatmullRom,
        Kernel::Gaussian,
        Kernel::Lanczos3,
    ];

    /// Эталон = то, что делал конвейер до этапа 4: premultiplied RGB и отдельная
    /// альфа уменьшаются `image::imageops::resize`, затем цвет делится на альфу.
    fn reference(src: &RgbaImage, gw: u32, gh: u32, kernel: Kernel, faint: u8) -> RgbaImage {
        let ft = match kernel {
            Kernel::Nearest => image::imageops::FilterType::Nearest,
            Kernel::Triangle => image::imageops::FilterType::Triangle,
            Kernel::CatmullRom => image::imageops::FilterType::CatmullRom,
            Kernel::Gaussian => image::imageops::FilterType::Gaussian,
            Kernel::Lanczos3 => image::imageops::FilterType::Lanczos3,
        };
        let (w, h) = src.dimensions();
        let n = w as usize * h as usize;
        let mut pre = Vec::with_capacity(n * 3);
        let mut al = Vec::with_capacity(n);
        for p in src.as_raw().as_chunks::<4>().0 {
            let a = clean_alpha(p[3], faint);
            for &v in p.iter().take(3) {
                pre.push(premultiply(v, a as u32));
            }
            al.push(a);
        }
        let pre = RgbImage::from_raw(w, h, pre).expect("pre");
        let al = GrayImage::from_raw(w, h, al).expect("al");
        let pre_s = image::imageops::resize(&pre, gw, gh, ft);
        let al_s = image::imageops::resize(&al, gw, gh, ft);

        let mut out = Vec::with_capacity(gw as usize * gh as usize * 4);
        for (c, &a) in pre_s.as_raw().as_chunks::<3>().0.iter().zip(al_s.as_raw()) {
            for &v in c {
                out.push(unpremultiply(v, a as u32));
            }
            out.push(a);
        }
        RgbaImage::from_raw(gw, gh, out).expect("out")
    }

    /// Фикстура: мягкая кайма, слабая альфа, прозрачный угол и шум.
    fn fixture(w: u32, h: u32, seed: u32) -> RgbaImage {
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
                let edge = x < (w / 10).max(1) && y < (h / 10).max(1);
                let alpha = if edge {
                    0
                } else {
                    match (x * 3 + y * 7) % 13 {
                        0 => 0,
                        1 => 12,
                        2 | 3 => 140,
                        _ => 255,
                    }
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgba([
                        ((x * 9 + v % 31) % 256) as u8,
                        ((y * 5 + (v >> 8) % 17) % 256) as u8,
                        ((x + y) % 256) as u8,
                        alpha,
                    ]),
                );
            }
        }
        img
    }

    const TARGETS: [(u32, u32); 4] = [(8, 8), (16, 12), (40, 30), (13, 7)];

    /// Паритет с прежним путём через `image`, байт в байт, для каждого ядра.
    #[test]
    fn parity_with_image_unchanged() {
        for &(w, h, seed) in &[(32u32, 24u32, 1u32), (17, 9, 2), (64, 48, 3)] {
            let src = fixture(w, h, seed);
            for kernel in ALL_KERNELS {
                for &(gw, gh) in &TARGETS {
                    let got = resize_premultiplied(&src, gw, gh, kernel, 16);
                    let want = reference(&src, gw, gh, kernel, 16);
                    assert_eq!(got.dimensions(), want.dimensions(), "{kernel:?}");
                    assert_eq!(
                        got.as_raw(),
                        want.as_raw(),
                        "пиксели {w}x{h} -> {gw}x{gh}, {kernel:?}"
                    );
                }
            }
        }
    }

    /// То же на «настоящих» пропорциях: вход покрупнее и сетка 128.
    #[test]
    fn parity_large_input() {
        let src = fixture(600, 400, 9);
        let (gw, gh) = (128, 85);
        for kernel in ALL_KERNELS {
            let got = resize_premultiplied(&src, gw, gh, kernel, 16);
            let want = reference(&src, gw, gh, kernel, 16);
            assert_eq!(got.as_raw(), want.as_raw(), "{kernel:?}");
        }
    }

    /// Равные размеры: `image` не фильтрует, конвейер только пересчитывает
    /// премультиплицированное значение.
    #[test]
    fn parity_same_size() {
        let src = fixture(24, 16, 4);
        for kernel in ALL_KERNELS {
            let got = resize_premultiplied(&src, 24, 16, kernel, 16);
            let want = reference(&src, 24, 16, kernel, 16);
            assert_eq!(got.as_raw(), want.as_raw(), "{kernel:?}");
        }
    }

    /// Апскейл: соотношение меньше единицы, `sratio = 1`.
    #[test]
    fn parity_upscale() {
        let src = fixture(11, 7, 5);
        for kernel in ALL_KERNELS {
            let got = resize_premultiplied(&src, 40, 25, kernel, 16);
            let want = reference(&src, 40, 25, kernel, 16);
            assert_eq!(got.as_raw(), want.as_raw(), "{kernel:?}");
        }
    }

    /// Вырожденные размеры и содержимое.
    #[test]
    fn degenerate() {
        // 1×1 -> 1×1, 1×N, N×1 и обратно.
        let one = RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]));
        assert_eq!(
            resize_premultiplied(&one, 1, 1, Kernel::Lanczos3, 16)
                .get_pixel(0, 0)
                .0,
            [10, 20, 30, 255]
        );
        for &(gw, gh) in &[(1u32, 5u32), (5, 1), (1, 1)] {
            let out = resize_premultiplied(&one, gw, gh, Kernel::Lanczos3, 16);
            assert_eq!(out.dimensions(), (gw, gh));
        }

        // Строка и столбец как источник.
        let row = fixture(9, 1, 6);
        assert_eq!(
            resize_premultiplied(&row, 4, 3, Kernel::Triangle, 16).dimensions(),
            (4, 3)
        );
        let col = fixture(1, 9, 7);
        assert_eq!(
            resize_premultiplied(&col, 3, 4, Kernel::Gaussian, 16).dimensions(),
            (3, 4)
        );

        // Нулевая ширина не паникует.
        assert_eq!(
            resize_premultiplied(&row, 0, 3, Kernel::Nearest, 16).dimensions(),
            (0, 3)
        );

        // Полностью прозрачное -> прозрачное с нулевым цветом.
        let transparent = RgbaImage::from_pixel(16, 16, image::Rgba([200, 100, 50, 0]));
        for kernel in ALL_KERNELS {
            let out = resize_premultiplied(&transparent, 5, 5, kernel, 16);
            for p in out.as_raw().as_chunks::<4>().0 {
                assert_eq!(
                    p,
                    &[0, 0, 0, 0],
                    "{kernel:?}: прозрачное должно остаться прозрачным"
                );
            }
        }

        // Полностью непрозрачное -> цвет не меняется.
        let solid = RgbaImage::from_pixel(16, 16, image::Rgba([40, 80, 120, 255]));
        for kernel in ALL_KERNELS {
            let out = resize_premultiplied(&solid, 5, 5, kernel, 16);
            for p in out.as_raw().as_chunks::<4>().0 {
                assert_eq!(p, &[40, 80, 120, 255], "{kernel:?}");
            }
        }

        // Полупрозрачный градиент: цвет и альфа проходят фильтр как в эталоне.
        let gradient =
            RgbaImage::from_fn(64, 8, |x, _| image::Rgba([255, 255, 255, (x * 4) as u8]));
        let got = resize_premultiplied(&gradient, 6, 2, Kernel::Lanczos3, 16);
        let want = reference(&gradient, 6, 2, Kernel::Lanczos3, 16);
        assert_eq!(got.as_raw(), want.as_raw(), "градиент");
    }

    /// О-2: число потоков не меняет ни одного байта.
    #[test]
    fn banded_equals_serial() {
        // 640×480 > `MIN_PIXELS`: параллельная ветка действительно включается.
        let src = fixture(640, 480, 8);
        for kernel in ALL_KERNELS {
            let serial = resize_premultiplied(&src, 64, 48, kernel, 16);
            let under = |threads: usize| {
                ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .expect("пул")
                    .install(|| resize_premultiplied(&src, 64, 48, kernel, 16))
            };
            assert_eq!(serial.as_raw(), under(1).as_raw(), "{kernel:?}: 1 поток");
            assert_eq!(serial.as_raw(), under(4).as_raw(), "{kernel:?}: 4 потока");
            assert_eq!(serial.as_raw(), under(16).as_raw(), "{kernel:?}: 16 потоков");
        }
    }

    /// Ядро берётся из `Filter` так, чтобы `Filter::downscale` и модуль не
    /// разъезжались в сопоставлении фильтров.
    #[test]
    fn filter_kernels_match() {
        assert_eq!(Filter::Nearest.kernel(), Some(Kernel::Nearest));
        assert_eq!(Filter::Triangle.kernel(), Some(Kernel::Triangle));
        assert_eq!(Filter::Catmull.kernel(), Some(Kernel::CatmullRom));
        assert_eq!(Filter::Gaussian.kernel(), Some(Kernel::Gaussian));
        assert_eq!(Filter::Lanczos.kernel(), Some(Kernel::Lanczos3));
        assert_eq!(Filter::Box.kernel(), None);
        assert_eq!(Filter::Mode.kernel(), None);
    }

    /// Слабая альфа: пиксель `a < faint` для фильтра неотличим от прозрачного.
    #[test]
    fn faint_alpha_counts_as_transparent() {
        let with_faint =
            RgbaImage::from_raw(2, 1, vec![200, 100, 50, 15, 10, 20, 30, 255]).expect("фикстура");
        let zeroed =
            RgbaImage::from_raw(2, 1, vec![0, 0, 0, 0, 10, 20, 30, 255]).expect("фикстура");
        for kernel in ALL_KERNELS {
            for &(gw, gh) in &[(1u32, 1u32), (4, 2), (7, 3)] {
                let a = resize_premultiplied(&with_faint, gw, gh, kernel, 16);
                let b = resize_premultiplied(&zeroed, gw, gh, kernel, 16);
                assert_eq!(a.as_raw(), b.as_raw(), "{kernel:?} {gw}x{gh}");
            }
        }
    }
}
