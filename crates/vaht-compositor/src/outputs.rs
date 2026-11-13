//! The outputs vaht shows on, and where each sits in global space.
//!
//! The backend finds the outputs and draws into them; [`Outputs`] knows
//! their layout: which are on, which one is primary, where each is, and
//! which one a point or an area belongs to. It sets them up as
//! `outputs.toml` says ([`Outputs::configure`]), asking the backend
//! ([`OutputControl`]) for what only it can do. An output's position lives
//! in its own state, which the backend reads when it draws, so the layout
//! has one owner and the backend never keeps a copy.

use smithay::output::{Output, Scale};
use smithay::utils::{Logical, Point, Rectangle, Transform};

use crate::config::{
    OutputEntry, OutputIdentity, OutputMode, OutputScale, OutputSettings, OutputTransform,
};

/// What only the backend can do to an output.
pub(crate) trait OutputControl {
    /// Turns `output` on or off: clients see only outputs that are on, and
    /// only those are drawn.
    fn set_enabled(&mut self, output: &Output, is_enabled: bool);

    /// Sets `output`'s mode to the one nearest `mode`, or its preferred one
    /// without it.
    fn set_mode(&mut self, output: &Output, mode: Option<OutputMode>);

    /// Turns `output` as `transform` says.
    fn set_transform(&mut self, output: &Output, transform: Transform);
}

/// The outputs on, the primary one first, and those off.
#[derive(Debug, Clone)]
pub(crate) struct Outputs {
    /// Where windows open and the taskbar shows by default.
    primary: Output,
    /// The rest of those on, in the order the backend found them.
    others: Vec<Output>,
    /// Every output the backend has, on or off, in the order it found
    /// them.
    connected: Vec<Output>,
}

impl Outputs {
    /// The outputs `first` and `rest`, the backend's, all on and laid out
    /// side by side from the left in that order, their tops level.
    pub(crate) fn in_a_row(first: Output, rest: Vec<Output>) -> Self {
        let connected = std::iter::once(first.clone())
            .chain(rest.iter().cloned())
            .collect();
        let outputs = Self {
            primary: first,
            others: rest,
            connected,
        };
        Self::lay_out_in_a_row(0, outputs.iter());
        outputs
    }

    /// Sets the outputs up as `settings` say, with `backend` doing what
    /// only it can: each output follows the first entry matching it. If
    /// the settings would turn every output off, the first stays on.
    pub(crate) fn configure(
        &mut self,
        settings: &OutputSettings,
        backend: &mut impl OutputControl,
    ) {
        let mut on: Vec<(Output, Option<&OutputEntry>)> = Vec::new();
        for output in &self.connected {
            let entry = settings.entry_for(Self::identity(output).as_identity());
            if entry.is_some_and(|entry| !entry.enabled) {
                backend.set_enabled(output, false);
            } else {
                on.push((output.clone(), entry));
            }
        }
        if on.is_empty() {
            tracing::warn!("outputs.toml turns every output off; the first stays on");
            let first = self
                .connected
                .first()
                .cloned()
                .unwrap_or_else(|| self.primary.clone());
            on.push((first, None));
        }
        for (output, entry) in &on {
            backend.set_enabled(output, true);
            backend.set_mode(output, entry.and_then(|entry| entry.mode));
            let transform = entry.map_or(OutputTransform::Normal, |entry| entry.transform);
            backend.set_transform(output, Self::turned(transform));
            let scale = entry
                .and_then(|entry| entry.scale)
                .map_or(1.0, OutputScale::get);
            output.change_current_state(None, None, Some(Scale::Fractional(scale)), None);
        }
        // Those placed by hand first; the rest go to their right.
        let mut right = 0;
        for (output, entry) in &on {
            if let Some([x, y]) = entry.and_then(|entry| entry.position) {
                output.change_current_state(None, None, None, Some((x, y).into()));
                let area = Self::geometry(output);
                right = right.max(area.loc.x + area.size.w);
            }
        }
        let unplaced = on
            .iter()
            .filter(|(_, entry)| entry.and_then(|entry| entry.position).is_none())
            .map(|(output, _)| output);
        Self::lay_out_in_a_row(right, unplaced);
        // Without an entry saying, Fluxbox's head 1: the leftmost, then
        // topmost, output.
        let primary_index = on
            .iter()
            .position(|(_, entry)| entry.is_some_and(|entry| entry.primary))
            .or_else(|| {
                on.iter()
                    .enumerate()
                    .min_by_key(|(_, (output, _))| {
                        let corner = output.current_location();
                        (corner.x, corner.y, output.name())
                    })
                    .map(|(index, _)| index)
            })
            .unwrap_or(0);
        let mut on: Vec<Output> = on.into_iter().map(|(output, _)| output).collect();
        self.primary = on.remove(primary_index);
        self.others = on;
        for output in self.iter() {
            tracing::info!(
                output = output.name(),
                area = ?Self::geometry(output),
                is_primary = (*output == self.primary),
                "output arranged"
            );
        }
    }

    /// Returns Smithay's transform for `transform`.
    const fn turned(transform: OutputTransform) -> Transform {
        match transform {
            OutputTransform::Normal => Transform::Normal,
            OutputTransform::Turned90 => Transform::_90,
            OutputTransform::Turned180 => Transform::_180,
            OutputTransform::Turned270 => Transform::_270,
            OutputTransform::Flipped => Transform::Flipped,
            OutputTransform::Flipped90 => Transform::Flipped90,
            OutputTransform::Flipped180 => Transform::Flipped180,
            OutputTransform::Flipped270 => Transform::Flipped270,
        }
    }

    /// Lays `outputs` out side by side from `left`, their tops at 0.
    fn lay_out_in_a_row<'output>(left: i32, outputs: impl Iterator<Item = &'output Output>) {
        let mut left = left;
        for output in outputs {
            output.change_current_state(None, None, None, Some((left, 0).into()));
            left += Self::geometry(output).size.w;
        }
    }

    /// Returns what identifies `output`, owned.
    pub(crate) fn identity(output: &Output) -> OwnedIdentity {
        let properties = output.physical_properties();
        OwnedIdentity {
            connector: output.name(),
            make: properties.make,
            model: properties.model,
            serial: properties.serial_number,
        }
    }

    /// Returns every output the backend has, on or off.
    pub(crate) fn connected(&self) -> impl Iterator<Item = &Output> {
        self.connected.iter()
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

/// What identifies an output, owned: see [`OutputIdentity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnedIdentity {
    /// The connector's name.
    pub(crate) connector: String,
    /// The display's manufacturer.
    pub(crate) make: String,
    /// The display's model.
    pub(crate) model: String,
    /// The display's serial number.
    pub(crate) serial: String,
}

impl OwnedIdentity {
    /// Returns it borrowed, as a matcher takes it.
    pub(crate) fn as_identity(&self) -> OutputIdentity<'_> {
        OutputIdentity {
            connector: &self.connector,
            make: &self.make,
            model: &self.model,
            serial: &self.serial,
        }
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

    /// Records what [`Outputs::configure`] asks of the backend, and sets
    /// modes as asked.
    #[derive(Default)]
    struct FakeBackend {
        /// Each output turned off, by name.
        off: Vec<String>,
    }

    impl OutputControl for FakeBackend {
        fn set_enabled(&mut self, output: &Output, is_enabled: bool) {
            self.off.retain(|name| *name != output.name());
            if !is_enabled {
                self.off.push(output.name());
            }
        }

        fn set_transform(&mut self, output: &Output, transform: Transform) {
            output.change_current_state(None, Some(transform), None, None);
        }

        fn set_mode(&mut self, output: &Output, mode: Option<OutputMode>) {
            if let Some(mode) = mode {
                output.change_current_state(
                    Some(Mode {
                        size: (mode.width, mode.height).into(),
                        refresh: 60_000,
                    }),
                    None,
                    None,
                    None,
                );
            }
        }
    }

    /// Three outputs, set up by `settings`; what the backend was asked.
    fn configured(settings: &str) -> (Outputs, FakeBackend) {
        let mut outputs = Outputs::in_a_row(
            output("ONE", 1920, 1080),
            vec![output("TWO", 1280, 720), output("THREE", 800, 600)],
        );
        let mut backend = FakeBackend::default();
        let settings: OutputSettings = toml::from_str(settings).expect("valid");
        outputs.configure(&settings, &mut backend);
        (outputs, backend)
    }

    #[test]
    fn outputs_go_where_placed_and_the_rest_to_their_right() {
        let (outputs, backend) = configured(
            r#"
[[output]]
match = { connector = "TWO" }
position = [0, 0]
primary = true

[[output]]
match = { connector = "THREE" }
enabled = false
"#,
        );
        assert_eq!(backend.off, ["THREE"]);
        assert_eq!(outputs.primary().name(), "TWO");
        let areas: Vec<_> = outputs
            .iter()
            .map(|output| (output.name(), Outputs::geometry(output)))
            .collect();
        assert_eq!(
            areas,
            [
                (
                    "TWO".to_owned(),
                    Rectangle::new((0, 0).into(), (1280, 720).into())
                ),
                (
                    "ONE".to_owned(),
                    Rectangle::new((1280, 0).into(), (1920, 1080).into())
                ),
            ]
        );
    }

    #[test]
    fn modes_and_scales_change_the_layout() {
        let (outputs, _) = configured(
            r#"
[[output]]
match = { connector = "ONE" }
mode = "2560x1440"
scale = 2.0
"#,
        );
        let areas: Vec<_> = outputs.iter().map(Outputs::geometry).collect();
        assert_eq!(areas[0], Rectangle::new((0, 0).into(), (1280, 720).into()));
        assert_eq!(areas[1].loc, Point::from((1280, 0)));
    }

    #[test]
    fn a_turned_output_is_as_tall_as_it_was_wide() {
        let (outputs, _) = configured(
            r#"
[[output]]
match = { connector = "TWO" }
transform = "90"
"#,
        );
        let areas: Vec<_> = outputs.iter().map(Outputs::geometry).collect();
        assert_eq!(
            areas[1],
            Rectangle::new((1920, 0).into(), (720, 1280).into())
        );
    }

    #[test]
    fn the_leftmost_output_is_primary_unless_one_is_named() {
        let (outputs, _) = configured(
            r#"
[[output]]
match = { connector = "THREE" }
position = [0, 0]
"#,
        );
        assert_eq!(outputs.primary().name(), "THREE");
    }

    #[test]
    fn turning_every_output_off_keeps_the_first() {
        let (outputs, backend) = configured(
            r#"
[[output]]
match = { make = "test" }
enabled = false
"#,
        );
        assert_eq!(outputs.primary().name(), "ONE");
        assert_eq!(outputs.others().count(), 0);
        assert_eq!(backend.off, ["TWO", "THREE"]);
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
