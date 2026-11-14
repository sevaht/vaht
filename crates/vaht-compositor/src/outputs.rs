//! The outputs vaht shows on, and where each sits in global space.
//!
//! The backend finds the outputs and draws into them; [`Outputs`] knows
//! their layout: which are on, which one is primary, where each is, and
//! which one a point or an area belongs to. It sets them up as
//! `outputs.toml` says, profiles included ([`Outputs::configure`],
//! [`Outputs::apply_profile`]), asking the backend ([`OutputControl`]) for
//! what only it can do. An output's position lives in its own state,
//! which the backend reads when it draws, so the layout has one owner and
//! the backend never keeps a copy.

mod arrangement;

use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Transform};

use self::arrangement::Arrangement;
use crate::config::{OutputIdentity, OutputMode, OutputPlan, OutputProfile, OutputSettings, Vrr};

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

/// What changed when the outputs were set up again, for the hooks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct OutputChanges {
    /// The profile just applied, if one was.
    pub(crate) applied_profile: Option<String>,
    /// The new primary output's connector, if it changed.
    pub(crate) primary: Option<String>,
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
    /// The name of the profile applied, if one is.
    profile: Option<String>,
    /// The connector `cycle-primary-output` made primary, which stays so
    /// while it is on.
    chosen_primary: Option<String>,
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
            profile: None,
            chosen_primary: None,
        };
        Self::lay_out_in_a_row(0, outputs.iter());
        outputs
    }

    /// No output yet: a placeholder no client sees holds the layout until
    /// a monitor is plugged in.
    pub(crate) fn waiting() -> Self {
        let placeholder = Output::new(
            "NONE".to_owned(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "vaht".to_owned(),
                model: "placeholder".to_owned(),
                serial_number: String::new(),
            },
        );
        placeholder.change_current_state(
            Some(smithay::output::Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            None,
            Some((0, 0).into()),
        );
        Self {
            primary: placeholder,
            others: Vec::new(),
            connected: Vec::new(),
            profile: None,
            chosen_primary: None,
        }
    }

    /// Returns the `vrr` setting `output` follows now: its entry's, under
    /// the profile applied.
    pub(crate) fn vrr_of(&self, output: &Output, settings: &OutputSettings) -> Vrr {
        let profile = self
            .profile
            .as_deref()
            .and_then(|name| settings.profile(name));
        let identity = Self::identity(output);
        match settings.plan(profile, identity.as_identity()) {
            OutputPlan::Follows(entry) => entry.vrr,
            OutputPlan::Unlisted | OutputPlan::Off => Vrr::default(),
        }
    }

    /// Returns the name of the profile applied, if one is.
    pub(crate) fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// Sets the outputs up as `settings` say, with `backend` doing what
    /// only it can. The profile applied stays while its outputs are all
    /// connected; otherwise the one the connected outputs call for applies
    /// itself, if any does.
    pub(crate) fn configure(
        &mut self,
        settings: &OutputSettings,
        backend: &mut impl OutputControl,
    ) -> OutputChanges {
        let identities = self.connected_identities();
        let connected: Vec<_> = identities.iter().map(OwnedIdentity::as_identity).collect();
        let kept = self
            .profile
            .as_deref()
            .and_then(|name| settings.profile(name))
            .filter(|profile| profile.fits(&connected));
        let profile = kept.or_else(|| settings.automatic_profile(&connected));
        let is_new = profile.map(OutputProfile::name) != self.profile.as_deref();
        self.arrange(settings, profile, backend, is_new)
    }

    /// Adds `output`, which the backend just found (a monitor plugged in
    /// or turned on), and sets every output up again.
    pub(crate) fn connect(
        &mut self,
        output: Output,
        settings: &OutputSettings,
        backend: &mut impl OutputControl,
    ) -> OutputChanges {
        if !self.connected.contains(&output) {
            tracing::info!(output = output.name(), "output connected");
            self.connected.push(output);
        }
        self.configure(settings, backend)
    }

    /// Removes `output`, which the backend lost (a monitor unplugged), and
    /// sets the rest up again. While no output is connected the layout
    /// stays as it was, showing nothing, so windows keep their places
    /// until one comes.
    pub(crate) fn disconnect(
        &mut self,
        output: &Output,
        settings: &OutputSettings,
        backend: &mut impl OutputControl,
    ) -> OutputChanges {
        tracing::info!(output = output.name(), "output disconnected");
        self.connected.retain(|connected| connected != output);
        if self.connected.is_empty() {
            tracing::warn!("no output is connected; nothing shows until one is");
            self.others.clear();
            return OutputChanges::default();
        }
        self.configure(settings, backend)
    }

    /// Sets the outputs up exactly as `settings` say, no profile applying:
    /// as a program configuring outputs asked.
    pub(crate) fn arrange_exactly(
        &mut self,
        settings: &OutputSettings,
        backend: &mut impl OutputControl,
    ) -> OutputChanges {
        self.arrange(settings, None, backend, false)
    }

    /// Applies the profile called `name`.
    ///
    /// # Errors
    ///
    /// Returns why not, for a person: there is no such profile, or an
    /// output it lists is not connected.
    pub(crate) fn apply_profile(
        &mut self,
        name: &str,
        settings: &OutputSettings,
        backend: &mut impl OutputControl,
    ) -> Result<OutputChanges, String> {
        let profile = settings
            .profile(name)
            .ok_or_else(|| format!("outputs.toml has no profile called {name}"))?;
        let identities = self.connected_identities();
        let connected: Vec<_> = identities.iter().map(OwnedIdentity::as_identity).collect();
        if !profile.fits(&connected) {
            return Err(format!(
                "the profile {name} lists an output that is not connected"
            ));
        }
        Ok(self.arrange(settings, Some(profile), backend, true))
    }

    /// Applies the next profile after the one applied whose outputs are
    /// all connected, if there is one.
    pub(crate) fn cycle_profile(
        &mut self,
        settings: &OutputSettings,
        backend: &mut impl OutputControl,
    ) -> Option<OutputChanges> {
        let identities = self.connected_identities();
        let connected: Vec<_> = identities.iter().map(OwnedIdentity::as_identity).collect();
        let next = settings.profile_after(self.profile.as_deref(), &connected)?;
        Some(self.arrange(settings, Some(next), backend, true))
    }

    /// Makes the next output primary: among the applied profile's primary
    /// candidates if it names any, else among the outputs on, left to
    /// right. None if no other output can be.
    pub(crate) fn cycle_primary(&mut self, settings: &OutputSettings) -> Option<OutputChanges> {
        let mut on: Vec<&Output> = self.iter().collect();
        on.sort_by_key(|output| Self::head_order(output));
        let identities: Vec<_> = on.iter().map(|output| Self::identity(output)).collect();
        let borrowed: Vec<_> = identities.iter().map(OwnedIdentity::as_identity).collect();
        let named: Vec<String> = self
            .profile
            .as_deref()
            .and_then(|name| settings.profile(name))
            .map(|profile| {
                profile
                    .primary_candidates(&borrowed)
                    .iter()
                    .map(|identity| identity.connector.to_owned())
                    .collect()
            })
            .unwrap_or_default();
        let candidates = if named.is_empty() {
            on.iter().map(|output| output.name()).collect()
        } else {
            named
        };
        let current = self.primary.name();
        let next = candidates
            .iter()
            .position(|name| *name == current)
            .map_or(0, |index| index + 1);
        let chosen = candidates.get(next % candidates.len().max(1))?.clone();
        self.choose_primary(&chosen)
    }

    /// Makes the output on `connector` primary, if it is on and is not
    /// already; it stays so while it is on.
    pub(crate) fn choose_primary(&mut self, connector: &str) -> Option<OutputChanges> {
        if self.primary.name() == connector {
            return None;
        }
        let mut on: Vec<Output> = self.iter().cloned().collect();
        on.sort_by_key(|output| self.found_index(output));
        let index = on.iter().position(|output| output.name() == connector)?;
        self.primary = on.remove(index);
        self.others = on;
        self.chosen_primary = Some(connector.to_owned());
        tracing::info!(output = connector, "primary output chosen");
        Some(OutputChanges {
            applied_profile: None,
            primary: Some(connector.to_owned()),
        })
    }

    /// Sets the outputs up as `settings` say with `profile` applied, or
    /// none; `is_announced` says the profile counts as just applied.
    fn arrange(
        &mut self,
        settings: &OutputSettings,
        profile: Option<&OutputProfile>,
        backend: &mut impl OutputControl,
        is_announced: bool,
    ) -> OutputChanges {
        let before = self.primary.name();
        let arrangement = Arrangement::plan(&self.connected, settings, profile);
        arrangement.apply(backend);
        let (primary, others) = arrangement.into_layout(self.chosen_primary.as_deref(), profile);
        self.primary = primary;
        self.others = others;
        self.profile = profile.map(|profile| profile.name().to_owned());
        for output in self.iter() {
            tracing::info!(
                output = output.name(),
                area = ?Self::geometry(output),
                is_primary = (*output == self.primary),
                profile = self.profile.as_deref(),
                "output arranged"
            );
        }
        let primary = self.primary.name();
        OutputChanges {
            applied_profile: self.profile.clone().filter(|_| is_announced),
            primary: (primary != before).then_some(primary),
        }
    }

    /// Returns what identifies each connected output.
    fn connected_identities(&self) -> Vec<OwnedIdentity> {
        self.connected.iter().map(Self::identity).collect()
    }

    /// Returns where `output` is in the order the backend found them.
    fn found_index(&self, output: &Output) -> usize {
        self.connected
            .iter()
            .position(|found| found == output)
            .unwrap_or(usize::MAX)
    }

    /// Returns what orders `output` among Fluxbox's heads: left to right,
    /// then top to bottom, then by connector.
    fn head_order(output: &Output) -> (i32, i32, String) {
        let corner = output.current_location();
        (corner.x, corner.y, output.name())
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
    fn a_monitor_plugged_in_goes_to_the_right() {
        let (mut outputs, mut backend) = configured("");
        let changes = outputs.connect(
            output("FOUR", 640, 480),
            &OutputSettings::default(),
            &mut backend,
        );
        assert_eq!(changes, OutputChanges::default());
        let fourth = outputs.named("FOUR").expect("connected and on");
        assert_eq!(
            Outputs::geometry(fourth),
            Rectangle::new((4000, 0).into(), (640, 480).into())
        );
    }

    #[test]
    fn unplugging_the_primary_monitor_makes_the_next_primary() {
        let (mut outputs, mut backend) = configured("");
        let one = outputs.named("ONE").cloned().expect("on");
        let changes = outputs.disconnect(&one, &OutputSettings::default(), &mut backend);
        assert_eq!(changes.primary.as_deref(), Some("TWO"));
        assert!(outputs.connected().all(|connected| *connected != one));
        let areas: Vec<_> = outputs.iter().map(Outputs::geometry).collect();
        assert_eq!(
            areas,
            [
                Rectangle::new((0, 0).into(), (1280, 720).into()),
                Rectangle::new((1280, 0).into(), (800, 600).into()),
            ],
            "the rest close up from the left"
        );
    }

    #[test]
    fn with_every_monitor_unplugged_the_layout_waits() {
        let (mut outputs, mut backend) = configured("");
        let settings = OutputSettings::default();
        for name in ["TWO", "THREE", "ONE"] {
            let output = outputs
                .connected()
                .find(|output| output.name() == name)
                .cloned()
                .expect("connected");
            let _ = outputs.disconnect(&output, &settings, &mut backend);
        }
        assert_eq!(outputs.primary().name(), "ONE", "kept until another comes");
        assert_eq!(outputs.others().count(), 0);
        let changes = outputs.connect(output("NEW", 1024, 768), &settings, &mut backend);
        assert_eq!(changes.primary.as_deref(), Some("NEW"));
    }

    #[test]
    fn starting_without_a_monitor_waits_for_one() {
        let mut outputs = Outputs::waiting();
        assert_eq!(outputs.connected().count(), 0);
        let mut backend = FakeBackend::default();
        let changes = outputs.connect(
            output("FIRST", 1280, 720),
            &OutputSettings::default(),
            &mut backend,
        );
        assert_eq!(changes.primary.as_deref(), Some("FIRST"));
        assert_eq!(outputs.iter().count(), 1, "the placeholder is gone");
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
