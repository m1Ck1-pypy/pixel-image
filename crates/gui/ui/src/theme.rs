//! Visual tokens sampled from `ui.jpg`, plus layout metrics.
//!
//! All layout values are logical UI pixels. The canvas multiplies them by the
//! integer UI scale.

use crate::color::{Color, rgb};

pub const DESKTOP: Color = rgb(0xD8, 0xD8, 0xD8);
pub const PANEL: Color = rgb(0xF0, 0xF0, 0xF0);
pub const PANEL_ALT: Color = rgb(0xE4, 0xE4, 0xE4);
pub const PANEL_HOVER: Color = rgb(0xDD, 0xDD, 0xDD);
pub const WHITE: Color = rgb(0xFF, 0xFF, 0xFF);

pub const BLUE: Color = rgb(0x00, 0x78, 0xD7);
pub const BLUE_DARK: Color = rgb(0x00, 0x5A, 0x9E);
pub const BLUE_LIGHT: Color = rgb(0x3E, 0x9B, 0xE8);
pub const BLUE_PALE: Color = rgb(0xD6, 0xEA, 0xFB);

pub const RED: Color = rgb(0xE8, 0x11, 0x23);
pub const RED_LIGHT: Color = rgb(0xF0, 0x4A, 0x58);
pub const RED_DARK: Color = rgb(0xB8, 0x0D, 0x1B);

pub const BLACK: Color = rgb(0x10, 0x10, 0x10);
pub const TEXT: Color = rgb(0x1A, 0x1A, 0x1A);
pub const MUTED: Color = rgb(0x60, 0x60, 0x60);
pub const BORDER: Color = rgb(0x50, 0x50, 0x50);
pub const DISABLED: Color = rgb(0xC8, 0xC8, 0xC8);
pub const DISABLED_TEXT: Color = rgb(0x78, 0x78, 0x78);

pub const CHECK_LIGHT: Color = rgb(0xF0, 0xF0, 0xF0);
pub const CHECK_DARK: Color = rgb(0xD8, 0xD8, 0xD8);

pub const HEADER_H: i32 = 36;
pub const TOOLBAR_H: i32 = 44;
pub const STATUS_H: i32 = 28;
pub const PREVIEW_HEADER_H: i32 = 30;
pub const LEFT_PANEL_W: i32 = 340;
pub const GAP: i32 = 12;
pub const OUTER_MARGIN: i32 = 12;
pub const SHADOW: i32 = 3;
pub const RADIUS: i32 = 3;
pub const CHECKER_SIZE: i32 = 8;
pub const BUTTON_H: i32 = 28;
pub const BUTTON_PAD: i32 = 14;

/// Settings sidebar geometry.
pub const ROW_H: i32 = 30;
pub const INPUT_H: i32 = 24;
pub const STEP_W: i32 = 22;
pub const FIELD_W: i32 = 66;
pub const HEX_FIELD_W: i32 = 88;
pub const SWATCH_W: i32 = 22;
pub const DROPDOWN_W: i32 = 132;
pub const SCROLL_W: i32 = 8;
pub const MENU_ITEM_H: i32 = 24;
pub const SIDEBAR_PAD: i32 = 14;

/// Result preview sprite card.
pub const SPRITE_CARD_W: i32 = 148;
pub const SPRITE_CARD_H: i32 = 164;

/// Font scales for the two logical DPI regimes.
///
/// At 100% the canvas is 1:1 and glyph pixels need scale 2 to be readable.
/// At 150%+ the canvas is already enlarged, so scale 1 keeps the physical text
/// height around 10x14 px without making the layout enormous.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub body: i32,
    pub title: i32,
    pub small: i32,
}

impl Metrics {
    pub const fn for_scale(ui_scale: u32) -> Self {
        if ui_scale >= 2 {
            Self {
                body: 1,
                title: 2,
                small: 1,
            }
        } else {
            Self {
                body: 2,
                title: 3,
                small: 1,
            }
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::for_scale(1)
    }
}
