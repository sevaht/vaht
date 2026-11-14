//! Setting up pointing devices as `[input]` says (02 §3).
//!
//! A device's settings ([`crate::config::InputSettings::for_device`])
//! become a list of [`DeviceSetting`]s, each set or back to libinput's
//! default, so a setting taken out of `settings.toml` is undone on reload.
//! [`configure`] applies them to a [`PointingDevice`]: libinput's devices
//! in the DRM backend, and a recorder in tests.

use crate::config::{
    AccelProfile, AccelSpeed, ClickMethod, DeviceSettings, InputSettings, ScrollMethod, SendEvents,
    TapButtonMap,
};

/// One setting of a device: its value, or none for libinput's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceSetting {
    /// Whether tapping clicks.
    Tap(Option<bool>),
    /// Whether a tap and a hold drags.
    TapDrag(Option<bool>),
    /// Whether a drag by tapping goes on a moment after the finger lifts.
    TapDragLock(Option<bool>),
    /// Which button a tap with two and three fingers is.
    TapButtonMap(Option<TapButtonMap>),
    /// Whether scrolling moves the content.
    NaturalScroll(Option<bool>),
    /// Whether the buttons are swapped for the left hand.
    LeftHanded(Option<bool>),
    /// Whether a touchpad rests while keys are typed.
    DisableWhileTyping(Option<bool>),
    /// Whether left and right together are the middle button.
    MiddleEmulation(Option<bool>),
    /// How motion is accelerated.
    AccelProfile(Option<AccelProfile>),
    /// How fast the pointer moves.
    AccelSpeed(Option<AccelSpeed>),
    /// How the device scrolls.
    ScrollMethod(Option<ScrollMethod>),
    /// The button held to scroll.
    ScrollButton(Option<u32>),
    /// How a clickpad's click is a button.
    ClickMethod(Option<ClickMethod>),
    /// Whether the device is used.
    SendEvents(Option<SendEvents>),
}

/// A device libinput sets up.
pub(crate) trait PointingDevice {
    /// Returns the name libinput gives it.
    fn name(&self) -> String;

    /// Returns whether it is a touchpad (it can tap).
    fn is_touchpad(&self) -> bool;

    /// Returns whether it moves the pointer (keyboards do not).
    fn is_pointer(&self) -> bool;

    /// Applies `setting`.
    ///
    /// # Errors
    ///
    /// Returns why not: the device lacks the feature, or refused the value.
    fn apply(&mut self, setting: DeviceSetting) -> Result<(), String>;
}

/// Sets `device` up as `settings` say, if it moves the pointer; a setting
/// the device lacks is skipped.
pub(crate) fn configure(device: &mut impl PointingDevice, settings: &InputSettings) {
    if !device.is_pointer() {
        return;
    }
    let name = device.name();
    for setting in settings_list(&settings.for_device(&name, device.is_touchpad())) {
        if let Err(error) = device.apply(setting) {
            tracing::debug!(
                device = name,
                ?setting,
                error,
                "an input setting was not applied"
            );
        }
    }
}

/// Returns `settings` as a list: every setting, set or default.
fn settings_list(settings: &DeviceSettings) -> [DeviceSetting; 14] {
    [
        DeviceSetting::Tap(settings.tap),
        DeviceSetting::TapDrag(settings.tap_drag),
        DeviceSetting::TapDragLock(settings.tap_drag_lock),
        DeviceSetting::TapButtonMap(settings.tap_button_map),
        DeviceSetting::NaturalScroll(settings.natural_scroll),
        DeviceSetting::LeftHanded(settings.left_handed),
        DeviceSetting::DisableWhileTyping(settings.disable_while_typing),
        DeviceSetting::MiddleEmulation(settings.middle_emulation),
        DeviceSetting::AccelProfile(settings.accel_profile),
        DeviceSetting::AccelSpeed(settings.accel_speed),
        DeviceSetting::ScrollMethod(settings.scroll_method),
        DeviceSetting::ScrollButton(settings.scroll_button),
        DeviceSetting::ClickMethod(settings.click_method),
        DeviceSetting::SendEvents(settings.send_events),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device that records what it was set to.
    #[derive(Debug)]
    struct Recorder {
        /// Its name.
        name: &'static str,
        /// Whether it is a touchpad.
        is_touchpad: bool,
        /// Whether it moves the pointer.
        is_pointer: bool,
        /// What it was set to, in order.
        applied: Vec<DeviceSetting>,
    }

    impl PointingDevice for Recorder {
        fn name(&self) -> String {
            self.name.to_owned()
        }

        fn is_touchpad(&self) -> bool {
            self.is_touchpad
        }

        fn is_pointer(&self) -> bool {
            self.is_pointer
        }

        fn apply(&mut self, setting: DeviceSetting) -> Result<(), String> {
            self.applied.push(setting);
            Ok(())
        }
    }

    /// A device named `name`, a touchpad or not, that moves the pointer.
    const fn device(name: &'static str, is_touchpad: bool) -> Recorder {
        Recorder {
            name,
            is_touchpad,
            is_pointer: true,
            applied: Vec::new(),
        }
    }

    /// Reads `[input]` settings from `toml`.
    fn settings(toml: &str) -> InputSettings {
        toml::from_str(toml).expect("the settings are valid")
    }

    #[test]
    fn a_touchpad_gets_its_settings_and_defaults_for_the_rest() {
        let mut touchpad = device("Touchpad", true);
        configure(&mut touchpad, &settings("[touchpad]\ntap = true\n"));
        assert!(touchpad.applied.contains(&DeviceSetting::Tap(Some(true))));
        assert!(
            touchpad
                .applied
                .contains(&DeviceSetting::NaturalScroll(None)),
            "unset: back to libinput's default"
        );
        assert_eq!(touchpad.applied.len(), 14, "every setting, set or default");
    }

    #[test]
    fn a_mouse_is_not_set_up_as_a_touchpad() {
        let mut mouse = device("Mouse", false);
        configure(&mut mouse, &settings("[touchpad]\ntap = true\n"));
        assert!(mouse.applied.contains(&DeviceSetting::Tap(None)));
    }

    #[test]
    fn a_keyboard_is_left_alone() {
        let mut keyboard = device("Keyboard", false);
        keyboard.is_pointer = false;
        configure(&mut keyboard, &settings("[all]\nleft_handed = true\n"));
        assert_eq!(keyboard.applied, []);
    }
}
