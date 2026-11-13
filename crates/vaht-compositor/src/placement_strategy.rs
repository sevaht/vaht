//! Where a new window goes (Fluxbox's `windowPlacement`).
//!
//! [`PlacementStrategy::place`] is a pure function of the window's size and
//! the [`Room`] it goes into, so every strategy is testable without a
//! compositor. The smart strategies try only positions flush against the
//! usable area's edges and other windows' edges, as Fluxbox does, so windows
//! fit against each other with no gaps; fluxland scanned a 16-pixel grid
//! instead.

use smithay::utils::{Logical, Point, Rectangle, Size};

/// An area in global logical coordinates.
type Area = Rectangle<i32, Logical>;

/// How a place for a new window is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PlacementStrategy {
    /// The first place, row by row, where it overlaps nothing; failing
    /// that, where it overlaps least (Fluxbox's `RowSmartPlacement`). With
    /// candidates only at edges, this places as `RowMinOverlap` does; both
    /// names are kept because Fluxbox configurations use both.
    #[default]
    Row,
    /// The same, column by column (`ColSmartPlacement`); places as
    /// `ColumnMinOverlap` does.
    Column,
    /// Where it overlaps least, row by row (`RowMinOverlapPlacement`).
    RowMinOverlap,
    /// Where it overlaps least, column by column (`ColMinOverlapPlacement`).
    ColumnMinOverlap,
    /// Down and right of the window before, starting one step in from the
    /// corner (`CascadePlacement`).
    Cascade,
    /// Centered under the pointer (`UnderMousePlacement`).
    UnderPointer,
    /// In the middle of the usable area (`CenterPlacement`).
    Center,
}

/// Which way rows are scanned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RowDirection {
    /// From the left.
    #[default]
    LeftToRight,
    /// From the right.
    RightToLeft,
}

/// Which way columns are scanned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ColumnDirection {
    /// From the top.
    #[default]
    TopToBottom,
    /// From the bottom.
    BottomToTop,
}

/// Where a new window can go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Room {
    /// The usable area of the output.
    pub(crate) usable: Area,
    /// The areas of the windows already shown there, frames included.
    pub(crate) occupied: Vec<Area>,
    /// Where the pointer is.
    pub(crate) pointer: Point<i32, Logical>,
}

impl PlacementStrategy {
    /// How far each cascaded window is from the one before.
    const CASCADE_STEP: i32 = 32;

    /// How many steps the cascade takes before starting over at the corner.
    const CASCADE_LENGTH: usize = 10;

    /// Returns where the top-left corner of a window of `size` (frame
    /// included) goes in `room`, scanning rows and columns the ways given.
    pub(crate) fn place(
        self,
        size: Size<i32, Logical>,
        room: &Room,
        rows: RowDirection,
        columns: ColumnDirection,
    ) -> Point<i32, Logical> {
        let usable = room.usable;
        let corner = match self {
            Self::Row | Self::Column | Self::RowMinOverlap | Self::ColumnMinOverlap => {
                Self::least_overlap(self, size, room, rows, columns)
            }
            Self::Cascade => {
                let step = i32::try_from(room.occupied.len() % Self::CASCADE_LENGTH + 1)
                    .expect("the cascade is far shorter than i32::MAX steps");
                usable.loc + Point::from((Self::CASCADE_STEP * step, Self::CASCADE_STEP * step))
            }
            Self::UnderPointer => room.pointer - size.downscale(2).to_point(),
            Self::Center => usable.loc + (usable.size - size).downscale(2).to_point(),
        };
        Self::within(corner, size, usable)
    }

    /// Scans the candidate corners in this strategy's order and returns the
    /// one that overlaps least, the earliest of equals: the first free one,
    /// when there is one.
    fn least_overlap(
        self,
        size: Size<i32, Logical>,
        room: &Room,
        rows: RowDirection,
        columns: ColumnDirection,
    ) -> Point<i32, Logical> {
        let usable = room.usable;
        let mut xs = Self::candidates(
            usable.loc.x,
            usable.size.w,
            size.w,
            room.occupied.iter().map(|area| (area.loc.x, area.size.w)),
        );
        let mut ys = Self::candidates(
            usable.loc.y,
            usable.size.h,
            size.h,
            room.occupied.iter().map(|area| (area.loc.y, area.size.h)),
        );
        if rows == RowDirection::RightToLeft {
            xs.reverse();
        }
        if columns == ColumnDirection::BottomToTop {
            ys.reverse();
        }
        let corners: Vec<Point<i32, Logical>> = match self {
            Self::Column | Self::ColumnMinOverlap => xs
                .iter()
                .flat_map(|&x| ys.iter().map(move |&y| Point::from((x, y))))
                .collect(),
            Self::Row | Self::RowMinOverlap | Self::Cascade | Self::UnderPointer | Self::Center => {
                ys.iter()
                    .flat_map(|&y| xs.iter().map(move |&x| Point::from((x, y))))
                    .collect()
            }
        };
        let overlap = |corner: Point<i32, Logical>| {
            let area = Rectangle::new(corner, size);
            room.occupied
                .iter()
                .filter_map(|occupied| occupied.intersection(area))
                .map(|shared| i64::from(shared.size.w) * i64::from(shared.size.h))
                .sum::<i64>()
        };
        corners
            .iter()
            .copied()
            .min_by_key(|&corner| overlap(corner))
            .unwrap_or(usable.loc)
    }

    /// Returns the positions along one axis where a window of `length`
    /// could start flush against an edge: the usable span's two ends
    /// (`start`, `span` long) and either side of each occupied span
    /// (`(start, length)` pairs). Sorted, without duplicates, and inside
    /// the usable span where it fits.
    fn candidates(
        start: i32,
        span: i32,
        length: i32,
        occupied: impl Iterator<Item = (i32, i32)>,
    ) -> Vec<i32> {
        let last = (start + span - length).max(start);
        let mut positions = vec![start, last];
        for (other_start, other_length) in occupied {
            positions.push(other_start + other_length);
            positions.push(other_start - length);
        }
        positions.retain(|position| (start..=last).contains(position));
        positions.sort_unstable();
        positions.dedup();
        positions
    }

    /// Returns `corner` moved so a window of `size` there stays inside
    /// `usable`, as far as it fits.
    fn within(
        corner: Point<i32, Logical>,
        size: Size<i32, Logical>,
        usable: Area,
    ) -> Point<i32, Logical> {
        let clamp = |position: i32, start: i32, span: i32, length: i32| {
            position.min(start + span - length).max(start)
        };
        Point::from((
            clamp(corner.x, usable.loc.x, usable.size.w, size.w),
            clamp(corner.y, usable.loc.y, usable.size.h, size.h),
        ))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// A 1000×600 usable area holding `occupied`, the pointer at 500, 300.
    fn room(occupied: &[(i32, i32, i32, i32)]) -> Room {
        Room {
            usable: Rectangle::new((0, 0).into(), (1000, 600).into()),
            occupied: occupied
                .iter()
                .map(|&(x, y, width, height)| Rectangle::new((x, y).into(), (width, height).into()))
                .collect(),
            pointer: (500, 300).into(),
        }
    }

    /// Places a 400×300 window in `room` scanning from the top left.
    fn place(strategy: PlacementStrategy, room: &Room) -> (i32, i32) {
        let corner = strategy.place(
            (400, 300).into(),
            room,
            RowDirection::LeftToRight,
            ColumnDirection::TopToBottom,
        );
        (corner.x, corner.y)
    }

    #[rstest]
    #[case::empty_row(PlacementStrategy::Row, &[], (0, 0))]
    #[case::beside_a_window(PlacementStrategy::Row, &[(0, 0, 400, 300)], (400, 0))]
    #[case::below_when_the_row_is_full(PlacementStrategy::Row, &[(0, 0, 400, 300), (400, 0, 600, 300)], (0, 300))]
    #[case::column_goes_down_first(PlacementStrategy::Column, &[(0, 0, 400, 300)], (0, 300))]
    #[case::least_overlap_when_full(PlacementStrategy::RowMinOverlap, &[(0, 0, 1000, 500)], (0, 300))]
    #[case::center(PlacementStrategy::Center, &[], (300, 150))]
    #[case::under_the_pointer(PlacementStrategy::UnderPointer, &[], (300, 150))]
    #[case::cascade_after_two(PlacementStrategy::Cascade, &[(0, 0, 10, 10), (5, 5, 10, 10)], (96, 96))]
    fn strategies_place(
        #[case] strategy: PlacementStrategy,
        #[case] occupied: &[(i32, i32, i32, i32)],
        #[case] expected: (i32, i32),
    ) {
        assert_eq!(place(strategy, &room(occupied)), expected);
    }

    #[test]
    fn right_to_left_starts_at_the_right_edge() {
        let corner = PlacementStrategy::Row.place(
            (400, 300).into(),
            &room(&[]),
            RowDirection::RightToLeft,
            ColumnDirection::BottomToTop,
        );
        assert_eq!((corner.x, corner.y), (600, 300));
    }

    #[test]
    fn a_window_larger_than_the_area_starts_at_its_corner() {
        let corner = PlacementStrategy::Center.place(
            (2000, 900).into(),
            &room(&[]),
            RowDirection::LeftToRight,
            ColumnDirection::TopToBottom,
        );
        assert_eq!((corner.x, corner.y), (0, 0));
    }
}
