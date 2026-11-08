//! Small procedural icons drawn from 16x16 bitmaps.

use crate::canvas::Canvas;
use crate::color::Color;
use crate::rect::Rect;
use crate::theme;

/// Edge length of the window icon returned by [`app_icon_rgba`].
pub const APP_ICON_SIZE: u32 = 32;

/// 16x16 monochrome mask: `#` is ink, anything else is transparent.
pub type Bitmap = [&'static str; 16];

const IMAGE: Bitmap = [
    "................",
    "..############..",
    "..#..........#..",
    "..#...####...#..",
    "..#...####...#..",
    "..#...####...#..",
    "..#..........#..",
    "..#......#...#..",
    "..#.....##...#..",
    "..#....####..#..",
    "..#..######..#..",
    "..#.########.#..",
    "..#..........#..",
    "..############..",
    "................",
    "................",
];

/// Floppy disk: "СОХРАНИТЬ".
pub const SAVE: Bitmap = [
    "................",
    "..############..",
    "..#..######..#..",
    "..#..######..#..",
    "..#..######..#..",
    "..#..........#..",
    "..#..........#..",
    "..#..........#..",
    "..#.########.#..",
    "..#.#......#.#..",
    "..#.#......#.#..",
    "..#.#......#.#..",
    "..#.########.#..",
    "..############..",
    "................",
    "................",
];

/// Circular arrow: "СБРОСИТЬ НАСТРОЙКИ".
pub const RESET: Bitmap = [
    "................",
    ".....######.....",
    "....########....",
    "...###########..",
    "..####....#####.",
    ".####......####.",
    ".###......#####.",
    ".###.......###..",
    ".###............",
    ".###........##..",
    ".####......####.",
    "..####....####..",
    "...##########...",
    "....########....",
    ".....######.....",
    "................",
];

/// Arrow pointing left: "ОТМЕНА".
pub const UNDO: Bitmap = [
    "................",
    "................",
    "................",
    ".....#..........",
    "....##..........",
    "...###..........",
    "..#############.",
    ".##############.",
    "..#############.",
    "...###..........",
    "....##..........",
    ".....#..........",
    "................",
    "................",
    "................",
    "................",
];

/// Mirror of [`UNDO`]: "ПОВТОР".
pub const REDO: Bitmap = [
    "................",
    "................",
    "................",
    "..........#.....",
    "..........##....",
    "..........###...",
    ".#############..",
    ".##############.",
    ".#############..",
    "..........###...",
    "..........##....",
    "..........#.....",
    "................",
    "................",
    "................",
    "................",
];

/// Container with an arrow leaving it: "ЭКСПОРТ".
pub const EXPORT: Bitmap = [
    "................",
    ".......##.......",
    "......####......",
    ".....######.....",
    "....########....",
    "......####......",
    "......####......",
    "......####......",
    "......####......",
    "................",
    ".#............#.",
    ".#............#.",
    ".#............#.",
    ".##############.",
    "................",
    "................",
];

/// Draw a 16x16 bitmap centred inside `rect`.
pub fn glyph(canvas: &mut Canvas, rect: Rect, scale: i32, color: Color, rows: &Bitmap) {
    let scale = scale.max(1);
    let size = 16 * scale;
    let x = rect.x + (rect.w - size) / 2;
    let y = rect.y + (rect.h - size) / 2;
    draw_bitmap(canvas, x, y, scale, color, rows);
}

/// Draw the image/picture icon centred inside `rect`.
pub fn image_icon(canvas: &mut Canvas, rect: Rect, scale: i32, color: Color) {
    glyph(canvas, rect, scale, color, &IMAGE);
}

/// RGBA pixels of the window icon: the picture glyph on the accent blue
/// background. Ready for `winit::window::Icon::from_rgba`.
pub fn app_icon_rgba() -> Vec<u8> {
    let scale = APP_ICON_SIZE / 16;
    let mut rgba = vec![0u8; (APP_ICON_SIZE * APP_ICON_SIZE * 4) as usize];
    let background = [theme::BLUE[0], theme::BLUE[1], theme::BLUE[2], 255];
    for index in (0..rgba.len()).step_by(4) {
        rgba[index..index + 4].copy_from_slice(&background);
    }
    for (row, line) in IMAGE.iter().enumerate() {
        for (col, cell) in line.chars().enumerate() {
            if cell != '#' {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let x = col as u32 * scale + dx;
                    let y = row as u32 * scale + dy;
                    let index = ((y * APP_ICON_SIZE + x) * 4) as usize;
                    rgba[index..index + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
    }
    rgba
}

fn draw_bitmap(canvas: &mut Canvas, x: i32, y: i32, scale: i32, color: Color, rows: &[&str; 16]) {
    for (row, line) in rows.iter().enumerate() {
        for (col, cell) in line.chars().enumerate() {
            if cell == '#' {
                canvas.fill_rect(
                    x + col as i32 * scale,
                    y + row as i32 * scale,
                    scale,
                    scale,
                    color,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_well_formed(name: &str, rows: &Bitmap) {
        for (y, row) in rows.iter().enumerate() {
            assert_eq!(row.chars().count(), 16, "{name}: row {y} is not 16 px wide");
            assert!(
                row.chars().all(|cell| cell == '#' || cell == '.'),
                "{name}: row {y} uses characters other than '#' and '.'"
            );
        }
    }

    #[test]
    fn bitmaps_are_square_16x16_masks() {
        assert_well_formed("IMAGE", &IMAGE);
        assert_well_formed("SAVE", &SAVE);
        assert_well_formed("EXPORT", &EXPORT);
        assert_well_formed("RESET", &RESET);
        assert_well_formed("UNDO", &UNDO);
        assert_well_formed("REDO", &REDO);
    }
}
