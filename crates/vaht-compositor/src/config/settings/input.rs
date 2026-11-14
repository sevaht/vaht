//! `[input]`: how pointing devices behave, as libinput sets them (02 §3):
//! fluxland's `session.input.*`.
//!
//! Settings come in four scopes, the more specific winning: `[input.all]`
//! (every device), `[input.touchpad]` or `[input.pointer]` (touchpads, or
//! other pointers such as mice and trackballs), and
//! `[input.device."<name>"]` (one device, by the name libinput gives it,
//! or that name with each character but letters and digits made `_`, as
//! fluxland's `session.input.device.<Name>` named it).
//! A setting set in no scope keeps libinput's default.

use std::collections::BTreeMap;

/// Every scope's settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct InputSettings {
    /// Every device's.
    all: DeviceSettings,
    /// Touchpads'.
    touchpad: DeviceSettings,
    /// Other pointers': mice, trackballs, trackpoints.
    pointer: DeviceSettings,
    /// One device's, by its name.
    device: BTreeMap<String, DeviceSettings>,
}

impl InputSettings {
    /// Returns the settings of the device `name`, a touchpad or not: each
    /// one from the most specific scope that sets it.
    pub(crate) fn for_device(&self, name: &str, is_touchpad: bool) -> DeviceSettings {
        let kind = if is_touchpad {
            &self.touchpad
        } else {
            &self.pointer
        };
        let general = kind.clone().or(&self.all);
        let sanitized: String = name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        self.device
            .get(name)
            .or_else(|| self.device.get(&sanitized))
            .map_or(general.clone(), |own| own.clone().or(&general))
    }
}

/// One scope's settings; each unset keeps the less specific scope's, or
/// libinput's default.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DeviceSettings {
    /// Whether tapping clicks.
    pub(crate) tap: Option<bool>,
    /// Whether a tap and a hold drags.
    pub(crate) tap_drag: Option<bool>,
    /// Whether a drag by tapping goes on a moment after the finger lifts.
    pub(crate) tap_drag_lock: Option<bool>,
    /// Which button a tap with two and three fingers is.
    pub(crate) tap_button_map: Option<TapButtonMap>,
    /// Whether scrolling moves the content (as on a phone), not the view.
    pub(crate) natural_scroll: Option<bool>,
    /// Whether the buttons are swapped for the left hand.
    pub(crate) left_handed: Option<bool>,
    /// Whether a touchpad rests while keys are typed.
    pub(crate) disable_while_typing: Option<bool>,
    /// Whether pressing left and right together is the middle button.
    pub(crate) middle_emulation: Option<bool>,
    /// How motion is accelerated.
    pub(crate) accel_profile: Option<AccelProfile>,
    /// How fast the pointer moves, from -1 (slowest) to 1 (fastest).
    pub(crate) accel_speed: Option<AccelSpeed>,
    /// How the device scrolls.
    pub(crate) scroll_method: Option<ScrollMethod>,
    /// The button held to scroll by moving, with `scroll_method =
    /// "button"`, as a Linux button code (`BTN_MIDDLE` is 274).
    pub(crate) scroll_button: Option<u32>,
    /// How a clickpad's click is a button.
    pub(crate) click_method: Option<ClickMethod>,
    /// Whether the device is used.
    pub(crate) send_events: Option<SendEvents>,
}

impl DeviceSettings {
    /// Returns these settings, each unset one taken from `fallback`.
    #[must_use]
    fn or(self, fallback: &Self) -> Self {
        Self {
            tap: self.tap.or(fallback.tap),
            tap_drag: self.tap_drag.or(fallback.tap_drag),
            tap_drag_lock: self.tap_drag_lock.or(fallback.tap_drag_lock),
            tap_button_map: self.tap_button_map.or(fallback.tap_button_map),
            natural_scroll: self.natural_scroll.or(fallback.natural_scroll),
            left_handed: self.left_handed.or(fallback.left_handed),
            disable_while_typing: self.disable_while_typing.or(fallback.disable_while_typing),
            middle_emulation: self.middle_emulation.or(fallback.middle_emulation),
            accel_profile: self.accel_profile.or(fallback.accel_profile),
            accel_speed: self.accel_speed.or(fallback.accel_speed),
            scroll_method: self.scroll_method.or(fallback.scroll_method),
            scroll_button: self.scroll_button.or(fallback.scroll_button),
            click_method: self.click_method.or(fallback.click_method),
            send_events: self.send_events.or(fallback.send_events),
        }
    }
}

/// Which button a tap with two and three fingers is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TapButtonMap {
    /// Two fingers right, three middle.
    Lrm,
    /// Two fingers middle, three right.
    Lmr,
}

/// How motion is accelerated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AccelProfile {
    /// Not at all: the pointer moves as far as the device does, times the
    /// speed.
    Flat,
    /// More the faster the device moves.
    Adaptive,
}

/// How fast the pointer moves, from -1 to 1, in thousandths (so settings
/// compare exactly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AccelSpeed(i16);

impl AccelSpeed {
    /// Returns the speed, from -1 to 1.
    pub(crate) fn value(self) -> f64 {
        f64::from(self.0) / 1000.0
    }
}

impl<'de> serde::Deserialize<'de> for AccelSpeed {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let speed = f64::deserialize(deserializer)?;
        if !(-1.0..=1.0).contains(&speed) {
            return Err(serde::de::Error::custom(format!(
                "accel_speed {speed} is not between -1 and 1"
            )));
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "between -1000 and 1000 after the check above"
        )]
        let thousandths = (speed * 1000.0).round() as i16;
        Ok(Self(thousandths))
    }
}

/// How a device scrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ScrollMethod {
    /// It does not, but by its wheel.
    None,
    /// By moving two fingers.
    TwoFinger,
    /// By moving a finger along the edge.
    Edge,
    /// By moving while a button is held (`scroll_button`).
    Button,
}

/// How a clickpad's click is a button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ClickMethod {
    /// By where it is pressed: the bottom right is the right button.
    ButtonAreas,
    /// By how many fingers press: two are the right button.
    ClickFinger,
}

/// Whether a device is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SendEvents {
    /// It is.
    Enabled,
    /// It is not.
    Disabled,
    /// Only while no mouse is plugged in (a touchpad).
    DisabledOnExternalMouse,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads `[input]` settings from `toml`.
    fn input(toml: &str) -> InputSettings {
        toml::from_str(toml).expect("the settings are valid")
    }

    #[test]
    fn the_most_specific_scope_wins() {
        let settings = input(
            r#"
            [all]
            natural_scroll = true
            left_handed = false
            [touchpad]
            tap = true
            [pointer]
            accel_profile = "flat"
            accel_speed = -0.5
            [device."Logitech USB Trackball"]
            scroll_method = "button"
            scroll_button = 276
            left_handed = true
            "#,
        );
        let touchpad = settings.for_device("SynPS/2 Touchpad", true);
        assert_eq!(touchpad.tap, Some(true));
        assert_eq!(touchpad.natural_scroll, Some(true), "from every device's");
        assert_eq!(
            touchpad.accel_profile, None,
            "a pointer's, not a touchpad's"
        );
        let trackball = settings.for_device("Logitech USB Trackball", false);
        assert_eq!(
            trackball.left_handed,
            Some(true),
            "its own over every device's"
        );
        assert_eq!(trackball.scroll_method, Some(ScrollMethod::Button));
        assert_eq!(trackball.scroll_button, Some(276));
        assert_eq!(trackball.accel_profile, Some(AccelProfile::Flat));
        assert_eq!(trackball.accel_speed.map(AccelSpeed::value), Some(-0.5));
        assert_eq!(trackball.tap, None, "a touchpad's, so libinput's default");
    }

    #[test]
    fn a_device_is_found_by_fluxlands_name_for_it() {
        let settings = input(
            "[device.Logitech_USB_Trackball]
left_handed = true
",
        );
        assert_eq!(
            settings
                .for_device("Logitech USB Trackball", false)
                .left_handed,
            Some(true)
        );
    }

    #[test]
    fn no_settings_leave_libinputs_defaults() {
        assert_eq!(
            InputSettings::default().for_device("any", true),
            DeviceSettings::default()
        );
    }

    #[test]
    fn a_speed_beyond_one_is_refused() {
        let error = toml::from_str::<InputSettings>("[all]\naccel_speed = 1.5\n")
            .expect_err("1.5 is too fast");
        assert!(error.to_string().contains("between -1 and 1"), "{error}");
    }

    #[test]
    fn an_unknown_setting_is_refused() {
        assert!(toml::from_str::<InputSettings>("[all]\ntap_to_click = true\n").is_err());
    }
}
