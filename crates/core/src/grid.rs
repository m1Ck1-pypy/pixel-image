use image::RgbaImage;

use crate::color::Color;
use crate::error::{Error, Result};

/// Накладывает цвет сетки с непрозрачностью `ga` (0..=255) на пиксель (source-over).
#[inline]
fn grid_over(px: [u8; 4], grid: Color, ga: u32) -> [u8; 4] {
    let pa = (px[3] as u32 * (255 - ga) + 127) / 255; // вклад исходного пикселя
    let out_a = ga + pa;
    if out_a == 0 {
        return [0, 0, 0, 0];
    }
    let mix = |g: u8, c: u8| ((g as u32 * ga + c as u32 * pa + out_a / 2) / out_a) as u8;
    [
        mix(grid[0], px[0]),
        mix(grid[1], px[1]),
        mix(grid[2], px[2]),
        out_a.min(255) as u8,
    ]
}

/// Увеличивает спрайт в `cell` раз (nearest neighbour) и рисует сетку только вокруг
/// непрозрачных ячеек. Линия на границе двух ячеек рисуется, если хотя бы одна из
/// них непрозрачна, поэтому контур персонажа обведён со всех сторон, а на
/// прозрачном фоне сетки нет. Пересечения линий затемняются дважды.
pub fn upscale_with_grid(
    sprite: &RgbaImage,
    cell: u32,
    grid: Color,
    alpha: u32,
    draw_grid: bool,
) -> Result<RgbaImage> {
    let (w, h) = sprite.dimensions();
    let out_w = w
        .checked_mul(cell)
        .ok_or_else(|| Error::msg("итоговая ширина слишком велика"))?;
    let out_h = h
        .checked_mul(cell)
        .ok_or_else(|| Error::msg("итоговая высота слишком велика"))?;
    let total = (out_w as usize)
        .checked_mul(out_h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| Error::msg("итоговое изображение слишком велико"))?;

    let (wi, hi) = (w as i64, h as i64);
    let raw = sprite.as_raw();
    let mask: Vec<bool> = raw.as_chunks::<4>().0.iter().map(|p| p[3] != 0).collect();
    let solid =
        |x: i64, y: i64| x >= 0 && y >= 0 && x < wi && y < hi && mask[(y * wi + x) as usize];

    let w_us = w as usize;
    let cell_us = cell as usize;
    let stride = out_w as usize * 4;
    let mut buf = vec![0u8; total]; // нули = полностью прозрачный пиксель

    for gy in 0..h as usize {
        for gx in 0..w_us {
            let (xi, yi) = (gx as i64, gy as i64);
            let me = solid(xi, yi);
            let v_edge = me || solid(xi - 1, yi); // линия на левой границе ячейки
            let h_edge = me || solid(xi, yi - 1); // линия на верхней границе ячейки
            let corner = v_edge || h_edge || solid(xi - 1, yi - 1);
            if !me && !(draw_grid && corner) {
                continue; // прозрачная ячейка вдали от персонажа остаётся пустой
            }

            let base: [u8; 4] = if me {
                let i = (gy * w_us + gx) * 4;
                [raw[i], raw[i + 1], raw[i + 2], raw[i + 3]]
            } else {
                [0, 0, 0, 0]
            };

            // Внутри ячейки всего четыре разных пикселя: угол, верхняя линия,
            // левая линия и тело. Считаем их по одному разу и заливаем ими
            // строки — вместо пересчёта `grid_over` на каждый пиксель.
            let (top_left, top, left) = if draw_grid {
                let mut top_left = base;
                if corner {
                    top_left = grid_over(top_left, grid, alpha);
                    top_left = grid_over(top_left, grid, alpha);
                }
                let mut top = base;
                if h_edge {
                    top = grid_over(top, grid, alpha);
                }
                let mut left = base;
                if v_edge {
                    left = grid_over(left, grid, alpha);
                }
                (top_left, top, left)
            } else {
                (base, base, base)
            };

            let col = gx * cell_us;
            for dy in 0..cell_us {
                let row_start = (gy * cell_us + dy) * stride;
                let row = &mut buf[row_start..row_start + stride];
                let (head, tail) = row[col * 4..(col + cell_us) * 4].split_at_mut(4);
                let (first, fill) = if dy == 0 {
                    (top_left, top)
                } else {
                    (left, base)
                };
                head.copy_from_slice(&first);
                for px in tail.as_chunks_mut::<4>().0 {
                    px.copy_from_slice(&fill);
                }
            }
        }
    }
    // Правая и нижняя внешняя граница сетки (линии справа/снизу от крайних ячеек)
    // здесь не рисуются: каждая линия принадлежит ячейке справа/снизу от неё.

    RgbaImage::from_raw(out_w, out_h, buf).ok_or_else(|| Error::msg("ошибка сборки изображения"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Постраничная (попиксельная) эталонная реализация: буквальный перенос
    /// прежнего кода. Нужна как оракул для проверки построчного писателя.
    fn upscale_with_grid_naive(
        sprite: &RgbaImage,
        cell: u32,
        grid: Color,
        alpha: u32,
        draw_grid: bool,
    ) -> RgbaImage {
        let (w, h) = sprite.dimensions();
        let out_w = w * cell;
        let out_h = h * cell;
        let (wi, hi) = (w as i64, h as i64);
        let mask: Vec<bool> = sprite.pixels().map(|p| p.0[3] != 0).collect();
        let solid =
            |x: i64, y: i64| x >= 0 && y >= 0 && x < wi && y < hi && mask[(y * wi + x) as usize];

        let cell_us = cell as usize;
        let out_w_us = out_w as usize;
        let mut buf = vec![0u8; (out_w * out_h * 4) as usize];

        for gy in 0..h as usize {
            for gx in 0..w as usize {
                let (xi, yi) = (gx as i64, gy as i64);
                let me = solid(xi, yi);
                let v_edge = me || solid(xi - 1, yi);
                let h_edge = me || solid(xi, yi - 1);
                let corner = v_edge || h_edge || solid(xi - 1, yi - 1);
                if !me && !(draw_grid && corner) {
                    continue;
                }
                let base = if me {
                    sprite.get_pixel(gx as u32, gy as u32).0
                } else {
                    [0, 0, 0, 0]
                };

                for dy in 0..cell_us {
                    for dx in 0..cell_us {
                        let mut p = base;
                        if draw_grid {
                            let on_v = dx == 0 && if dy == 0 { corner } else { v_edge };
                            let on_h = dy == 0 && if dx == 0 { corner } else { h_edge };
                            if on_v {
                                p = grid_over(p, grid, alpha);
                            }
                            if on_h {
                                p = grid_over(p, grid, alpha);
                            }
                        }
                        let o = ((gy * cell_us + dy) * out_w_us + gx * cell_us + dx) * 4;
                        buf[o..o + 4].copy_from_slice(&p);
                    }
                }
            }
        }
        RgbaImage::from_raw(out_w, out_h, buf).expect("эталон")
    }

    /// Варианты спрайта: шахматка прозрачности, всё непрозрачное, почти пустое.
    fn sprite_fixture(w: u32, h: u32, variant: u32) -> RgbaImage {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let color = [
                    ((x * 7) % 256) as u8,
                    ((y * 11) % 256) as u8,
                    (variant.wrapping_mul(53) % 256) as u8,
                ];
                let alpha = match variant {
                    0 => {
                        if (x + y) % 3 == 0 {
                            0
                        } else {
                            255
                        }
                    }
                    1 => 255,
                    _ => {
                        if (x * 3 + y) % 7 == 0 {
                            200
                        } else {
                            0
                        }
                    }
                };
                buf.extend_from_slice(&[color[0], color[1], color[2], alpha]);
            }
        }
        RgbaImage::from_raw(w, h, buf).expect("фикстура")
    }

    #[test]
    fn row_writer_matches_naive() {
        let shapes = [(1, 1), (3, 2), (8, 5), (17, 9)];
        let alphas = [0u32, (27 * 255 + 50) / 100, 255];

        for &(w, h) in &shapes {
            for cell in 1..=4u32 {
                for &alpha in &alphas {
                    for &draw in &[true, false] {
                        for variant in 0..3u32 {
                            let sprite = sprite_fixture(w, h, variant);
                            let got = upscale_with_grid(&sprite, cell, [10, 20, 30], alpha, draw)
                                .expect("построчный вариант");
                            let want =
                                upscale_with_grid_naive(&sprite, cell, [10, 20, 30], alpha, draw);
                            assert_eq!(
                                got.dimensions(),
                                want.dimensions(),
                                "размер: {w}x{h}, cell {cell}"
                            );
                            assert_eq!(
                                got.as_raw(),
                                want.as_raw(),
                                "пиксели: {w}x{h}, cell {cell}, alpha {alpha}, draw {draw}, вариант {variant}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Прозрачная ячейка вдали от объекта остаётся полностью прозрачной,
    /// а не закрашивается цветом сетки.
    #[test]
    fn isolated_transparent_cells_stay_empty() {
        let mut sprite = RgbaImage::new(4, 4);
        sprite.put_pixel(1, 1, image::Rgba([200, 100, 50, 255]));
        let out = upscale_with_grid(&sprite, 2, [0, 0, 0], 128, true).expect("сетка");

        // Ячейка (3, 3) не граничит с непрозрачной — её 2x2 пикселя пустые.
        for y in 6..8 {
            for x in 6..8 {
                assert_eq!(out.get_pixel(x, y).0, [0, 0, 0, 0], "ячейка (3,3)");
            }
        }
    }
}
