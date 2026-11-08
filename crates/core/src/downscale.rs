use std::collections::HashMap;

use image::RgbaImage;
use rayon::prelude::*;

use crate::color::Color;
use crate::parallel;
use crate::resize::{self, Kernel};

/// Способ уменьшения изображения до сетки.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// Lanczos3: самый чёткий, но возможны ореолы у резких границ
    Lanczos,
    /// Catmull-Rom: чёткий, ореолы слабее, чем у Lanczos
    Catmull,
    /// Билинейный: мягкий, без ореолов
    Triangle,
    /// Гауссов: самый размытый
    Gaussian,
    /// Ближайший сосед: берётся один пиксель, самые жёсткие края, возможны потери деталей
    Nearest,
    /// Среднее цветов блока: честное усреднение, без ореолов
    Box,
    /// Самый частый цвет блока: чистые края, без смешанных оттенков
    Mode,
}

impl Filter {
    /// Уменьшает изображение до `gw` x `gh` этим фильтром с учётом альфа-канала:
    /// цвет краевых пикселей считается только по непрозрачной части, поэтому фон
    /// «под» прозрачностью (белый или чёрный) не подмешивается в контур.
    ///
    /// `faint` — пиксели с `0 < альфа < faint` считаются полностью прозрачными
    /// (мягкая маска от JPEG-сглаживания). Так исходник не нужно копировать и
    /// чистить перед уменьшением: фильтр сам не берёт их в расчёт.
    pub fn downscale(&self, src: &RgbaImage, gw: u32, gh: u32, faint: u8) -> RgbaImage {
        match *self {
            Filter::Box => downscale_blocks(src, gw, gh, false, faint),
            Filter::Mode => downscale_blocks(src, gw, gh, true, faint),
            _ => resize::resize_premultiplied(
                src,
                gw,
                gh,
                self.kernel().expect("у фильтра есть ядро"),
                faint,
            ),
        }
    }

    /// Ядро разделимого ресэмплера; `None` у блочных фильтров, которые считаются
    /// не свёрткой, а средним/модой по блоку.
    pub(crate) fn kernel(&self) -> Option<Kernel> {
        match self {
            Filter::Lanczos => Some(Kernel::Lanczos3),
            Filter::Catmull => Some(Kernel::CatmullRom),
            Filter::Triangle => Some(Kernel::Triangle),
            Filter::Gaussian => Some(Kernel::Gaussian),
            Filter::Nearest => Some(Kernel::Nearest),
            Filter::Box | Filter::Mode => None,
        }
    }
}

/// Альфа с отсечением мягкой маски: `0 < a < faint` -> 0.
fn clean_alpha(a: u8, faint: u8) -> u8 {
    crate::resize::clean_alpha(a, faint)
}

/// Делит изображение на `gw` x `gh` блоков. В режиме `mode` цвет блока — самый
/// частый цвет среди непрозрачных пикселей (цвета группируются по 5 бит на канал,
/// внутри победившей группы берётся среднее). Иначе — среднее с весом альфы.
/// Альфа блока — среднее альфы всех его пикселей.
fn downscale_blocks(src: &RgbaImage, gw: u32, gh: u32, mode: bool, faint: u8) -> RgbaImage {
    let (w, h) = src.dimensions();
    let raw = src.as_raw();
    let mut out = vec![0u8; gw as usize * gh as usize * 4];
    if gw == 0 || gh == 0 {
        return RgbaImage::from_raw(gw, gh, out).expect("размер буфера");
    }
    let row_cells = gw as usize * 4;

    // Строка блоков независима от соседних: у каждого блока свои границы, свой
    // аккумулятор и своя ячейка выхода. Поэтому блоки можно раздать потокам без
    // изменения результата (в т.ч. «самый частый цвет» с тай-брейком по ключу).
    let write_row = |gy: u32, cells: &mut [u8]| {
        // группа -> [число пикселей, сумма R, сумма G, сумма B]
        let mut buckets: HashMap<u16, [u32; 4]> = HashMap::new();
        let y0 = (gy as u64 * h as u64 / gh as u64) as u32;
        let y1 = (((gy as u64 + 1) * h as u64 / gh as u64) as u32).clamp(y0 + 1, h);

        for (gx, cell) in cells.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let gx = gx as u32;
            let x0 = (gx as u64 * w as u64 / gw as u64) as u32;
            let x1 = (((gx as u64 + 1) * w as u64 / gw as u64) as u32).clamp(x0 + 1, w);

            buckets.clear();
            let mut sum_a = 0u64;
            let mut sum_ca = [0u64; 3]; // сумма цвет * альфа
            for y in y0..y1 {
                let row = y as usize * w as usize * 4;
                for x in x0..x1 {
                    let i = row + x as usize * 4;
                    let (r, g, b, a) = (
                        raw[i],
                        raw[i + 1],
                        raw[i + 2],
                        clean_alpha(raw[i + 3], faint),
                    );
                    sum_a += a as u64;
                    if mode {
                        if a >= 128 {
                            let k =
                                ((r >> 3) as u16) << 10 | ((g >> 3) as u16) << 5 | (b >> 3) as u16;
                            let e = buckets.entry(k).or_insert([0; 4]);
                            e[0] += 1;
                            e[1] += r as u32;
                            e[2] += g as u32;
                            e[3] += b as u32;
                        }
                    } else {
                        sum_ca[0] += r as u64 * a as u64;
                        sum_ca[1] += g as u64 * a as u64;
                        sum_ca[2] += b as u64 * a as u64;
                    }
                }
            }

            let n = (y1 - y0) as u64 * (x1 - x0) as u64;
            let alpha = ((sum_a + n / 2) / n) as u8;
            let color: Color = if mode {
                // Самая большая группа; при равенстве — с меньшим ключом (детерминированно).
                match buckets
                    .iter()
                    .max_by(|a, b| a.1[0].cmp(&b.1[0]).then_with(|| b.0.cmp(a.0)))
                {
                    Some((_, e)) => {
                        let c = e[0];
                        [
                            ((e[1] + c / 2) / c) as u8,
                            ((e[2] + c / 2) / c) as u8,
                            ((e[3] + c / 2) / c) as u8,
                        ]
                    }
                    None => [0, 0, 0],
                }
            } else {
                // при sum_a == 0 (полностью прозрачный блок) все каналы становятся 0
                [
                    ((sum_ca[0] + sum_a / 2)
                        .checked_div(sum_a)
                        .unwrap_or_default()) as u8,
                    ((sum_ca[1] + sum_a / 2)
                        .checked_div(sum_a)
                        .unwrap_or_default()) as u8,
                    ((sum_ca[2] + sum_a / 2)
                        .checked_div(sum_a)
                        .unwrap_or_default()) as u8,
                ]
            };
            cell.copy_from_slice(&[color[0], color[1], color[2], alpha]);
        }
    };

    if gh > 1 && parallel::should_parallel(w as usize * h as usize) {
        out.par_chunks_mut(row_cells)
            .enumerate()
            .for_each(|(gy, cells)| write_row(gy as u32, cells));
    } else {
        for (gy, cells) in out.chunks_mut(row_cells).enumerate() {
            write_row(gy as u32, cells);
        }
    }
    RgbaImage::from_raw(gw, gh, out).expect("размер буфера")
}

/// Делает прозрачность бинарной: альфа >= порога -> непрозрачный, иначе прозрачный.
pub fn apply_alpha_threshold(img: &mut RgbaImage, threshold: u8) {
    let width = img.width() as usize;
    let raw = img.as_mut();
    if width == 0 || !parallel::should_parallel(raw.len() / 4) {
        threshold_rows(raw, threshold);
        return;
    }
    // Строки независимы: каждый поток правит свою, порядок обхода на байты не влияет.
    raw.par_chunks_mut(width * 4)
        .for_each(|row| threshold_rows(row, threshold));
}

fn threshold_rows(raw: &mut [u8], threshold: u8) {
    for p in raw.as_chunks_mut::<4>().0 {
        if p[3] >= threshold {
            p[3] = 255;
        } else {
            p.copy_from_slice(&[0, 0, 0, 0]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Фикстура с полупрозрачной каймой, мягкой маской и прозрачным углом.
    fn sprite_fixture(w: u32, h: u32, seed: u32) -> RgbaImage {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let edge = x < w / 4 && y < h / 4;
                let alpha = if edge {
                    0
                } else if (x + y + seed).is_multiple_of(7) {
                    12 // слабая альфа: считается прозрачной
                } else if (x + y + seed).is_multiple_of(5) {
                    128
                } else {
                    255
                };
                buf.extend_from_slice(&[
                    ((x * 13 + seed) % 256) as u8,
                    ((y * 7 + seed * 3) % 256) as u8,
                    ((x * y + 90) % 256) as u8,
                    alpha,
                ]);
            }
        }
        RgbaImage::from_raw(w, h, buf).expect("фикстура")
    }

    /// Прежняя семантика: слабая альфа обнулена заранее, отдельной копией.
    fn precut(src: &RgbaImage) -> RgbaImage {
        let mut copy = src.clone();
        crate::clean::cut_faint_alpha(copy.as_mut(), crate::pipeline::FAINT_ALPHA);
        copy
    }

    const ALL_FILTERS: [Filter; 7] = [
        Filter::Lanczos,
        Filter::Catmull,
        Filter::Triangle,
        Filter::Gaussian,
        Filter::Nearest,
        Filter::Box,
        Filter::Mode,
    ];

    /// Новый путь (исходник как есть + отсечение слабой альфы внутри фильтра)
    /// обязан совпадать со старым (копия с обнулённой слабой альфой).
    #[test]
    fn filter_output_unchanged() {
        let fixtures = [(32u32, 24u32, 1u32), (17, 9, 2), (64, 48, 3)];
        let targets = [(8u32, 8u32), (16, 12), (40, 30)];

        for &(w, h, seed) in &fixtures {
            let raw = sprite_fixture(w, h, seed);
            let cut = precut(&raw);
            for filter in ALL_FILTERS {
                for &(gw, gh) in &targets {
                    let got = filter.downscale(&raw, gw, gh, crate::pipeline::FAINT_ALPHA);
                    let want = filter.downscale(&cut, gw, gh, 0);
                    assert_eq!(
                        got.dimensions(),
                        want.dimensions(),
                        "{w}x{h} -> {gw}x{gh}, {filter:?}"
                    );
                    assert_eq!(
                        got.as_raw(),
                        want.as_raw(),
                        "пиксели: {w}x{h} -> {gw}x{gh}, {filter:?}"
                    );
                }
            }
        }
    }

    /// Пиксель со слабой альфой эквивалентен полностью прозрачному: цвет
    /// «под» ним не подмешивается ни в одном фильтре.
    #[test]
    fn faint_alpha_counts_as_transparent() {
        let with_faint =
            RgbaImage::from_raw(2, 1, vec![200, 100, 50, 15, 10, 20, 30, 255]).expect("фикстура");
        let zeroed =
            RgbaImage::from_raw(2, 1, vec![0, 0, 0, 0, 10, 20, 30, 255]).expect("фикстура");

        for filter in ALL_FILTERS {
            let a = filter.downscale(&with_faint, 1, 1, crate::pipeline::FAINT_ALPHA);
            let b = filter.downscale(&zeroed, 1, 1, crate::pipeline::FAINT_ALPHA);
            assert_eq!(a.as_raw(), b.as_raw(), "{filter:?}");
        }
    }

    #[test]
    fn alpha_threshold_zeroes_low_alpha() {
        let mut img =
            RgbaImage::from_raw(2, 1, vec![10, 20, 30, 100, 40, 50, 60, 200]).expect("фикстура");
        apply_alpha_threshold(&mut img, 128);
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 0, 0]);
        assert_eq!(img.get_pixel(1, 0).0, [40, 50, 60, 255]);
    }
}
