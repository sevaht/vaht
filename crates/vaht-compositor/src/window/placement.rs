//! Where a window is and why: floating, maximized, tiled to part of the
//! output, or fullscreen (01 §4.2).
//!
//! [`Placement`] is one value, so a window can never be maximized and tiled
//! at once, and the area a window covers always follows from it. Changing
//! placement is a pure function from the old value to the new one:
//! [`Placement::with_maximized_toggled`], [`Placement::with_fullscreen_toggled`]
//! and [`Placement::snap_step`] (the Windows-key-and-arrow ladder of my
//! fluxland setup), all testable without a compositor.
//!
//! Areas here are a window's *outer* area, frame included: halves and
//! quarters are halves and quarters of what the user sees.

use std::fmt;

use smithay::utils::{Logical, Rectangle};

/// An area in global logical coordinates.
pub(crate) type Area = Rectangle<i32, Logical>;

impl fmt::Display for TileCell {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} {} {}",
            self.x, self.y, self.width, self.height
        )
    }
}

/// The parts of an output a window can be placed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputArea {
    /// All of it: where a fullscreen window goes.
    pub(crate) whole: Area,
    /// What panels such as the taskbar leave free: where windows are
    /// maximized and tiled.
    pub(crate) usable: Area,
}

/// Where a window is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum Placement {
    /// Wherever it was put: its area is its own.
    #[default]
    Floating,
    /// Filling the usable area along `axes`.
    Maximized {
        /// The axes filled.
        axes: Axes,
        /// The area it had when floating, to return to.
        restore: Area,
        /// The cell a snap step up maximized it from, which a step down
        /// returns to.
        return_to: Option<TileCell>,
    },
    /// Covering one cell of the usable area.
    Tiled {
        /// The cell.
        cell: TileCell,
        /// The area it had when floating, to return to.
        restore: Area,
    },
    /// Covering the whole output, with no frame.
    Fullscreen {
        /// The placement it returns to.
        underneath: Box<Placement>,
        /// The area it had just before, to return to if that was floating.
        restore: Area,
    },
}

impl Placement {
    /// Returns whether the window is fullscreen.
    pub(crate) const fn is_fullscreen(&self) -> bool {
        matches!(self, Self::Fullscreen { .. })
    }

    /// Returns whether the window is maximized along both axes.
    pub(crate) const fn is_fully_maximized(&self) -> bool {
        matches!(
            self,
            Self::Maximized {
                axes: Axes::Both,
                ..
            }
        )
    }

    /// Returns the area the window covers when placed so, given its
    /// `current` area, the output's `usable` area and its `whole` area; a
    /// floating window keeps `current`.
    pub(crate) fn area(&self, current: Area, usable: Area, whole: Area) -> Area {
        match self {
            Self::Floating => current,
            Self::Maximized { axes, .. } => axes.fill(current, usable),
            Self::Tiled { cell, .. } => cell.area_in(usable),
            Self::Fullscreen { .. } => whole,
        }
    }

    /// Returns the area to return to when the window floats again, given
    /// its `current` area.
    pub(crate) fn restore_area(&self, current: Area) -> Area {
        match self {
            Self::Floating => current,
            Self::Maximized { restore, .. }
            | Self::Tiled { restore, .. }
            | Self::Fullscreen { restore, .. } => *restore,
        }
    }

    /// Returns the placement after toggling maximization along `axes`, the
    /// window covering `current`. Fullscreen windows stay as they are.
    #[must_use]
    pub(crate) fn with_maximized_toggled(self, axes: Axes, current: Area) -> Self {
        let restore = self.restore_area(current);
        let maximized = |axes| Self::Maximized {
            axes,
            restore,
            return_to: None,
        };
        match self {
            Self::Fullscreen { .. } => self,
            Self::Floating | Self::Tiled { .. } => maximized(axes),
            Self::Maximized { axes: held, .. } => {
                held.with_toggled(axes).map_or(Self::Floating, maximized)
            }
        }
    }

    /// Returns the placement after toggling fullscreen, the window
    /// covering `current`.
    #[must_use]
    pub(crate) fn with_fullscreen_toggled(self, current: Area) -> Self {
        match self {
            Self::Fullscreen { underneath, .. } => *underneath,
            Self::Floating | Self::Maximized { .. } | Self::Tiled { .. } => Self::Fullscreen {
                restore: current,
                underneath: Box::new(self),
            },
        }
    }

    /// Takes one step of the snap ladder in `direction`, the window
    /// covering `current` in the `usable` area, as the Windows key and an
    /// arrow do:
    /// - left and right snap to that half, and undo a snap to the other
    ///   half;
    /// - up maximizes, or makes a half a top quarter, or maximizes a top
    ///   quarter;
    /// - down restores a maximized window (to the quarter it came from),
    ///   makes a half a bottom quarter and a top quarter a half, and
    ///   minimizes anything else.
    ///
    /// A floating window placed by hand exactly on a half or quarter counts
    /// as snapped there.
    #[must_use]
    pub(crate) fn snap_step(self, direction: Direction, current: Area, usable: Area) -> SnapStep {
        let placement = self.adopting_snap_cell(current, usable);
        let restore = placement.restore_area(current);
        let snapped = match &placement {
            Self::Tiled { cell, .. } => SnapCell::of(*cell),
            Self::Floating | Self::Maximized { .. } | Self::Fullscreen { .. } => None,
        };
        let tile = |cell: SnapCell| {
            SnapStep::Place(Self::Tiled {
                cell: cell.cell(),
                restore,
            })
        };
        match (direction, &placement, snapped) {
            (_, Self::Fullscreen { .. }, _) => SnapStep::Unchanged,
            (Direction::Left | Direction::Right, _, Some(cell)) => {
                let side = Side::of(direction).expect("left and right are sides");
                if cell.side == side {
                    // Onward to the next output, once there are several.
                    SnapStep::Unchanged
                } else if cell.row == Row::Whole {
                    SnapStep::Place(Self::Floating)
                } else {
                    tile(SnapCell {
                        side,
                        row: cell.row,
                    })
                }
            }
            (Direction::Left | Direction::Right, _, None) => tile(SnapCell {
                side: Side::of(direction).expect("left and right are sides"),
                row: Row::Whole,
            }),
            (Direction::Up, Self::Floating, _) => SnapStep::Place(Self::Maximized {
                axes: Axes::Both,
                restore,
                return_to: None,
            }),
            (Direction::Up, _, Some(cell)) => match cell.row.above() {
                Some(row) => tile(SnapCell { row, ..cell }),
                None => SnapStep::Place(Self::Maximized {
                    axes: Axes::Both,
                    restore,
                    return_to: Some(cell.cell()),
                }),
            },
            (
                Direction::Down,
                Self::Maximized {
                    axes: Axes::Both,
                    return_to,
                    ..
                },
                _,
            ) => SnapStep::Place(
                return_to.map_or(Self::Floating, |cell| Self::Tiled { cell, restore }),
            ),
            (Direction::Down, _, Some(cell)) => match cell.row.below() {
                Some(row) => tile(SnapCell { row, ..cell }),
                None => SnapStep::Minimize,
            },
            (Direction::Down, Self::Floating, None) => SnapStep::Minimize,
            // A partly maximized or freely tiled window has no rung to go to.
            (Direction::Up | Direction::Down, _, None) => SnapStep::Unchanged,
        }
    }

    /// Returns the placement, taking a floating window that sits on a half
    /// or quarter of `usable` for snapped there. Where it came from is not
    /// known then, so it would return to two thirds of the area, centered.
    fn adopting_snap_cell(self, current: Area, usable: Area) -> Self {
        if self != Self::Floating {
            return self;
        }
        let adopted = SnapCell::ALL
            .into_iter()
            .find(|cell| cell.cell().is_occupied_by(current, usable));
        match adopted {
            Some(cell) => {
                let size = usable.size.downscale(3).upscale(2);
                let corner = usable.loc + (usable.size - size).downscale(2).to_point();
                Self::Tiled {
                    cell: cell.cell(),
                    restore: Rectangle::new(corner, size),
                }
            }
            None => self,
        }
    }
}

/// What a snap step does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SnapStep {
    /// Places the window so.
    Place(Placement),
    /// Minimizes it.
    Minimize,
    /// Nothing.
    Unchanged,
}

/// A direction of the snap ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumString, strum::Display)]
#[strum(serialize_all = "kebab-case")]
pub(crate) enum Direction {
    /// Left.
    Left,
    /// Right.
    Right,
    /// Up.
    Up,
    /// Down.
    Down,
}

/// The axes along which a window is maximized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumString, strum::Display)]
#[strum(serialize_all = "kebab-case")]
pub(crate) enum Axes {
    /// Side to side only.
    Horizontal,
    /// Top to bottom only.
    Vertical,
    /// Both.
    Both,
}

impl Axes {
    /// Returns the axes left after toggling `toggled`: none, if that takes
    /// them all away. Toggling both on a window maximized along both takes
    /// both away; on any other, it fills both.
    #[must_use]
    pub(crate) const fn with_toggled(self, toggled: Self) -> Option<Self> {
        match (self, toggled) {
            (Self::Both, Self::Both)
            | (Self::Horizontal, Self::Horizontal)
            | (Self::Vertical, Self::Vertical) => None,
            (Self::Horizontal | Self::Vertical, Self::Both)
            | (Self::Horizontal, Self::Vertical)
            | (Self::Vertical, Self::Horizontal) => Some(Self::Both),
            (Self::Both, Self::Horizontal) => Some(Self::Vertical),
            (Self::Both, Self::Vertical) => Some(Self::Horizontal),
        }
    }

    /// Returns `current` stretched to fill `usable` along these axes.
    fn fill(self, current: Area, usable: Area) -> Area {
        let mut area = current;
        if matches!(self, Self::Horizontal | Self::Both) {
            area.loc.x = usable.loc.x;
            area.size.w = usable.size.w;
        }
        if matches!(self, Self::Vertical | Self::Both) {
            area.loc.y = usable.loc.y;
            area.size.h = usable.size.h;
        }
        area
    }
}

/// A part of the usable area, in thousandths of its width and height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TileCell {
    /// The left edge.
    x: u16,
    /// The top edge.
    y: u16,
    /// The width.
    width: u16,
    /// The height.
    height: u16,
}

impl TileCell {
    /// The whole of an axis, in the thousandths cells are measured in.
    const WHOLE: u16 = 1000;

    /// Half an axis.
    const HALF: u16 = Self::WHOLE / 2;

    /// Creates the cell from `x`, `y` across a `width` × `height` part, in
    /// thousandths; `None` if it is empty or reaches outside.
    pub(crate) fn new(x: u16, y: u16, width: u16, height: u16) -> Option<Self> {
        let fits_along = |start: u16, length: u16| {
            length > 0
                && start
                    .checked_add(length)
                    .is_some_and(|end| end <= Self::WHOLE)
        };
        (fits_along(x, width) && fits_along(y, height)).then_some(Self {
            x,
            y,
            width,
            height,
        })
    }

    /// Returns the part of `usable` the cell covers. Neighboring cells
    /// share their edges exactly, with no gap or overlap from rounding.
    pub(crate) fn area_in(self, usable: Area) -> Area {
        let along = |start: i32, length: i32, thousandths: u16| {
            start + length * i32::from(thousandths) / i32::from(Self::WHOLE)
        };
        let left = along(usable.loc.x, usable.size.w, self.x);
        let top = along(usable.loc.y, usable.size.h, self.y);
        let right = along(usable.loc.x, usable.size.w, self.x + self.width);
        let bottom = along(usable.loc.y, usable.size.h, self.y + self.height);
        Rectangle::new((left, top).into(), (right - left, bottom - top).into())
    }

    /// Returns whether a window covering `area` sits on this cell of
    /// `usable`: placed within a few pixels, and sized within a character
    /// cell, since a terminal cannot be exactly half an output.
    fn is_occupied_by(self, area: Area, usable: Area) -> bool {
        /// How far, in pixels, a window may be from a cell's corner.
        const POSITION_SLACK: i32 = 2;
        /// How far, in pixels, a window's size may be from a cell's.
        const SIZE_SLACK: i32 = 48;
        let cell = self.area_in(usable);
        (area.loc.x - cell.loc.x).abs() <= POSITION_SLACK
            && (area.loc.y - cell.loc.y).abs() <= POSITION_SLACK
            && (area.size.w - cell.size.w).abs() <= SIZE_SLACK
            && (area.size.h - cell.size.h).abs() <= SIZE_SLACK
    }
}

/// The half or quarter of the snap ladder a window holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SnapCell {
    /// Which half.
    side: Side,
    /// How much of its height.
    row: Row,
}

impl SnapCell {
    /// Every snap cell.
    const ALL: [Self; 6] = [
        Self::new(Side::Left, Row::Whole),
        Self::new(Side::Right, Row::Whole),
        Self::new(Side::Left, Row::Top),
        Self::new(Side::Right, Row::Top),
        Self::new(Side::Left, Row::Bottom),
        Self::new(Side::Right, Row::Bottom),
    ];

    /// Creates the cell of `row` on `side`.
    const fn new(side: Side, row: Row) -> Self {
        Self { side, row }
    }

    /// Returns the snap cell `cell` is, if it is one.
    fn of(cell: TileCell) -> Option<Self> {
        Self::ALL.into_iter().find(|snap| snap.cell() == cell)
    }

    /// Returns the cell's part of the usable area.
    const fn cell(self) -> TileCell {
        let x = match self.side {
            Side::Left => 0,
            Side::Right => TileCell::HALF,
        };
        let (y, height) = match self.row {
            Row::Top => (0, TileCell::HALF),
            Row::Whole => (0, TileCell::WHOLE),
            Row::Bottom => (TileCell::HALF, TileCell::HALF),
        };
        TileCell {
            x,
            y,
            width: TileCell::HALF,
            height,
        }
    }
}

/// A half of the usable area, side to side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Side {
    /// The left half.
    Left,
    /// The right half.
    Right,
}

impl Side {
    /// Returns the side `direction` points to, if it points sideways.
    const fn of(direction: Direction) -> Option<Self> {
        match direction {
            Direction::Left => Some(Self::Left),
            Direction::Right => Some(Self::Right),
            Direction::Up | Direction::Down => None,
        }
    }
}

/// How much of a half's height a snap cell covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Row {
    /// The top quarter.
    Top,
    /// The whole half.
    Whole,
    /// The bottom quarter.
    Bottom,
}

impl Row {
    /// Returns the row a step up goes to; none above the top.
    const fn above(self) -> Option<Self> {
        match self {
            Self::Top => None,
            Self::Whole => Some(Self::Top),
            Self::Bottom => Some(Self::Whole),
        }
    }

    /// Returns the row a step down goes to; none below the bottom.
    const fn below(self) -> Option<Self> {
        match self {
            Self::Top => Some(Self::Whole),
            Self::Whole => Some(Self::Bottom),
            Self::Bottom => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use smithay::utils::{Point, Size};

    use super::*;

    /// The usable area of a 1920×1080 output.
    fn usable() -> Area {
        Rectangle::new((0, 0).into(), (1920, 1080).into())
    }

    /// A floating window's area, away from every cell.
    fn floating_area() -> Area {
        Rectangle::new((300, 200).into(), (640, 480).into())
    }

    /// The placement of a window snapped to `cell`, from `floating_area`.
    fn snapped(side: Side, row: Row) -> Placement {
        Placement::Tiled {
            cell: SnapCell::new(side, row).cell(),
            restore: floating_area(),
        }
    }

    /// Takes a snap step from `placement`, the window where it puts it.
    fn step(placement: Placement, direction: Direction) -> SnapStep {
        let current = placement.area(floating_area(), usable(), usable());
        placement.snap_step(direction, current, usable())
    }

    #[rstest]
    #[case::left_snaps_left(Direction::Left, Side::Left)]
    #[case::right_snaps_right(Direction::Right, Side::Right)]
    fn a_floating_window_snaps_to_a_half(#[case] direction: Direction, #[case] side: Side) {
        assert_eq!(
            step(Placement::Floating, direction),
            SnapStep::Place(snapped(side, Row::Whole))
        );
    }

    #[rstest]
    #[case::other_half_undoes(
        Side::Left,
        Row::Whole,
        Direction::Right,
        SnapStep::Place(Placement::Floating)
    )]
    #[case::same_half_stays(Side::Left, Row::Whole, Direction::Left, SnapStep::Unchanged)]
    #[case::quarter_crosses_over(
        Side::Left,
        Row::Top,
        Direction::Right,
        SnapStep::Place(snapped(Side::Right, Row::Top))
    )]
    #[case::half_up_is_top_quarter(
        Side::Right,
        Row::Whole,
        Direction::Up,
        SnapStep::Place(snapped(Side::Right, Row::Top))
    )]
    #[case::half_down_is_bottom_quarter(
        Side::Left,
        Row::Whole,
        Direction::Down,
        SnapStep::Place(snapped(Side::Left, Row::Bottom))
    )]
    #[case::top_quarter_down_is_half(
        Side::Left,
        Row::Top,
        Direction::Down,
        SnapStep::Place(snapped(Side::Left, Row::Whole))
    )]
    #[case::bottom_quarter_up_is_half(
        Side::Right,
        Row::Bottom,
        Direction::Up,
        SnapStep::Place(snapped(Side::Right, Row::Whole))
    )]
    #[case::bottom_quarter_down_minimizes(
        Side::Right,
        Row::Bottom,
        Direction::Down,
        SnapStep::Minimize
    )]
    fn snapped_windows_climb_the_ladder(
        #[case] side: Side,
        #[case] row: Row,
        #[case] direction: Direction,
        #[case] expected: SnapStep,
    ) {
        assert_eq!(step(snapped(side, row), direction), expected);
    }

    #[test]
    fn up_from_a_top_quarter_maximizes_and_down_returns_there() {
        let SnapStep::Place(maximized) = step(snapped(Side::Left, Row::Top), Direction::Up) else {
            panic!("up from a top quarter maximizes");
        };
        assert!(maximized.is_fully_maximized());
        assert_eq!(
            step(maximized, Direction::Down),
            SnapStep::Place(snapped(Side::Left, Row::Top))
        );
    }

    #[test]
    fn up_maximizes_a_floating_window_and_down_restores_it() {
        let SnapStep::Place(maximized) = step(Placement::Floating, Direction::Up) else {
            panic!("up maximizes");
        };
        assert_eq!(maximized.restore_area(usable()), floating_area());
        assert_eq!(
            step(maximized, Direction::Down),
            SnapStep::Place(Placement::Floating)
        );
        assert_eq!(
            step(Placement::Floating, Direction::Down),
            SnapStep::Minimize
        );
    }

    #[test]
    fn sideways_from_maximized_snaps_to_a_half_and_keeps_the_restore_area() {
        let maximized = Placement::Floating.with_maximized_toggled(Axes::Both, floating_area());
        assert_eq!(
            step(maximized, Direction::Left),
            SnapStep::Place(snapped(Side::Left, Row::Whole))
        );
    }

    #[test]
    fn a_window_dropped_on_a_half_counts_as_snapped_there() {
        let half = SnapCell::new(Side::Right, Row::Whole)
            .cell()
            .area_in(usable());
        let by_hand = Rectangle::new(
            half.loc + Point::from((1, 1)),
            half.size - Size::from((20, 30)),
        );
        assert_eq!(
            Placement::Floating.snap_step(Direction::Left, by_hand, usable()),
            SnapStep::Place(Placement::Floating),
            "the other way undoes the snap"
        );
    }

    #[test]
    fn neighboring_cells_share_their_edge() {
        let odd = Rectangle::new((5, 7).into(), (1279, 1023).into());
        let left = SnapCell::new(Side::Left, Row::Top).cell().area_in(odd);
        let right = SnapCell::new(Side::Right, Row::Bottom).cell().area_in(odd);
        assert_eq!(left.loc.x + left.size.w, right.loc.x);
        assert_eq!(left.loc.y + left.size.h, right.loc.y);
        assert_eq!(right.loc.x + right.size.w, odd.loc.x + odd.size.w);
        assert_eq!(right.loc.y + right.size.h, odd.loc.y + odd.size.h);
    }

    #[rstest]
    #[case::both_off_both(Axes::Both, Axes::Both, None)]
    #[case::vertical_onto_horizontal(Axes::Horizontal, Axes::Vertical, Some(Axes::Both))]
    #[case::vertical_off_both(Axes::Both, Axes::Vertical, Some(Axes::Horizontal))]
    #[case::both_onto_vertical(Axes::Vertical, Axes::Both, Some(Axes::Both))]
    fn toggling_axes(#[case] held: Axes, #[case] toggled: Axes, #[case] expected: Option<Axes>) {
        assert_eq!(held.with_toggled(toggled), expected);
    }

    #[test]
    fn leaving_fullscreen_returns_to_what_was_underneath() {
        let tiled = snapped(Side::Left, Row::Whole);
        let fullscreen = tiled.clone().with_fullscreen_toggled(floating_area());
        assert!(fullscreen.is_fullscreen());
        assert_eq!(
            fullscreen.area(floating_area(), usable(), usable()),
            usable()
        );
        assert_eq!(fullscreen.with_fullscreen_toggled(usable()), tiled);
    }

    #[test]
    fn a_cell_must_fit_the_area() {
        assert!(TileCell::new(500, 0, 500, 1000).is_some());
        assert!(TileCell::new(600, 0, 500, 1000).is_none());
        assert!(TileCell::new(0, 0, 0, 1000).is_none());
    }
}
