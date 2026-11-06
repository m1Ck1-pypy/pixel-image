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
    let mask: Vec<bool> = sprite.pixels().map(|p| p.0[3] != 0).collect();
    let solid =
        |x: i64, y: i64| x >= 0 && y >= 0 && x < wi && y < hi && mask[(y * wi + x) as usize];

    let cell_us = cell as usize;
    let out_w_us = out_w as usize;
    let mut buf = vec![0u8; total]; // нули = полностью прозрачный пиксель

    for gy in 0..h as usize {
        for gx in 0..w as usize {
            let (xi, yi) = (gx as i64, gy as i64);
            let me = solid(xi, yi);
            let v_edge = me || solid(xi - 1, yi); // линия на левой границе ячейки
            let h_edge = me || solid(xi, yi - 1); // линия на верхней границе ячейки
            let corner = v_edge || h_edge || solid(xi - 1, yi - 1);
            if !me && !(draw_grid && corner) {
                continue; // прозрачная ячейка вдали от персонажа остаётся пустой
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
    // Правая и нижняя внешняя граница сетки (линии справа/снизу от крайних ячеек)
    // здесь не рисуются: каждая линия принадлежит ячейке справа/снизу от неё.

    RgbaImage::from_raw(out_w, out_h, buf).ok_or_else(|| Error::msg("ошибка сборки изображения"))
}
