pub type Color = [u8; 3];

#[inline]
pub fn key(c: Color) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

/// Разбирает цвет в формате `RRGGBB` (можно с ведущим `#`).
pub fn parse_hex(s: &str) -> Result<Color, String> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return Err(format!("ожидается цвет в формате RRGGBB, получено '{s}'"));
    }
    let v = u32::from_str_radix(s, 16).map_err(|_| format!("некорректный HEX-цвет '{s}'"))?;
    Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_with_and_without_hash() {
        assert_eq!(parse_hex("334455"), Ok([0x33, 0x44, 0x55]));
        assert_eq!(parse_hex("#AABBCC"), Ok([0xAA, 0xBB, 0xCC]));
        assert_eq!(parse_hex("000000"), Ok([0, 0, 0]));
    }

    #[test]
    fn rejects_bad_hex() {
        assert!(parse_hex("345").is_err());
        assert!(parse_hex("GGGGGG").is_err());
        assert!(parse_hex("#33445500").is_err());
    }

    #[test]
    fn key_is_rgb_packed() {
        assert_eq!(key([0x12, 0x34, 0x56]), 0x123456);
        assert_eq!(key([0, 0, 0]), 0);
        assert_eq!(key([255, 255, 255]), 0xFFFFFF);
    }
}
