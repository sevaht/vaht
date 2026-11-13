//! The arithmetic of resizing a window by dragging its edges.
//!
//! [`ResizeEdges`] says which edges move; [`ResizeEdges::resize`] turns a
//! pointer movement into a new size within the client's [`SizeLimits`], and
//! [`ResizeEdges::anchored_location`] keeps the opposite edges still. All of
//! it is plain values, testable without a compositor.

use smithay::utils::{Logical, Point, Rectangle, Size};

/// Which vertical side of a window moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum HorizontalEdge {
    /// Neither: the width stays.
    None,
    /// The left side.
    Left,
    /// The right side.
    Right,
}

/// Which horizontal side of a window moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum VerticalEdge {
    /// Neither: the height stays.
    None,
    /// The top side.
    Top,
    /// The bottom side.
    Bottom,
}

/// The edges a resize moves: one horizontal and one vertical choice, so
/// moving both left and right at once cannot be expressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ResizeEdges {
    /// Which side of the width moves.
    pub(crate) horizontal: HorizontalEdge,
    /// Which side of the height moves.
    pub(crate) vertical: VerticalEdge,
}

impl ResizeEdges {
    /// Returns the edges of `area`'s corner nearest to `point`: Fluxbox's
    /// `StartResizing NearestCorner`.
    pub(crate) fn nearest_corner(
        area: Rectangle<i32, Logical>,
        point: Point<f64, Logical>,
    ) -> Self {
        let center = area.to_f64().loc + area.to_f64().size.downscale(2.0).to_point();
        Self {
            horizontal: if point.x < center.x {
                HorizontalEdge::Left
            } else {
                HorizontalEdge::Right
            },
            vertical: if point.y < center.y {
                VerticalEdge::Top
            } else {
                VerticalEdge::Bottom
            },
        }
    }

    /// Returns the edges a point on `outer`'s border means: the side it is
    /// on, plus the adjacent side when it is within `corner` pixels of a
    /// corner, so a border's ends resize diagonally.
    pub(crate) fn for_border_point(
        outer: Rectangle<i32, Logical>,
        point: Point<i32, Logical>,
        corner: i32,
    ) -> Self {
        let (left, top) = (outer.loc.x, outer.loc.y);
        let (right, bottom) = (left + outer.size.w, top + outer.size.h);
        let horizontal = if point.x < left + corner {
            HorizontalEdge::Left
        } else if point.x >= right - corner {
            HorizontalEdge::Right
        } else {
            HorizontalEdge::None
        };
        let vertical = if point.y < top + corner {
            VerticalEdge::Top
        } else if point.y >= bottom - corner {
            VerticalEdge::Bottom
        } else {
            VerticalEdge::None
        };
        Self {
            horizontal,
            vertical,
        }
    }

    /// Returns the size of `initial` after the pointer moved by `delta`,
    /// within `limits`.
    pub(crate) fn resize(
        self,
        initial: Size<i32, Logical>,
        delta: Point<i32, Logical>,
        limits: SizeLimits,
    ) -> Size<i32, Logical> {
        let width = match self.horizontal {
            HorizontalEdge::None => initial.w,
            HorizontalEdge::Left => initial.w - delta.x,
            HorizontalEdge::Right => initial.w + delta.x,
        };
        let height = match self.vertical {
            VerticalEdge::None => initial.h,
            VerticalEdge::Top => initial.h - delta.y,
            VerticalEdge::Bottom => initial.h + delta.y,
        };
        limits.clamp(width, height)
    }

    /// Returns where content of `size` goes so the edges opposite the moving
    /// ones stay where they were in `initial`.
    pub(crate) fn anchored_location(
        self,
        initial: Rectangle<i32, Logical>,
        size: Size<i32, Logical>,
    ) -> Point<i32, Logical> {
        let x = match self.horizontal {
            HorizontalEdge::Left => initial.loc.x + initial.size.w - size.w,
            HorizontalEdge::None | HorizontalEdge::Right => initial.loc.x,
        };
        let y = match self.vertical {
            VerticalEdge::Top => initial.loc.y + initial.size.h - size.h,
            VerticalEdge::None | VerticalEdge::Bottom => initial.loc.y,
        };
        Point::from((x, y))
    }
}

/// The smallest and largest content size a client accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SizeLimits {
    /// The smallest size; at least 1 × 1.
    minimum: Size<i32, Logical>,
    /// The largest size.
    maximum: Size<i32, Logical>,
}

impl SizeLimits {
    /// Creates limits from what a client sets, where 0 means no limit.
    pub(crate) fn from_client(minimum: Size<i32, Logical>, maximum: Size<i32, Logical>) -> Self {
        let unlimited = |value: i32| if value <= 0 { i32::MAX } else { value };
        let minimum = Size::from((minimum.w.max(1), minimum.h.max(1)));
        Self {
            minimum,
            maximum: Size::from((
                unlimited(maximum.w).max(minimum.w),
                unlimited(maximum.h).max(minimum.h),
            )),
        }
    }

    /// Returns a `width` × `height` brought within the limits. The raw
    /// values may be negative (an edge dragged past the opposite one), which
    /// a `Size` cannot hold, so they are clamped first.
    pub(crate) fn clamp(self, width: i32, height: i32) -> Size<i32, Logical> {
        Size::from((
            width.clamp(self.minimum.w, self.maximum.w),
            height.clamp(self.minimum.h, self.maximum.h),
        ))
    }
}

/// No limits beyond the 1 × 1 minimum: what a client that sets none
/// accepts.
impl Default for SizeLimits {
    fn default() -> Self {
        Self::from_client(Size::default(), Size::default())
    }
}

/// A resize in progress, or just finished: where the window started, and
/// which edges move. It positions the window as the client's new sizes
/// arrive, since a client takes a moment to apply each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResizeAnchor {
    /// The edges being dragged.
    pub(crate) edges: ResizeEdges,
    /// The content area when the resize began.
    pub(crate) initial: Rectangle<i32, Logical>,
    /// Whether the drag has ended; the next commit is the last one anchored.
    pub(crate) is_finishing: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No limits beyond the 1 × 1 minimum.
    fn unlimited() -> SizeLimits {
        SizeLimits::from_client(Size::from((0, 0)), Size::from((0, 0)))
    }

    /// Edges from a horizontal and a vertical choice.
    const fn edges(horizontal: HorizontalEdge, vertical: VerticalEdge) -> ResizeEdges {
        ResizeEdges {
            horizontal,
            vertical,
        }
    }

    #[test]
    fn dragging_the_right_edge_widens_and_keeps_the_left_still() {
        let initial = Rectangle::new((100, 50).into(), (200, 100).into());
        let right = edges(HorizontalEdge::Right, VerticalEdge::None);
        let size = right.resize(initial.size, (30, 999).into(), unlimited());
        assert_eq!(size, Size::from((230, 100)));
        assert_eq!(right.anchored_location(initial, size), initial.loc);
    }

    #[test]
    fn dragging_the_top_left_corner_keeps_the_bottom_right_still() {
        let initial = Rectangle::new((100, 50).into(), (200, 100).into());
        let top_left = edges(HorizontalEdge::Left, VerticalEdge::Top);
        let size = top_left.resize(initial.size, (-20, -10).into(), unlimited());
        assert_eq!(size, Size::from((220, 110)));
        let location = top_left.anchored_location(initial, size);
        assert_eq!(location, Point::from((80, 40)));
        assert_eq!(location.x + size.w, 300, "the right edge stays");
        assert_eq!(location.y + size.h, 150, "the bottom edge stays");
    }

    #[test]
    fn the_client_limits_bound_the_size() {
        let limits = SizeLimits::from_client(Size::from((50, 40)), Size::from((300, 0)));
        let bottom_right = edges(HorizontalEdge::Right, VerticalEdge::Bottom);
        assert_eq!(
            bottom_right.resize(Size::from((100, 100)), (-500, -500).into(), limits),
            Size::from((50, 40))
        );
        assert_eq!(
            bottom_right.resize(Size::from((100, 100)), (500, 500).into(), limits),
            Size::from((300, 600)),
            "no maximum height"
        );
    }

    #[test]
    fn a_maximum_below_the_minimum_is_raised_to_it() {
        let limits = SizeLimits::from_client(Size::from((80, 80)), Size::from((10, 10)));
        assert_eq!(limits.clamp(5, 5), Size::from((80, 80)));
    }

    #[test]
    fn the_nearest_corner_follows_the_quadrant() {
        let area = Rectangle::new((0, 0).into(), (100, 100).into());
        assert_eq!(
            ResizeEdges::nearest_corner(area, (10.0, 90.0).into()),
            edges(HorizontalEdge::Left, VerticalEdge::Bottom)
        );
        assert_eq!(
            ResizeEdges::nearest_corner(area, (70.0, 20.0).into()),
            edges(HorizontalEdge::Right, VerticalEdge::Top)
        );
    }

    #[test]
    fn a_border_point_near_a_corner_resizes_diagonally() {
        let outer = Rectangle::new((0, 0).into(), (200, 100).into());
        assert_eq!(
            ResizeEdges::for_border_point(outer, (100, 99).into(), 20),
            edges(HorizontalEdge::None, VerticalEdge::Bottom),
            "the middle of the bottom border"
        );
        assert_eq!(
            ResizeEdges::for_border_point(outer, (195, 99).into(), 20),
            edges(HorizontalEdge::Right, VerticalEdge::Bottom),
            "the bottom border's right end"
        );
        assert_eq!(
            ResizeEdges::for_border_point(outer, (0, 50).into(), 20),
            edges(HorizontalEdge::Left, VerticalEdge::None),
            "the middle of the left border"
        );
    }
}
