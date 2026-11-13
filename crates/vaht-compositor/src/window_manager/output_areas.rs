//! Where windows can be placed on each output.
//!
//! The window manager knows outputs only as areas: [`OutputAreas`] holds
//! each output's [`OutputArea`] and says which one a window or a point
//! belongs to. A window belongs to the output showing the most of it, so
//! it needs no output of its own to remember: moved across, it belongs to
//! the next; maximized, it fills the one it is on.

use smithay::utils::{Logical, Point};

use super::window::{Area, OutputArea};

/// Each output's areas, the primary output's first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutputAreas {
    /// The primary output's: where anything on no output goes.
    primary: OutputArea,
    /// The rest.
    others: Vec<OutputArea>,
}

impl OutputAreas {
    /// The areas of the primary output, `primary`, and of the `others`.
    pub(crate) const fn new(primary: OutputArea, others: Vec<OutputArea>) -> Self {
        Self { primary, others }
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
    fn a_point_belongs_to_the_output_under_it() {
        let outputs = two();
        assert_eq!(outputs.at((900, 10).into()), output(800, (640, 480)));
        assert_eq!(outputs.at((900, 590).into()), outputs.primary());
    }
}
