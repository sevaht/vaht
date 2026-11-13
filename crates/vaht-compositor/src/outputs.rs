//! The outputs vaht shows on, and where each sits in global space.
//!
//! The backend makes the outputs and draws into them; [`Outputs`] knows
//! their layout: which one is primary, where each is, and which one a
//! point or an area belongs to. An output's position lives in its own
//! state, which the backend reads when it draws, so the layout has one
//! owner and the backend never keeps a copy.

use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle};

/// The outputs, the primary one first.
#[derive(Debug, Clone)]
pub(crate) struct Outputs {
    /// Where windows open and the taskbar shows until vaht arranges them
    /// per output.
    primary: Output,
    /// The rest, in the order the backend found them.
    others: Vec<Output>,
}

impl Outputs {
    /// The outputs `primary` and `others`, laid out side by side from the
    /// left in that order, their tops level.
    pub(crate) fn in_a_row(primary: Output, others: Vec<Output>) -> Self {
        let outputs = Self { primary, others };
        let mut left = 0;
        for output in outputs.iter() {
            output.change_current_state(None, None, None, Some((left, 0).into()));
            left += Self::geometry(output).size.w;
        }
        outputs
    }

    /// Returns the primary output.
    pub(crate) const fn primary(&self) -> &Output {
        &self.primary
    }

    /// Returns the primary output's area in global coordinates.
    pub(crate) fn primary_geometry(&self) -> Rectangle<i32, Logical> {
        Self::geometry(&self.primary)
    }

    /// Returns the output named `name`, such as `HDMI-A-1`, if there is
    /// one.
    pub(crate) fn named(&self, name: &str) -> Option<&Output> {
        self.iter().find(|output| output.name() == name)
    }

    /// Returns every output but the primary one.
    pub(crate) fn others(&self) -> impl Iterator<Item = &Output> {
        self.others.iter()
    }

    /// Returns every output, the primary one first.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Output> {
        std::iter::once(&self.primary).chain(&self.others)
    }

    /// Returns the smallest area holding every output.
    pub(crate) fn bounds(&self) -> Rectangle<i32, Logical> {
        self.iter()
            .map(Self::geometry)
            .fold(self.primary_geometry(), Rectangle::merge)
    }

    /// Returns `output`'s area in global coordinates: its position, and its
    /// mode's size as its transform turns it and its scale shrinks it.
    pub(crate) fn geometry(output: &Output) -> Rectangle<i32, Logical> {
        let size = output.current_mode().map_or_else(Default::default, |mode| {
            output
                .current_transform()
                .transform_size(mode.size)
                .to_f64()
                .to_logical(output.current_scale().fractional_scale())
                .to_i32_round()
        });
        Rectangle::new(output.current_location(), size)
    }

    /// Returns the output that shows the most of `area`, if any shows it.
    pub(crate) fn most_overlapping(&self, area: Rectangle<i32, Logical>) -> Option<&Output> {
        self.iter()
            .filter_map(|output| {
                let overlap = Self::geometry(output).intersection(area)?;
                Some((output, overlap.size.w * overlap.size.h))
            })
            .max_by_key(|(_, shown)| *shown)
            .map(|(output, _)| output)
    }

    /// Returns the output `point` is on, if it is on one.
    pub(crate) fn at(&self, point: Point<f64, Logical>) -> Option<&Output> {
        self.iter()
            .find(|output| Self::geometry(output).to_f64().contains(point))
    }

    /// Returns where the pointer goes when moved from `from` toward `to`:
    /// `to` itself on an output, else the nearest point of the output it
    /// leaves (or the primary one), so it stops at an edge with nothing
    /// past it.
    pub(crate) fn clamp(
        &self,
        from: Point<f64, Logical>,
        to: Point<f64, Logical>,
    ) -> Point<f64, Logical> {
        if self.at(to).is_some() {
            return to;
        }
        let area = Self::geometry(self.at(from).unwrap_or(&self.primary)).to_f64();
        // The far edges are outside the output, so stop just short.
        let right = (area.loc.x + area.size.w - 1.0).max(area.loc.x);
        let bottom = (area.loc.y + area.size.h - 1.0).max(area.loc.y);
        Point::from((
            to.x.clamp(area.loc.x, right),
            to.y.clamp(area.loc.y, bottom),
        ))
    }
}

#[cfg(test)]
mod tests {
    use smithay::output::{Mode, PhysicalProperties, Subpixel};

    use super::*;

    /// An output of `width` by `height` pixels named `name`.
    fn output(name: &str, width: i32, height: i32) -> Output {
        let output = Output::new(
            name.to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".to_owned(),
                model: "test".to_owned(),
                serial_number: String::new(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (width, height).into(),
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        output
    }

    /// Two outputs side by side, the second shorter.
    fn two() -> Outputs {
        Outputs::in_a_row(output("ONE", 1920, 1080), vec![output("TWO", 1280, 720)])
    }

    #[test]
    fn outputs_sit_side_by_side() {
        let outputs = two();
        let areas: Vec<_> = outputs.iter().map(Outputs::geometry).collect();
        assert_eq!(
            areas,
            [
                Rectangle::new((0, 0).into(), (1920, 1080).into()),
                Rectangle::new((1920, 0).into(), (1280, 720).into()),
            ]
        );
    }

    #[test]
    fn the_bounds_hold_every_output() {
        assert_eq!(
            two().bounds(),
            Rectangle::new((0, 0).into(), (3200, 1080).into())
        );
    }

    #[test]
    fn an_area_belongs_to_the_output_showing_most_of_it() {
        let outputs = two();
        let mostly_second = Rectangle::new((1800, 0).into(), (400, 100).into());
        assert_eq!(
            outputs.most_overlapping(mostly_second).map(Output::name),
            Some("TWO".to_owned())
        );
        let nowhere = Rectangle::new((5000, 0).into(), (10, 10).into());
        assert!(outputs.most_overlapping(nowhere).is_none());
    }

    #[test]
    fn the_pointer_crosses_to_a_neighbor_but_stops_where_nothing_is() {
        let outputs = two();
        let crossed = outputs.clamp((1910.0, 10.0).into(), (1930.0, 10.0).into());
        assert_eq!(crossed, Point::from((1930.0, 10.0)));
        // Below the shorter output there is nothing: it stops at its edge.
        let stopped = outputs.clamp((2000.0, 700.0).into(), (2000.0, 900.0).into());
        assert_eq!(stopped, Point::from((2000.0, 719.0)));
        let left = outputs.clamp((5.0, 5.0).into(), (-20.0, 5.0).into());
        assert_eq!(left, Point::from((0.0, 5.0)));
    }
}
