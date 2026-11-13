//! Snapping a moved window's edges to nearby edges (Fluxbox's
//! `edgeSnapThreshold`).
//!
//! An edge of the window within [`EdgeSnap`]'s distance of the usable
//! area's edge, or of another window's edge beside it, jumps to it, so
//! windows line up against the output and each other without pixel hunting.

use smithay::utils::{Logical, Point};

use super::placement::Area;

/// The edges a moved window snaps to, and from how far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EdgeSnap {
    /// How close an edge must come to snap, in pixels; 0 for never.
    distance: i32,
    /// The usable area: the window's edges snap to the inside of its edges.
    usable: Area,
    /// Other windows' areas: the window's edges snap to theirs.
    others: Vec<Area>,
}

impl EdgeSnap {
    /// Snaps within `distance` pixels to `usable`'s edges and `others`'.
    pub(crate) fn new(distance: u16, usable: Area, others: Vec<Area>) -> Self {
        Self {
            distance: i32::from(distance),
            usable,
            others,
        }
    }

    /// Returns where `area`'s corner goes once its edges have snapped.
    pub(crate) fn snap(&self, area: Area) -> Point<i32, Logical> {
        if self.distance == 0 {
            return area.loc;
        }
        let horizontal_lines = self
            .others
            .iter()
            .filter(|other| Self::spans_overlap(other.loc.y, other.size.h, area.loc.y, area.size.h))
            .flat_map(|other| [other.loc.x, other.loc.x + other.size.w])
            .chain([self.usable.loc.x, self.usable.loc.x + self.usable.size.w]);
        let vertical_lines = self
            .others
            .iter()
            .filter(|other| Self::spans_overlap(other.loc.x, other.size.w, area.loc.x, area.size.w))
            .flat_map(|other| [other.loc.y, other.loc.y + other.size.h])
            .chain([self.usable.loc.y, self.usable.loc.y + self.usable.size.h]);
        Point::from((
            area.loc.x + self.shift(area.loc.x, area.size.w, horizontal_lines),
            area.loc.y + self.shift(area.loc.y, area.size.h, vertical_lines),
        ))
    }

    /// Returns how far a span from `start`, `length` long, moves so that
    /// one of its ends meets the nearest of `lines` within the distance;
    /// 0 if none is near.
    fn shift(&self, start: i32, length: i32, lines: impl Iterator<Item = i32>) -> i32 {
        lines
            .flat_map(|line| [line - start, line - (start + length)])
            .filter(|shift| shift.abs() <= self.distance)
            .min_by_key(|shift| shift.abs())
            .unwrap_or(0)
    }

    /// Returns whether two spans, each a start and a length, overlap.
    const fn spans_overlap(start: i32, length: i32, other_start: i32, other_length: i32) -> bool {
        start < other_start + other_length && other_start < start + length
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use smithay::utils::Rectangle;

    use super::*;

    /// An area from its corner and size.
    fn area(x: i32, y: i32, width: i32, height: i32) -> Area {
        Rectangle::new((x, y).into(), (width, height).into())
    }

    /// Snapping within 10 pixels in a 1000×600 area, beside a window at
    /// 500, 100, 200 × 200.
    fn snapping() -> EdgeSnap {
        EdgeSnap::new(10, area(0, 0, 1000, 600), vec![area(500, 100, 200, 200)])
    }

    #[rstest]
    #[case::to_the_left_edge(area(7, 300, 100, 100), (0, 300))]
    #[case::to_the_bottom_edge(area(300, 495, 100, 100), (300, 500))]
    #[case::against_a_window(area(395, 120, 100, 100), (400, 120))]
    #[case::beyond_reach(area(380, 120, 100, 100), (380, 120))]
    #[case::not_beside_it(area(395, 400, 100, 100), (395, 400))]
    fn edges_snap_when_near(#[case] moved: Area, #[case] expected: (i32, i32)) {
        let corner = snapping().snap(moved);
        assert_eq!((corner.x, corner.y), expected);
    }

    #[test]
    fn zero_distance_never_snaps() {
        let snap = EdgeSnap::new(0, area(0, 0, 1000, 600), Vec::new());
        assert_eq!(snap.snap(area(1, 1, 10, 10)).x, 1);
    }
}
