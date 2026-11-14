//! libinput's devices as [`PointingDevice`]s: each setting carried to
//! libinput, or its default read back from it.

use smithay::reexports::input::{self, Device, DeviceCapability};

use crate::config::{
    AccelProfile, AccelSpeed, ClickMethod, ScrollMethod, SendEvents, TapButtonMap,
};
use crate::pointing_devices::{DeviceSetting, PointingDevice};

impl PointingDevice for Device {
    fn name(&self) -> String {
        Self::name(self).into_owned()
    }

    fn is_touchpad(&self) -> bool {
        self.config_tap_finger_count() > 0
    }

    fn is_pointer(&self) -> bool {
        self.has_capability(DeviceCapability::Pointer)
    }

    fn apply(&mut self, setting: DeviceSetting) -> Result<(), String> {
        let result = match setting {
            DeviceSetting::Tap(value) => {
                let enabled = value.unwrap_or_else(|| self.config_tap_default_enabled());
                self.config_tap_set_enabled(enabled)
            }
            DeviceSetting::TapDrag(value) => {
                let enabled = value.unwrap_or_else(|| self.config_tap_default_drag_enabled());
                self.config_tap_set_drag_enabled(enabled)
            }
            DeviceSetting::TapDragLock(value) => {
                let state = match value {
                    Some(true) => input::DragLockState::EnabledTimeout,
                    Some(false) => input::DragLockState::Disabled,
                    None => self.config_tap_default_drag_lock_enabled(),
                };
                self.config_tap_set_drag_lock_enabled(state)
            }
            DeviceSetting::TapButtonMap(value) => {
                let map = match value {
                    Some(TapButtonMap::Lrm) => Some(input::TapButtonMap::LeftRightMiddle),
                    Some(TapButtonMap::Lmr) => Some(input::TapButtonMap::LeftMiddleRight),
                    None => self.config_tap_default_button_map(),
                };
                map.map_or(Ok(()), |map| self.config_tap_set_button_map(map))
            }
            DeviceSetting::NaturalScroll(value) => {
                let enabled =
                    value.unwrap_or_else(|| self.config_scroll_default_natural_scroll_enabled());
                self.config_scroll_set_natural_scroll_enabled(enabled)
            }
            DeviceSetting::LeftHanded(value) => {
                let enabled = value.unwrap_or_else(|| self.config_left_handed_default());
                self.config_left_handed_set(enabled)
            }
            DeviceSetting::DisableWhileTyping(value) => {
                let enabled = value.unwrap_or_else(|| self.config_dwt_default_enabled());
                self.config_dwt_set_enabled(enabled)
            }
            DeviceSetting::MiddleEmulation(value) => {
                let enabled =
                    value.unwrap_or_else(|| self.config_middle_emulation_default_enabled());
                self.config_middle_emulation_set_enabled(enabled)
            }
            DeviceSetting::AccelProfile(value) => {
                let profile = match value {
                    Some(AccelProfile::Flat) => Some(input::AccelProfile::Flat),
                    Some(AccelProfile::Adaptive) => Some(input::AccelProfile::Adaptive),
                    None => self.config_accel_default_profile(),
                };
                profile.map_or(Ok(()), |profile| self.config_accel_set_profile(profile))
            }
            DeviceSetting::AccelSpeed(value) => {
                let speed =
                    value.map_or_else(|| self.config_accel_default_speed(), AccelSpeed::value);
                self.config_accel_set_speed(speed)
            }
            DeviceSetting::ScrollMethod(value) => {
                let method = match value {
                    Some(ScrollMethod::None) => Some(input::ScrollMethod::NoScroll),
                    Some(ScrollMethod::TwoFinger) => Some(input::ScrollMethod::TwoFinger),
                    Some(ScrollMethod::Edge) => Some(input::ScrollMethod::Edge),
                    Some(ScrollMethod::Button) => Some(input::ScrollMethod::OnButtonDown),
                    None => self.config_scroll_default_method(),
                };
                method.map_or(Ok(()), |method| self.config_scroll_set_method(method))
            }
            DeviceSetting::ScrollButton(value) => {
                let button = value.unwrap_or_else(|| self.config_scroll_default_button());
                self.config_scroll_set_button(button)
            }
            DeviceSetting::ClickMethod(value) => {
                let method = match value {
                    Some(ClickMethod::ButtonAreas) => Some(input::ClickMethod::ButtonAreas),
                    Some(ClickMethod::ClickFinger) => Some(input::ClickMethod::Clickfinger),
                    None => self.config_click_default_method(),
                };
                method.map_or(Ok(()), |method| self.config_click_set_method(method))
            }
            DeviceSetting::SendEvents(value) => {
                let mode = match value {
                    // libinput's default is always to send events.
                    Some(SendEvents::Enabled) | None => input::SendEventsMode::ENABLED,
                    Some(SendEvents::Disabled) => input::SendEventsMode::DISABLED,
                    Some(SendEvents::DisabledOnExternalMouse) => {
                        input::SendEventsMode::DISABLED_ON_EXTERNAL_MOUSE
                    }
                };
                self.config_send_events_set_mode(mode)
            }
        };
        result.map_err(|error| format!("{error:?}"))
    }
}
