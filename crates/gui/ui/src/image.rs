//! Helpers for fitting an RGBA image into a logical rectangle.

use crate::canvas::Canvas;
use crate::rect::Rect;

/// Largest rectangle with the source aspect ratio that fits inside `area`.
///
/// `allow_upscale = false` is useful for a future 100% mode; the default fit
/// preview allows upscaling so small images are still visible.
pub fn fit_rect(src_w: u32, src_h: u32, area: Rect, allow_upscale: bool) -> Rect {
    if src_w == 0 || src_h == 0 || area.is_empty() {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let scale_x = area.w as f32 / src_w as f32;
    let scale_y = area.h as f32 / src_h as f32;
    let mut scale = scale_x.min(scale_y);
    if !allow_upscale {
        scale = scale.min(1.0);
    }
    if !scale.is_finite() || scale <= 0.0 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let w = ((src_w as f32 * scale).round() as i32).clamp(1, area.w);
    let h = ((src_h as f32 * scale).round() as i32).clamp(1, area.h);
    Rect::new(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h)
}

/// Draw an image fitted into `area` over a checkerboard.
///
/// Returns the destination rectangle used, which is useful for hit-testing or
/// drawing a focus frame later.
pub fn draw_fitted(
    canvas: &mut Canvas,
    rgba: &[u8],
    src_w: u32,
    src_h: u32,
    area: Rect,
    checker: bool,
) -> Rect {
    let dst = fit_rect(src_w, src_h, area, true);
    if !dst.is_empty() {
        if checker {
            canvas.fill_checker_rect(dst.x, dst.y, dst.w, dst.h);
        }
        canvas.draw_rgba_nearest(rgba, src_w, src_h, dst, checker);
    }
    dst
}
