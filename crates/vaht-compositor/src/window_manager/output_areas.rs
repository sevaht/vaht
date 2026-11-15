//! Where windows can be placed on each output.
//!
//! The window manager knows outputs only as areas: [`OutputAreas`] holds
//! each output's [`OutputArea`] and says which one a window or a point
//! belongs to. A window belongs to the output showing the most of it, so
//! it needs no output of its own to remember: moved across, it belongs to
//! the next; maximized, it fills the one it is on. Outputs are also
//! numbered as heads, from 1, left to right and then top to bottom, as
//! Fluxbox and fluxland number them, for `send-to-output`.

use std::num::NonZeroUsize;

use smithay::utils::{Logical, Point};

use super::window::{Area, OutputArea};

/// Which output `send-to-output` sends a window to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTarget {
    /// The next head, after the last the first.
    Next,
    /// The head before, before the first the last.
    Previous,
    /// The head with this number, from 1.
    Head(NonZeroUsize),
}

/// Each output's areas, the primary output's first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutputAreas {
    /// The primary output's: where anything on no output goes.
    primary: OutputArea,
    /// The rest.
    others: Vec<OutputArea>,
    /// Each output's connector name, the primary output's first, when
    /// known.
    names: Vec<String>,
}

impl OutputAreas {
    /// The areas of the primary output, `primary`, and of the `others`.
    pub(crate) const fn new(primary: OutputArea, others: Vec<OutputArea>) -> Self {
        Self {
            primary,
            others,
            names: Vec::new(),
        }
    }

    /// Names the outputs: `names` are their connectors', the primary
    /// output's first.
    #[must_use]
    pub(crate) fn named(mut self, names: Vec<String>) -> Self {
        self.names = names;
        self
    }

    /// Returns the areas of the output on the connector `name`, if there
    /// is one.
    pub(crate) fn by_name(&self, name: &str) -> Option<OutputArea> {
        self.names
            .iter()
            .position(|known| known == name)
            .and_then(|index| self.iter().nth(index))
            .copied()
    }

    /// Returns the areas of head `number`, from 1 left to right and then
    /// top to bottom, if there is one.
    pub(crate) fn head(&self, number: NonZeroUsize) -> Option<OutputArea> {
        self.heads().get(number.get() - 1).copied()
    }

    /// Returns the primary output's areas.
    #[cfg(test)]
    pub(crate) const fn primary(&self) -> OutputArea {
        self.primary
    }

    /// Returns every output's areas, the primary output's first.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &OutputArea> {
        std::iter::once(&self.primary).chain(&self.others)
    }

    /// Returns the areas of the output showing the most of `area`, or the
    /// primary output's if none shows any.
    pub(crate) fn for_area(&self, area: Area) -> OutputArea {
        self.iter()
            .filter_map(|output| {
                let shown = output.whole.intersection(area)?;
                Some((output, shown.size.w * shown.size.h))
            })
            .max_by_key(|(_, shown)| *shown)
            .map_or(self.primary, |(output, _)| *output)
    }

    /// Returns where `area`'s corner goes so that an output shows it: where
    /// it is if one does, else the nearest place in the primary output's
    /// usable area, as when its own output is gone.
    pub(crate) fn corner_shown(&self, area: Area) -> Point<i32, Logical> {
        if self
            .iter()
            .any(|output| output.whole.intersection(area).is_some())
        {
            return area.loc;
        }
        let usable = self.primary.usable;
        let within = |start: i32, length: i32, room_start: i32, room_length: i32| {
            start.clamp(
                room_start,
                room_start.max(room_start + room_length - length),
            )
        };
        Point::from((
            within(area.loc.x, area.size.w, usable.loc.x, usable.size.w),
            within(area.loc.y, area.size.h, usable.loc.y, usable.size.h),
        ))
    }

    /// Returns the outputs as heads: left to right, then top to bottom.
    fn heads(&self) -> Vec<OutputArea> {
        let mut heads: Vec<OutputArea> = self.iter().copied().collect();
        heads.sort_by_key(|output| (output.whole.loc.x, output.whole.loc.y));
        heads
    }

    /// Returns the output `target` names, from the output `from`; none if
    /// there is no such head.
    pub(crate) fn target(&self, from: OutputArea, target: OutputTarget) -> Option<OutputArea> {
        let heads = self.heads();
        let count = heads.len();
        let index = heads.iter().position(|head| *head == from).unwrap_or(0);
        let chosen = match target {
            OutputTarget::Next => (index + 1) % count,
            OutputTarget::Previous => (index + count - 1) % count,
            OutputTarget::Head(number) => number.get() - 1,
        };
        heads.get(chosen).copied()
    }

    /// Returns where `area`'s corner goes, carried from the output `from`
    /// to `to`: as far into `to`'s usable area as it was into `from`'s,
    /// in proportion, and wholly within it where it fits.
    pub(crate) fn carried(area: Area, from: OutputArea, to: OutputArea) -> Point<i32, Logical> {
        let along = |offset: i32, from_length: i32, length: i32, to_start: i32, to_length: i32| {
            let room_from = i64::from((from_length - length).max(1));
            let room_to = i64::from((to_length - length).max(0));
            let offset = i64::from(offset.clamp(0, from_length - length.min(from_length)));
            let moved = i32::try_from(offset * room_to / room_from).unwrap_or_default();
            to_start + moved.clamp(0, room_to.try_into().unwrap_or_default())
        };
        let (from_usable, to_usable) = (from.usable, to.usable);
        Point::from((
            along(
                area.loc.x - from_usable.loc.x,
                from_usable.size.w,
                area.size.w,
                to_usable.loc.x,
                to_usable.size.w,
            ),
            along(
                area.loc.y - from_usable.loc.y,
                from_usable.size.h,
                area.size.h,
                to_usable.loc.y,
                to_usable.size.h,
            ),
        ))
    }

    /// Returns the areas of the output `point` is on, or the primary
    /// output's if it is on none.
    pub(crate) fn at(&self, point: Point<i32, Logical>) -> OutputArea {
        self.iter()
            .find(|output| output.whole.contains(point))
            .copied()
            .unwrap_or(self.primary)
    }
}

#[cfg(test)]
mod tests {
    use smithay::utils::Rectangle;

    use super::*;

    /// An output at `x` with `size`, its usable area all of it.
    fn output(x: i32, size: (i32, i32)) -> OutputArea {
        let whole = Rectangle::new((x, 0).into(), size.into());
        OutputArea {
            whole,
            usable: whole,
        }
    }

    /// Two outputs side by side.
    fn two() -> OutputAreas {
        OutputAreas::new(output(0, (800, 600)), vec![output(800, (640, 480))])
    }

    #[test]
    fn an_area_belongs_to_the_output_showing_most_of_it() {
        let outputs = two();
        let mostly_second = Rectangle::new((700, 10).into(), (300, 100).into());
        assert_eq!(outputs.for_area(mostly_second), output(800, (640, 480)));
        let nowhere = Rectangle::new((5000, 0).into(), (10, 10).into());
        assert_eq!(outputs.for_area(nowhere), outputs.primary());
    }

    #[test]
    fn an_area_no_output_shows_moves_onto_the_primary_one() {
        let outputs = two();
        let shown = Rectangle::new((900, 10).into(), (100, 100).into());
        assert_eq!(outputs.corner_shown(shown), shown.loc);
        let lost = Rectangle::new((3000, 700).into(), (100, 100).into());
        assert_eq!(outputs.corner_shown(lost), Point::from((700, 500)));
    }

    #[test]
    fn heads_go_left_to_right_and_wrap() {
        let left = output(0, (800, 600));
        let right = output(800, (640, 480));
        // The primary output is the right one here.
        let outputs = OutputAreas::new(right, vec![left]);
        assert_eq!(outputs.target(left, OutputTarget::Next), Some(right));
        assert_eq!(
            outputs.target(right, OutputTarget::Next),
            Some(left),
            "wraps"
        );
        assert_eq!(outputs.target(left, OutputTarget::Previous), Some(right));
        let head = |number| OutputTarget::Head(NonZeroUsize::new(number).expect("not 0"));
        assert_eq!(outputs.target(right, head(1)), Some(left));
        assert_eq!(outputs.target(right, head(3)), None);
    }

    #[test]
    fn a_window_carried_across_keeps_its_place_in_proportion() {
        let (from, to) = (output(0, (800, 600)), output(800, (640, 480)));
        let centered = Rectangle::new((300, 250).into(), (200, 100).into());
        assert_eq!(
            OutputAreas::carried(centered, from, to),
            Point::from((800 + 220, 190)),
            "still centred"
        );
        let too_wide = Rectangle::new((0, 0).into(), (700, 100).into());
        assert_eq!(
            OutputAreas::carried(too_wide, from, to).x,
            800,
            "against the left edge when it does not fit"
        );
    }

    #[test]
    fn a_point_belongs_to_the_output_under_it() {
        let outputs = two();
        assert_eq!(outputs.at((900, 10).into()), output(800, (640, 480)));
        assert_eq!(outputs.at((900, 590).into()), outputs.primary());
    }
}
