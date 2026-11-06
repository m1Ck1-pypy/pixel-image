//! Software RGBA canvas with an integer UI scale.
//!
//! The backing buffer is the physical `pixels` frame. All public drawing
//! methods take logical coordinates; one logical pixel is `scale` physical
//! pixels. This keeps the retro pixel grid crisp on high-DPI screens.

use crate::color::{Color, over};
use crate::rect::Rect;
use crate::theme;

pub struct Canvas<'a> {
    width: i32,
    height: i32,
    physical_width: i32,
    physical_height: i32,
    scale: i32,
    clip: Rect,
    buf: &'a mut [u8],
}

impl<'a> Canvas<'a> {
    pub fn new(buf: &'a mut [u8], physical_width: u32, physical_height: u32, scale: u32) -> Self {
        let scale = scale.max(1) as i32;
        let physical_width = physical_width.max(1) as i32;
        let physical_height = physical_height.max(1) as i32;
        debug_assert_eq!(
            buf.len(),
            physical_width as usize * physical_height as usize * 4
        );
        let width = (physical_width + scale - 1) / scale;
        let height = (physical_height + scale - 1) / scale;
        Self {
            width,
            height,
            physical_width,
            physical_height,
            scale,
            clip: Rect::new(0, 0, width, height),
            buf,
        }
    }

    /// Restrict every following draw to `rect` (logical pixels).
    pub fn set_clip(&mut self, rect: Rect) {
        self.clip = rect.intersect(Rect::new(0, 0, self.width, self.height));
    }

    /// Remove the current clipping rectangle.
    pub fn clear_clip(&mut self) {
        self.clip = Rect::new(0, 0, self.width, self.height);
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn scale(&self) -> i32 {
        self.scale
    }

    pub fn physical_size(&self) -> (i32, i32) {
        (self.physical_width, self.physical_height)
    }

    #[inline]
    fn write_physical(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 || x >= self.physical_width || y >= self.physical_height {
            return;
        }
        let index = ((y * self.physical_width + x) * 4) as usize;
        self.buf[index..index + 4].copy_from_slice(&color);
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, color: Color) {
        if x < self.clip.x || y < self.clip.y || x >= self.clip.right() || y >= self.clip.bottom() {
            return;
        }
        let scale = self.scale;
        let px = x * scale;
        let py = y * scale;
        for yy in py..py + scale {
            for xx in px..px + scale {
                self.write_physical(xx, yy, color);
            }
        }
    }

    pub fn clear(&mut self, color: Color) {
        for pixel in self.buf.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&color);
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Color) {
        if w <= 0 || h <= 0 {
            return;
        }
        let x0 = x.max(self.clip.x);
        let y0 = y.max(self.clip.y);
        let x1 = (x + w).min(self.clip.right());
        let y1 = (y + h).min(self.clip.bottom());
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let scale = self.scale;
        let px0 = x0 * scale;
        let px1 = (x1 * scale).min(self.physical_width);
        let py0 = y0 * scale;
        let py1 = (y1 * scale).min(self.physical_height);
        if px0 >= px1 || py0 >= py1 {
            return;
        }
        for py in py0..py1 {
            let row = (py * self.physical_width) as usize * 4;
            let start = row + px0 as usize * 4;
            let end = row + px1 as usize * 4;
            for pixel in self.buf[start..end].as_chunks_mut::<4>().0 {
                pixel.copy_from_slice(&color);
            }
        }
    }

    pub fn stroke_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Color) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.fill_rect(x, y, w, 1, color);
        self.fill_rect(x, y + h - 1, w, 1, color);
        self.fill_rect(x, y, 1, h, color);
        self.fill_rect(x + w - 1, y, 1, h, color);
    }

    pub fn fill_round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, color: Color) {
        if w <= 0 || h <= 0 {
            return;
        }
        let r = radius.max(0).min(w / 2).min(h / 2);
        if r == 0 {
            self.fill_rect(x, y, w, h, color);
            return;
        }
        for row in 0..h {
            let inset = corner_inset(row, h, r);
            self.fill_rect(x + inset, y + row, w - inset * 2, 1, color);
        }
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, color: Color) {
        self.fill_rect(x, y, w, 1, color);
    }

    pub fn vline(&mut self, x: i32, y: i32, h: i32, color: Color) {
        self.fill_rect(x, y, 1, h, color);
    }

    /// Chunky 50% checkerboard used for overlays and transparent previews.
    pub fn dither_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Color, phase: i32) {
        let x0 = x.max(self.clip.x);
        let y0 = y.max(self.clip.y);
        let x1 = (x + w).min(self.clip.right());
        let y1 = (y + h).min(self.clip.bottom());
        for py in y0..y1 {
            for px in x0..x1 {
                if (px + py + phase) & 1 == 0 {
                    self.put(px, py, color);
                }
            }
        }
    }

    /// Checkerboard in physical pixels, so its squares stay square on any DPI.
    pub fn fill_checker_rect(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        let x0 = (x.max(self.clip.x) * self.scale).min(self.physical_width);
        let y0 = (y.max(self.clip.y) * self.scale).min(self.physical_height);
        let x1 = ((x + w).min(self.clip.right()) * self.scale).min(self.physical_width);
        let y1 = ((y + h).min(self.clip.bottom()) * self.scale).min(self.physical_height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let cell = (theme::CHECKER_SIZE * self.scale).max(1);
        for py in y0..y1 {
            let cy = py / cell;
            for px in x0..x1 {
                let cx = px / cell;
                let color = if (cx + cy) & 1 == 0 {
                    theme::CHECK_LIGHT
                } else {
                    theme::CHECK_DARK
                };
                self.write_physical(px, py, color);
            }
        }
    }

    /// Draw an RGBA image into `dst` with nearest-neighbour sampling.
    ///
    /// When `checker` is true, transparent source pixels show the checkerboard
    /// and partially transparent pixels are composited over it. The destination
    /// is clipped to the canvas.
    pub fn draw_rgba_nearest(
        &mut self,
        rgba: &[u8],
        src_w: u32,
        src_h: u32,
        dst: Rect,
        checker: bool,
    ) {
        self.draw_rgba_nearest_inner(rgba, src_w, src_h, dst, None, checker);
    }

    /// Draw an RGBA image composited over a solid `back` colour.
    pub fn draw_rgba_nearest_over(
        &mut self,
        rgba: &[u8],
        src_w: u32,
        src_h: u32,
        dst: Rect,
        back: Color,
    ) {
        self.draw_rgba_nearest_inner(rgba, src_w, src_h, dst, Some(back), false);
    }

    fn draw_rgba_nearest_inner(
        &mut self,
        rgba: &[u8],
        src_w: u32,
        src_h: u32,
        dst: Rect,
        back: Option<Color>,
        checker: bool,
    ) {
        if src_w == 0 || src_h == 0 || dst.is_empty() {
            return;
        }
        if rgba.len() < src_w as usize * src_h as usize * 4 {
            return;
        }
        let clip = dst
            .intersect(Rect::new(0, 0, self.width, self.height))
            .intersect(self.clip);
        if clip.is_empty() {
            return;
        }
        let scale = self.scale;
        let dx0 = clip.x * scale;
        let dy0 = clip.y * scale;
        let dx1 = (clip.right() * scale).min(self.physical_width);
        let dy1 = (clip.bottom() * scale).min(self.physical_height);
        if dx1 <= dx0 || dy1 <= dy0 {
            return;
        }
        // Sample relative to the whole destination rectangle, not to its
        // clipped part: a zoomed image larger than the visible area must be
        // cropped by the clip, not squashed into it.
        let dst_x0 = dst.x * scale;
        let dst_y0 = dst.y * scale;
        let dst_w = (dst.w * scale).max(1) as i64;
        let dst_h = (dst.h * scale).max(1) as i64;
        let cell = (theme::CHECKER_SIZE * scale).max(1);
        for py in dy0..dy1 {
            let sy =
                (((py - dst_y0) as i64 * src_h as i64) / dst_h).clamp(0, src_h as i64 - 1) as u32;
            let cy = py / cell;
            for px in dx0..dx1 {
                let sx = (((px - dst_x0) as i64 * src_w as i64) / dst_w).clamp(0, src_w as i64 - 1)
                    as u32;
                let index = ((sy * src_w + sx) * 4) as usize;
                let a = rgba[index + 3];
                let mut color = [rgba[index], rgba[index + 1], rgba[index + 2], a];
                let backdrop = if checker {
                    Some(if (px / cell + cy) & 1 == 0 {
                        theme::CHECK_LIGHT
                    } else {
                        theme::CHECK_DARK
                    })
                } else {
                    back
                };
                if let Some(backdrop) = backdrop {
                    if a == 0 {
                        color = backdrop;
                    } else if a < 255 {
                        color = over(color, backdrop);
                    }
                }
                self.write_physical(px, py, color);
            }
        }
    }

    /// Read one logical pixel back; out-of-bounds reads as transparent.
    pub fn pixel(&self, x: i32, y: i32) -> Color {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return [0, 0, 0, 0];
        }
        let px = x * self.scale;
        let py = y * self.scale;
        let index = ((py * self.physical_width + px) * 4) as usize;
        [
            self.buf[index],
            self.buf[index + 1],
            self.buf[index + 2],
            self.buf[index + 3],
        ]
    }
}

fn corner_inset(row: i32, h: i32, r: i32) -> i32 {
    if r <= 0 || (row >= r && row < h - r) {
        return 0;
    }
    let dy = if row < r { r - row - 1 } else { row - (h - r) };
    let d = (r * r - dy * dy).max(0);
    r - integer_sqrt(d)
}

fn integer_sqrt(value: i32) -> i32 {
    let mut v = value.max(0);
    let mut result = 0;
    let mut bit = 1 << 30;
    while bit > v {
        bit >>= 2;
    }
    while bit != 0 {
        if v >= result + bit {
            v -= result + bit;
            result = (result >> 1) + bit;
        } else {
            result >>= 1;
        }
        bit >>= 2;
    }
    result
}
