//! Integer geometry in logical UI pixels.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn right(self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(self) -> i32 {
        self.y + self.h
    }

    pub const fn center_x(self) -> i32 {
        self.x + self.w / 2
    }

    pub const fn center_y(self) -> i32 {
        self.y + self.h / 2
    }

    pub const fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn contains(self, point: Point) -> bool {
        point.x >= self.x && point.y >= self.y && point.x < self.right() && point.y < self.bottom()
    }

    /// Move every edge inward by `amount`.
    pub fn inset(self, amount: i32) -> Self {
        let amount = amount.max(0);
        Self {
            x: self.x + amount,
            y: self.y + amount,
            w: (self.w - amount * 2).max(0),
            h: (self.h - amount * 2).max(0),
        }
    }

    /// Rectangle with the same origin and a smaller size.
    pub fn with_height(self, height: i32) -> Self {
        Self {
            h: height.max(0),
            ..self
        }
    }

    /// Overlap of two rectangles, or an empty rectangle at `(0, 0)`.
    pub fn intersect(self, other: Rect) -> Rect {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        if x1 <= x0 || y1 <= y0 {
            Rect::new(0, 0, 0, 0)
        } else {
            Rect::new(x0, y0, x1 - x0, y1 - y0)
        }
    }
}
