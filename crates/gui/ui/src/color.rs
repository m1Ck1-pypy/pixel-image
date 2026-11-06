//! RGBA colours and small compositing helpers.

/// Straight-alpha RGBA colour.
pub type Color = [u8; 4];

/// Opaque colour from its RGB components.
pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    [r, g, b, 255]
}

/// Same colour with another alpha value.
pub const fn with_alpha(color: Color, alpha: u8) -> Color {
    [color[0], color[1], color[2], alpha]
}

/// Source-over compositing of `src` over `dst`.
///
/// Both colours use straight alpha. The result is opaque when `dst` is opaque,
/// which is the only case the toolkit currently needs.
pub fn over(src: Color, dst: Color) -> Color {
    let sa = src[3] as u32;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let da = dst[3] as u32;
    let inv = 255 - sa;
    let out_a = sa + (da * inv + 127) / 255;
    if out_a == 0 {
        return [0, 0, 0, 0];
    }
    let mut out = [0u8; 4];
    for channel in 0..3 {
        let value =
            (src[channel] as u32 * sa + dst[channel] as u32 * da * inv / 255 + out_a / 2) / out_a;
        out[channel] = value.min(255) as u8;
    }
    out[3] = out_a.min(255) as u8;
    out
}

/// Darken a colour by `percent` (0 = unchanged, 100 = black).
pub fn darken(color: Color, percent: u32) -> Color {
    let keep = 100u32.saturating_sub(percent.min(100));
    [
        ((color[0] as u32 * keep) / 100) as u8,
        ((color[1] as u32 * keep) / 100) as u8,
        ((color[2] as u32 * keep) / 100) as u8,
        color[3],
    ]
}

/// Lighten a colour towards white by `percent`.
pub fn lighten(color: Color, percent: u32) -> Color {
    let amount = percent.min(100);
    let inv = 100 - amount;
    [
        ((color[0] as u32 * inv + 255 * amount) / 100) as u8,
        ((color[1] as u32 * inv + 255 * amount) / 100) as u8,
        ((color[2] as u32 * inv + 255 * amount) / 100) as u8,
        color[3],
    ]
}
