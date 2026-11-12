//! Positions, sizes and rectangles in a panel's pixels.
//!
//! Panels are drawn in whole pixels, so these are integers. Positions are
//! signed (a point can lie left of a panel); sizes are not.

/// A position, in pixels from a panel's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Point {
    /// Distance from the left edge.
    pub x: i32,
    /// Distance from the top edge.
    pub y: i32,
}

impl Point {
    /// Creates a point.
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// A width and height, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Size {
    /// Horizontal extent.
    pub width: u32,
    /// Vertical extent.
    pub height: u32,
}

impl Size {
    /// Creates a size.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Returns whether the size covers no pixels.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Space kept free inside a rectangle's edges, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Insets {
    /// Space below the top edge.
    pub top: u32,
    /// Space left of the right edge.
    pub right: u32,
    /// Space above the bottom edge.
    pub bottom: u32,
    /// Space right of the left edge.
    pub left: u32,
}

impl Insets {
    /// Returns the same space on every side.
    #[must_use]
    pub const fn uniform(space: u32) -> Self {
        Self {
            top: space,
            right: space,
            bottom: space,
            left: space,
        }
    }

    /// Returns the total horizontal space.
    #[must_use]
    pub const fn horizontal(self) -> u32 {
        self.left + self.right
    }
}

/// An axis-aligned rectangle of pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    /// The left edge.
    pub x: i32,
    /// The top edge.
    pub y: i32,
    /// Horizontal extent.
    pub width: u32,
    /// Vertical extent.
    pub height: u32,
}

impl Rect {
    /// Creates a rectangle.
    #[must_use]
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Returns a rectangle of `size` at the origin.
    #[must_use]
    pub const fn from_size(size: Size) -> Self {
        Self::new(0, 0, size.width, size.height)
    }

    /// Returns the rectangle's size.
    #[must_use]
    pub const fn size(self) -> Size {
        Size::new(self.width, self.height)
    }

    /// Returns whether `point` lies inside; the right and bottom edges are
    /// outside, so adjacent rectangles never both contain a point.
    #[must_use]
    pub fn contains(self, point: Point) -> bool {
        let right = i64::from(self.x) + i64::from(self.width);
        let bottom = i64::from(self.y) + i64::from(self.height);
        point.x >= self.x
            && point.y >= self.y
            && i64::from(point.x) < right
            && i64::from(point.y) < bottom
    }

    /// Returns the rectangle shrunk by `insets`, never below zero size.
    #[must_use]
    pub fn inset(self, insets: Insets) -> Self {
        Self {
            x: self.x.saturating_add_unsigned(insets.left),
            y: self.y.saturating_add_unsigned(insets.top),
            width: self.width.saturating_sub(insets.left + insets.right),
            height: self.height.saturating_sub(insets.top + insets.bottom),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rectangle_contains_its_top_left_but_not_its_far_edges() {
        let rect = Rect::new(10, 20, 30, 40);
        assert!(rect.contains(Point::new(10, 20)));
        assert!(rect.contains(Point::new(39, 59)));
        assert!(
            !rect.contains(Point::new(40, 20)),
            "the right edge is outside"
        );
        assert!(
            !rect.contains(Point::new(10, 60)),
            "the bottom edge is outside"
        );
        assert!(!rect.contains(Point::new(9, 20)));
    }

    #[test]
    fn insetting_shrinks_from_each_side_and_stops_at_zero() {
        let rect = Rect::new(0, 0, 10, 10);
        let insets = Insets {
            top: 1,
            right: 2,
            bottom: 3,
            left: 4,
        };
        assert_eq!(rect.inset(insets), Rect::new(4, 1, 4, 6));
        assert_eq!(rect.inset(Insets::uniform(8)).size(), Size::new(0, 0));
    }
}
