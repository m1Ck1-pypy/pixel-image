//! Retro software UI toolkit for `pixel-image-gui`.
//!
//! The crate is deliberately independent of `winit`, `pixels`, `image` and
//! `pixel-image-core`. It only knows how to draw into an RGBA buffer with an
//! integer UI scale and how to report hover/press state. The application
//! translates platform events into [`UiState`].

pub mod canvas;
pub mod color;
pub mod font;
pub mod icons;
pub mod image;
pub mod rect;
pub mod theme;
pub mod widgets;

pub use canvas::Canvas;
pub use color::Color;
pub use font::Font;
pub use rect::{Point, Rect};
pub use theme::Metrics;

/// Input and animation state shared by the application and the widgets.
///
/// Coordinates are logical UI pixels, not physical surface pixels.
#[derive(Clone, Copy, Debug)]
pub struct UiState {
    /// Current cursor position, if the pointer is over the window.
    pub pointer: Option<Point>,
    /// Left mouse button is held down.
    pub mouse_down: bool,
    /// A full press/release happened and no widget has consumed it yet.
    pub click_pending: bool,
    /// Seconds since application start; used for blinking and loading dots.
    pub time: f32,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            pointer: None,
            mouse_down: false,
            click_pending: false,
            time: 0.0,
        }
    }
}

impl UiState {
    /// Whether the pointer is over `rect`.
    pub fn hovered(&self, rect: Rect) -> bool {
        self.pointer.is_some_and(|point| rect.contains(point))
    }

    /// Consume a pending click when it lands inside `rect`.
    ///
    /// The first widget under the cursor wins, so callers must draw and test
    /// widgets in visual stacking order.
    pub fn clicked(&mut self, rect: Rect) -> bool {
        if self.click_pending && self.hovered(rect) {
            self.click_pending = false;
            true
        } else {
            false
        }
    }

    /// Clear one-frame input flags at the end of a redraw.
    pub fn end_frame(&mut self) {
        self.click_pending = false;
    }
}
