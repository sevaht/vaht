//! Touch screens: each touch goes to the client it lands on, as
//! `wl_touch` points; one landing on vaht's own parts (the taskbar, a
//! menu, a frame) stands in for the pointer instead, pressing and
//! releasing its left button, so tapping works there as clicking does.
//! fluxland did the same.
//!
//! The seat offers touch once a touch screen is found. A touch screen
//! covers the laptop's own panel (an `eDP` output) when there is one, else
//! the primary output. Which touch goes where is
//! decided from plain values ([`TouchRoutes`]), so tests follow it alone.

use smithay::backend::input::{
    AbsolutePositionEvent as _, ButtonState, Device as _, DeviceCapability, Event as _,
    InputBackend, InputEvent, TouchEvent as _, TouchSlot,
};
use smithay::input::touch::{DownEvent, MotionEvent, UpEvent};
use smithay::utils::{Logical, Rectangle, SERIAL_COUNTER};

use crate::Compositor;
use crate::outputs::Outputs;
use crate::scene::PointerTarget;

/// The left button's code, as the kernel numbers it (`BTN_LEFT`).
const LEFT_BUTTON: u32 = 0x110;

/// Where a touch point goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TouchRoute {
    /// To the client it landed on.
    Client,
    /// To vaht, as the pointer.
    Pointer,
}

/// The touch points down, and where each goes.
#[derive(Debug, Default)]
pub(crate) struct TouchRoutes {
    /// Each point down, by slot.
    down: Vec<(TouchSlot, TouchRoute)>,
}

impl TouchRoutes {
    /// Returns where a touch landing at `slot` goes: to the client it is
    /// on, if it `is_on_client`; else as the pointer, unless another
    /// point is the pointer already, when it goes nowhere.
    pub(crate) fn down(&mut self, slot: TouchSlot, is_on_client: bool) -> Option<TouchRoute> {
        self.down.retain(|(known, _)| *known != slot);
        let route = if is_on_client {
            TouchRoute::Client
        } else if self
            .down
            .iter()
            .any(|(_, route)| *route == TouchRoute::Pointer)
        {
            return None;
        } else {
            TouchRoute::Pointer
        };
        self.down.push((slot, route));
        Some(route)
    }

    /// Returns where the touch at `slot` goes, if it is down.
    pub(crate) fn route(&self, slot: TouchSlot) -> Option<TouchRoute> {
        self.down
            .iter()
            .find(|(known, _)| *known == slot)
            .map(|(_, route)| *route)
    }

    /// Forgets the touch at `slot`, lifted; returns where it went.
    pub(crate) fn up(&mut self, slot: TouchSlot) -> Option<TouchRoute> {
        let route = self.route(slot);
        self.down.retain(|(known, _)| *known != slot);
        route
    }

    /// Forgets every touch, cancelled; returns whether one was the
    /// pointer.
    pub(crate) fn cancel(&mut self) -> bool {
        let was_pointer = self
            .down
            .iter()
            .any(|(_, route)| *route == TouchRoute::Pointer);
        self.down.clear();
        was_pointer
    }
}

/// Handles `event` if it is a touch: within `area`, the backend's when it
/// gives one, else the touch screen's output; returns whether it was.
pub(crate) fn handle<B: InputBackend>(
    compositor: &mut Compositor,
    event: &InputEvent<B>,
    area: Option<Rectangle<i32, Logical>>,
) -> bool {
    if let InputEvent::DeviceAdded { device } = event {
        // Clients are told the seat has touch once a touch screen is there.
        if device.has_capability(DeviceCapability::Touch) && compositor.seat.get_touch().is_none() {
            tracing::info!(device = device.name(), "touch screen found");
            compositor.seat.add_touch();
        }
        return false;
    }
    let area = area.unwrap_or_else(|| touch_area(&compositor.outputs));
    let Some(touch) = compositor.seat.get_touch() else {
        return false;
    };
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "every kind of input but touch is the caller's"
    )]
    match event {
        InputEvent::TouchDown { event } => {
            let location = event.position_transformed(area.size) + area.loc.to_f64();
            let target = if compositor.menus.covers(location) {
                None
            } else {
                compositor.scene().target_at(location)
            };
            let client = target.as_ref().and_then(PointerTarget::client_surface);
            match compositor.input.touch.down(event.slot(), client.is_some()) {
                Some(TouchRoute::Client) => {
                    if let Some(window) = target.as_ref().and_then(PointerTarget::window) {
                        compositor.window_manager.focus(Some(window));
                        if compositor.config.settings.focus.raise_on_click {
                            compositor.window_manager.raise(window);
                        }
                    }
                    touch.down(
                        compositor,
                        client,
                        &DownEvent {
                            slot: event.slot(),
                            location,
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time(),
                        },
                    );
                    compositor.settle();
                }
                Some(TouchRoute::Pointer) => {
                    compositor.move_pointer_to(location, event.time());
                    compositor.pointer_button(LEFT_BUTTON, ButtonState::Pressed, event.time());
                }
                None => {}
            }
        }
        InputEvent::TouchMotion { event } => {
            let location = event.position_transformed(area.size) + area.loc.to_f64();
            match compositor.input.touch.route(event.slot()) {
                Some(TouchRoute::Client) => {
                    // Each point stays with the surface it landed on.
                    touch.motion(
                        compositor,
                        None,
                        &MotionEvent {
                            slot: event.slot(),
                            location,
                            time: event.time(),
                        },
                    );
                }
                Some(TouchRoute::Pointer) => compositor.move_pointer_to(location, event.time()),
                None => {}
            }
        }
        InputEvent::TouchUp { event } => match compositor.input.touch.up(event.slot()) {
            Some(TouchRoute::Client) => touch.up(
                compositor,
                &UpEvent {
                    slot: event.slot(),
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time(),
                },
            ),
            Some(TouchRoute::Pointer) => {
                compositor.pointer_button(LEFT_BUTTON, ButtonState::Released, event.time());
            }
            None => {}
        },
        InputEvent::TouchFrame { .. } => touch.frame(compositor),
        InputEvent::TouchCancel { event } => {
            if compositor.input.touch.cancel() {
                compositor.pointer_button(LEFT_BUTTON, ButtonState::Released, event.time());
            }
            touch.cancel(compositor);
        }
        _ => return false,
    }
    compositor.schedule_render();
    true
}

/// Returns the area a touch screen covers: the laptop's own panel if
/// there is one on, else the primary output.
fn touch_area(outputs: &Outputs) -> Rectangle<i32, Logical> {
    outputs
        .iter()
        .find(|output| output.name().starts_with("eDP"))
        .map_or_else(|| outputs.primary_geometry(), Outputs::geometry)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot numbered `id`.
    fn slot(id: u32) -> TouchSlot {
        Some(id).into()
    }

    #[test]
    fn a_touch_on_a_client_is_its_and_one_elsewhere_is_the_pointer() {
        let mut routes = TouchRoutes::default();
        assert_eq!(routes.down(slot(0), true), Some(TouchRoute::Client));
        assert_eq!(routes.down(slot(1), false), Some(TouchRoute::Pointer));
        assert_eq!(
            routes.down(slot(2), false),
            None,
            "one point at a time is the pointer"
        );
        assert_eq!(routes.route(slot(1)), Some(TouchRoute::Pointer));
        assert_eq!(routes.up(slot(1)), Some(TouchRoute::Pointer));
        assert_eq!(routes.down(slot(3), false), Some(TouchRoute::Pointer));
        assert!(routes.cancel(), "the pointer's touch was cancelled");
        assert_eq!(routes.route(slot(0)), None);
    }
}
