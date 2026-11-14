//! `outputs.toml`: how each output is set up (02 §9).
//!
//! Each `[[output]]` entry matches outputs by connector, or by make, model
//! and serial, and says what mode, position and scale they take, whether
//! they are on, and whether one is primary. The first entry that matches
//! an output is the one it follows; an output no entry matches keeps its
//! preferred mode and goes to the right of the others. A profile
//! ([`OutputProfile`]), while applied, takes the entries' place.

mod profile;

use std::fmt;

use serde::de::{self, Deserializer};
use smithay::utils::Transform;

pub(crate) use self::profile::{OutputHooks, OutputProfile};

/// `outputs.toml`.
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
#[serde(try_from = "RawOutputSettings")]
pub(crate) struct OutputSettings {
    /// The entries, in the order written.
    entries: Vec<OutputEntry>,
    /// The profiles, in the order written; no two share a name.
    profiles: Vec<OutputProfile>,
    /// The commands run when outputs change.
    pub(crate) hooks: OutputHooks,
}

/// [`OutputSettings`] as written, before the profiles' names are checked.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawOutputSettings {
    /// See [`OutputSettings::entries`].
    #[serde(rename = "output")]
    entries: Vec<OutputEntry>,
    /// See [`OutputSettings::profiles`].
    #[serde(rename = "profile")]
    profiles: Vec<OutputProfile>,
    /// See [`OutputSettings::hooks`].
    hooks: OutputHooks,
}

/// What an output does under the settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum OutputPlan<'settings> {
    /// It follows this entry.
    Follows(&'settings OutputEntry),
    /// No entry is for it: it is on, as it prefers.
    Unlisted,
    /// The profile applied leaves it out, so it is off.
    Off,
}

/// One `[[output]]` entry.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OutputEntry {
    /// Which outputs it is for.
    #[serde(rename = "match")]
    matcher: OutputMatcher,
    /// The mode to set; the output's preferred one without it.
    #[serde(default)]
    pub(crate) mode: Option<OutputMode>,
    /// Where its top left corner goes in the layout.
    #[serde(default)]
    pub(crate) position: Option<[i32; 2]>,
    /// How much larger everything is drawn on it.
    #[serde(default)]
    pub(crate) scale: Option<OutputScale>,
    /// How it is turned: for a display on its side or upside down.
    #[serde(default)]
    pub(crate) transform: OutputTransform,
    /// Whether it is on.
    #[serde(default = "OutputEntry::enabled_by_default")]
    pub(crate) enabled: bool,
    /// Whether it is the primary output: where the taskbar shows by
    /// default, and windows go when their output does.
    #[serde(default)]
    pub(crate) primary: bool,
    /// When its refresh rate follows what is shown; only a real display
    /// has one to vary.
    #[serde(default)]
    pub(crate) vrr: Vrr,
}

/// Which outputs an entry is for: every field given must be the output's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "RawOutputMatcher")]
pub(crate) struct OutputMatcher {
    /// The connector's name, such as `HDMI-A-1`.
    connector: Option<String>,
    /// The display's manufacturer, as its EDID gives it.
    make: Option<String>,
    /// The display's model.
    model: Option<String>,
    /// The display's serial number.
    serial: Option<String>,
}

/// An [`OutputMatcher`] as written, before it is checked to say something.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOutputMatcher {
    /// See [`OutputMatcher::connector`].
    connector: Option<String>,
    /// See [`OutputMatcher::make`].
    make: Option<String>,
    /// See [`OutputMatcher::model`].
    model: Option<String>,
    /// See [`OutputMatcher::serial`].
    serial: Option<String>,
}

/// What identifies an output: its connector, and the display's make,
/// model and serial number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputIdentity<'output> {
    /// The connector's name.
    pub(crate) connector: &'output str,
    /// The display's manufacturer.
    pub(crate) make: &'output str,
    /// The display's model.
    pub(crate) model: &'output str,
    /// The display's serial number.
    pub(crate) serial: &'output str,
}

/// A mode: `1920x1080`, or with a refresh rate, `1920x1080@60` or
/// `2560x1440@59.951`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputMode {
    /// Its width, in pixels.
    pub(crate) width: i32,
    /// Its height, in pixels.
    pub(crate) height: i32,
    /// Its refresh rate in millihertz; the highest one available without
    /// it.
    pub(crate) refresh_millihertz: Option<i32>,
}

/// How much larger everything is drawn on an output: 0.25 to 8.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OutputScale(f64);

/// How an output is turned, clockwise, and whether it is mirrored first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
pub(crate) enum OutputTransform {
    /// As it is.
    #[default]
    #[serde(rename = "normal")]
    Normal,
    /// A quarter turn.
    #[serde(rename = "90")]
    Turned90,
    /// Upside down.
    #[serde(rename = "180")]
    Turned180,
    /// Three quarters of a turn.
    #[serde(rename = "270")]
    Turned270,
    /// Mirrored.
    #[serde(rename = "flipped")]
    Flipped,
    /// Mirrored, then a quarter turn.
    #[serde(rename = "flipped-90")]
    Flipped90,
    /// Mirrored, then upside down.
    #[serde(rename = "flipped-180")]
    Flipped180,
    /// Mirrored, then three quarters of a turn.
    #[serde(rename = "flipped-270")]
    Flipped270,
}

/// When an output's refresh rate follows what is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Vrr {
    /// Never.
    Off,
    /// Always.
    On,
    /// While a fullscreen window is shown: games and video.
    #[default]
    Fullscreen,
}

impl OutputSettings {
    /// Returns settings of `entries` alone, with these settings' hooks: an
    /// arrangement asked for outright, as a program configuring outputs
    /// asks.
    pub(crate) fn exactly(&self, entries: Vec<OutputEntry>) -> Self {
        Self {
            entries,
            profiles: Vec::new(),
            hooks: self.hooks.clone(),
        }
    }

    /// Returns the entry `output` follows: the first that matches it.
    pub(crate) fn entry_for(&self, output: OutputIdentity<'_>) -> Option<&OutputEntry> {
        self.entries
            .iter()
            .find(|entry| entry.matcher.matches(output))
    }

    /// Returns what `output` does with `profile` applied, or with only the
    /// `[[output]]` entries without one.
    pub(crate) fn plan<'settings>(
        &'settings self,
        profile: Option<&'settings OutputProfile>,
        output: OutputIdentity<'_>,
    ) -> OutputPlan<'settings> {
        match profile {
            Some(profile) => profile
                .entry_for(output)
                .map_or(OutputPlan::Off, OutputPlan::Follows),
            None => self
                .entry_for(output)
                .map_or(OutputPlan::Unlisted, OutputPlan::Follows),
        }
    }

    /// Returns the profiles' names, in the order written.
    pub(crate) fn profile_names(&self) -> impl Iterator<Item = &str> {
        self.profiles.iter().map(OutputProfile::name)
    }

    /// Returns the profile called `name`, if there is one.
    pub(crate) fn profile(&self, name: &str) -> Option<&OutputProfile> {
        self.profiles.iter().find(|profile| profile.name() == name)
    }

    /// Returns the profile that applies by itself with `connected`: of
    /// those whose `when_connected` outputs are all there, the one listing
    /// the most, the first written on a tie.
    pub(crate) fn automatic_profile(
        &self,
        connected: &[OutputIdentity<'_>],
    ) -> Option<&OutputProfile> {
        self.profiles
            .iter()
            .filter_map(|profile| Some((profile, profile.connected_match(connected)?)))
            .fold(
                None,
                |best: Option<(&OutputProfile, usize)>, (profile, listed)| match best {
                    Some((_, most)) if most >= listed => best,
                    Some(_) | None => Some((profile, listed)),
                },
            )
            .map(|(profile, _)| profile)
    }

    /// Returns the profile `cycle-profile` goes to from `current`: the next
    /// written after it, round to the start, whose outputs are all among
    /// `connected`; from none, the first such.
    pub(crate) fn profile_after(
        &self,
        current: Option<&str>,
        connected: &[OutputIdentity<'_>],
    ) -> Option<&OutputProfile> {
        let start = current
            .and_then(|name| {
                self.profiles
                    .iter()
                    .position(|profile| profile.name() == name)
            })
            .map_or(0, |index| index + 1);
        let count = self.profiles.len();
        (0..count)
            .map(|step| &self.profiles[(start + step) % count])
            .filter(|profile| Some(profile.name()) != current)
            .find(|profile| profile.fits(connected))
    }
}

impl TryFrom<RawOutputSettings> for OutputSettings {
    type Error = String;

    fn try_from(raw: RawOutputSettings) -> Result<Self, Self::Error> {
        let RawOutputSettings {
            entries,
            profiles,
            hooks,
        } = raw;
        for (index, profile) in profiles.iter().enumerate() {
            if profiles[..index]
                .iter()
                .any(|earlier| earlier.name() == profile.name())
            {
                return Err(format!("two profiles are called \"{}\"", profile.name()));
            }
        }
        Ok(Self {
            entries,
            profiles,
            hooks,
        })
    }
}

impl OutputEntry {
    /// Outputs are on unless an entry says otherwise.
    const fn enabled_by_default() -> bool {
        true
    }

    /// Returns an entry for the output on `connector` that leaves it on,
    /// as it prefers, for the caller to fill in.
    pub(crate) fn for_connector(connector: &str) -> Self {
        Self {
            matcher: OutputMatcher {
                connector: Some(connector.to_owned()),
                make: None,
                model: None,
                serial: None,
            },
            mode: None,
            position: None,
            scale: None,
            transform: OutputTransform::Normal,
            enabled: true,
            primary: false,
            vrr: Vrr::default(),
        }
    }
}

impl OutputTransform {
    /// Returns Smithay's transform for this one.
    pub(crate) const fn to_smithay(self) -> Transform {
        match self {
            Self::Normal => Transform::Normal,
            Self::Turned90 => Transform::_90,
            Self::Turned180 => Transform::_180,
            Self::Turned270 => Transform::_270,
            Self::Flipped => Transform::Flipped,
            Self::Flipped90 => Transform::Flipped90,
            Self::Flipped180 => Transform::Flipped180,
            Self::Flipped270 => Transform::Flipped270,
        }
    }

    /// Returns the transform Smithay's `transform` is.
    pub(crate) const fn from_smithay(transform: Transform) -> Self {
        match transform {
            Transform::Normal => Self::Normal,
            Transform::_90 => Self::Turned90,
            Transform::_180 => Self::Turned180,
            Transform::_270 => Self::Turned270,
            Transform::Flipped => Self::Flipped,
            Transform::Flipped90 => Self::Flipped90,
            Transform::Flipped180 => Self::Flipped180,
            Transform::Flipped270 => Self::Flipped270,
        }
    }
}

impl OutputMatcher {
    /// Returns whether `output` has every field this matcher gives.
    pub(crate) fn matches(&self, output: OutputIdentity<'_>) -> bool {
        let agrees = |wanted: &Option<String>, actual: &str| {
            wanted.as_deref().is_none_or(|wanted| wanted == actual)
        };
        agrees(&self.connector, output.connector)
            && agrees(&self.make, output.make)
            && agrees(&self.model, output.model)
            && agrees(&self.serial, output.serial)
    }
}

impl TryFrom<RawOutputMatcher> for OutputMatcher {
    type Error = &'static str;

    fn try_from(raw: RawOutputMatcher) -> Result<Self, Self::Error> {
        let RawOutputMatcher {
            connector,
            make,
            model,
            serial,
        } = raw;
        if connector.is_none() && make.is_none() && model.is_none() && serial.is_none() {
            return Err("a match needs a connector, or a make, model or serial");
        }
        Ok(Self {
            connector,
            make,
            model,
            serial,
        })
    }
}

impl OutputMode {
    /// Reads `WIDTHxHEIGHT` with an optional `@HERTZ` (which may end in
    /// `Hz`); none if it is not one.
    fn parse(text: &str) -> Option<Self> {
        let (size, refresh) = match text.split_once('@') {
            Some((size, refresh)) => (size, Some(refresh)),
            None => (text, None),
        };
        let (width, height) = size.split_once('x')?;
        let (width, height): (i32, i32) = (width.parse().ok()?, height.parse().ok()?);
        if width <= 0 || height <= 0 {
            return None;
        }
        let refresh_millihertz = match refresh {
            Some(hertz) => {
                let hertz = hertz
                    .strip_suffix("Hz")
                    .or_else(|| hertz.strip_suffix("hz"))
                    .unwrap_or(hertz);
                let hertz: f64 = hertz.parse().ok()?;
                if !(1.0..=1000.0).contains(&hertz) {
                    return None;
                }
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "at most a million millihertz, checked above"
                )]
                Some((hertz * 1000.0).round() as i32)
            }
            None => None,
        };
        Some(Self {
            width,
            height,
            refresh_millihertz,
        })
    }
}

impl fmt::Display for OutputMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}x{}", self.width, self.height)?;
        if let Some(millihertz) = self.refresh_millihertz {
            write!(formatter, "@{}", f64::from(millihertz) / 1000.0)?;
        }
        Ok(())
    }
}

impl<'de> serde::Deserialize<'de> for OutputMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            de::Error::custom(format!(
                "\"{text}\" is not a mode such as \"1920x1080\" or \"1920x1080@60\""
            ))
        })
    }
}

impl OutputScale {
    /// The smallest scale.
    const MIN: f64 = 0.25;
    /// The largest scale.
    const MAX: f64 = 8.0;

    /// Returns the scale `scale`, if it is from 0.25 to 8.
    pub(crate) fn new(scale: f64) -> Option<Self> {
        (Self::MIN..=Self::MAX)
            .contains(&scale)
            .then_some(Self(scale))
    }

    /// Returns the scale.
    pub(crate) const fn get(self) -> f64 {
        self.0
    }
}

impl<'de> serde::Deserialize<'de> for OutputScale {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let scale = f64::deserialize(deserializer)?;
        if (Self::MIN..=Self::MAX).contains(&scale) {
            Ok(Self(scale))
        } else {
            Err(de::Error::custom(format!(
                "a scale of {scale} is not from {} to {}",
                Self::MIN,
                Self::MAX
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two outputs of a desk, and a laptop's panel turned off.
    const DESK: &str = r#"
[[output]]
match = { connector = "HDMI-A-1" }
mode = "1920x1080@60"
position = [0, 0]
primary = true

[[output]]
match = { make = "Dell", model = "U2720Q" }
mode = "3840x2160"
scale = 2.0
transform = "flipped-90"

[[output]]
match = { connector = "eDP-1" }
enabled = false
"#;

    /// An output's identity with `connector`, `make` and `model`.
    const fn identity<'output>(
        connector: &'output str,
        make: &'output str,
        model: &'output str,
    ) -> OutputIdentity<'output> {
        OutputIdentity {
            connector,
            make,
            model,
            serial: "",
        }
    }

    #[test]
    fn entries_match_by_connector_or_by_display() {
        let settings: OutputSettings = toml::from_str(DESK).expect("valid");
        let hdmi = settings
            .entry_for(identity("HDMI-A-1", "Acer", "XB1"))
            .expect("matched by connector");
        assert!(hdmi.primary);
        assert_eq!(hdmi.position, Some([0, 0]));
        assert_eq!(
            hdmi.mode,
            Some(OutputMode {
                width: 1920,
                height: 1080,
                refresh_millihertz: Some(60_000)
            })
        );
        let dell = settings
            .entry_for(identity("DP-2", "Dell", "U2720Q"))
            .expect("matched by make and model");
        assert_eq!(dell.scale.map(OutputScale::get), Some(2.0));
        assert_eq!(dell.transform, OutputTransform::Flipped90);
        assert_eq!(dell.vrr, Vrr::Fullscreen);
        let panel = settings
            .entry_for(identity("eDP-1", "BOE", "0x0BCA"))
            .expect("matched");
        assert!(!panel.enabled);
        assert!(
            settings
                .entry_for(identity("DP-3", "Dell", "P2419H"))
                .is_none()
        );
    }

    /// Returns `profile`'s name, if there is one.
    fn profile_name(profile: Option<&OutputProfile>) -> Option<&str> {
        profile.map(OutputProfile::name)
    }

    /// av-output-switcher's two profiles, as vaht writes them.
    const PROFILES: &str = r#"
[[profile]]
name = "single"
when_connected = [{ connector = "DP-2" }]
outputs = [{ match = { connector = "DP-2" }, mode = "1920x1080@60", position = [0, 0] }]

[[profile]]
name = "office"
when_connected = [{ connector = "HDMI-A-1" }, { connector = "DP-2" }]
primary = [{ connector = "HDMI-A-1" }, { connector = "DP-2" }]
outputs = [
    { match = { connector = "HDMI-A-1" }, position = [0, 0] },
    { match = { connector = "DP-2" }, position = [1920, 0] },
]

[hooks]
profile_applied = "~/bin/on-profile"
"#;

    #[test]
    fn the_profile_naming_the_most_connected_outputs_applies_itself() {
        let settings: OutputSettings = toml::from_str(PROFILES).expect("valid");
        let dp = identity("DP-2", "", "");
        let hdmi = identity("HDMI-A-1", "", "");
        assert_eq!(
            profile_name(settings.automatic_profile(&[dp])),
            Some("single")
        );
        assert_eq!(
            profile_name(settings.automatic_profile(&[dp, hdmi])),
            Some("office")
        );
        assert_eq!(profile_name(settings.automatic_profile(&[hdmi])), None);
        assert_eq!(
            settings.hooks.profile_applied.as_deref(),
            Some("~/bin/on-profile")
        );
    }

    #[test]
    fn a_profile_turns_off_what_it_does_not_list() {
        let settings: OutputSettings = toml::from_str(PROFILES).expect("valid");
        let single = settings.profile("single");
        assert!(matches!(
            settings.plan(single, identity("DP-2", "", "")),
            OutputPlan::Follows(_)
        ));
        assert_eq!(
            settings.plan(single, identity("HDMI-A-1", "", "")),
            OutputPlan::Off
        );
        assert_eq!(
            settings.plan(None, identity("HDMI-A-1", "", "")),
            OutputPlan::Unlisted
        );
    }

    #[test]
    fn cycling_skips_profiles_whose_outputs_are_missing() {
        let settings: OutputSettings = toml::from_str(PROFILES).expect("valid");
        let both = [identity("DP-2", "", ""), identity("HDMI-A-1", "", "")];
        assert_eq!(
            profile_name(settings.profile_after(Some("single"), &both)),
            Some("office")
        );
        assert_eq!(
            profile_name(settings.profile_after(Some("office"), &both)),
            Some("single")
        );
        assert_eq!(
            profile_name(settings.profile_after(None, &both)),
            Some("single")
        );
        assert_eq!(
            profile_name(settings.profile_after(Some("single"), &both[..1])),
            None,
            "office needs HDMI-A-1"
        );
    }

    #[test]
    fn profile_names_are_unique() {
        let twice = format!("{PROFILES}\n[[profile]]\nname = \"single\"\noutputs = []\n");
        let error = toml::from_str::<OutputSettings>(&twice).expect_err("refused");
        assert!(error.to_string().contains("two profiles"), "{error}");
    }

    #[test]
    fn modes_read_with_and_without_a_refresh_rate() {
        assert_eq!(
            OutputMode::parse("2560x1440@59.951").map(|mode| mode.refresh_millihertz),
            Some(Some(59_951))
        );
        assert_eq!(
            OutputMode::parse("1280x720").map(|mode| mode.to_string()),
            Some("1280x720".to_owned())
        );
        assert_eq!(
            OutputMode::parse("1920x1080@144Hz").map(|mode| mode.refresh_millihertz),
            Some(Some(144_000))
        );
        assert_eq!(OutputMode::parse("1280x0"), None);
        assert_eq!(OutputMode::parse("big"), None);
    }

    #[test]
    fn a_match_must_say_something() {
        let error = toml::from_str::<OutputSettings>("[[output]]\nmatch = {}\n")
            .expect_err("an empty match is refused");
        assert!(error.to_string().contains("needs a connector"), "{error}");
    }

    #[test]
    fn a_scale_must_be_sensible() {
        let error = toml::from_str::<OutputSettings>(
            "[[output]]\nmatch = { connector = \"DP-1\" }\nscale = 0.0\n",
        )
        .expect_err("a zero scale is refused");
        assert!(error.to_string().contains("not from"), "{error}");
    }
}
