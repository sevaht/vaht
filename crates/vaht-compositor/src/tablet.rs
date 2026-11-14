//! Drawing tablets (`zwp_tablet_manager_v2`): a stylus's position,
//! pressure, tilt and buttons go to the client under it, as Krita and
//! GIMP read them; fluxland did the same through wlroots.
//!
//! The stylus moves the pointer too, so vaht's own parts follow it as they
//! follow a mouse; and its tip pressed on them (the taskbar, a menu, a
//! frame) presses the pointer's left button there, so tapping works as
//! clicking does. A tablet covers every output, as an absolute pointer
//! does.

use smithay::backend::input::{
    AbsolutePositionEvent as _, ButtonState, Device as _, DeviceCapability, Event as _,
    InputBackend, InputEvent, InputTime, ProximityState, TabletToolButtonEvent as _,
    TabletToolEvent as _, TabletToolProximityEvent as _, TabletToolTipEvent as _,
    TabletToolTipState,
};
use smithay::input::tablet::tool::{
    AxisFrame, ButtonEvent, DownEvent, MotionEvent, ProximityInEvent, ProximityOutEvent, UpEvent,
};
use smithay::input::tablet::{TabletDescriptor, TabletSeat, TabletSeatTrait as _};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER};

use crate::Compositor;
use crate::scene::PointerTarget;

/// The left button's code, as the kernel numbers it (`BTN_LEFT`).
const LEFT_BUTTON: u32 = 0x110;

/// The tablets of the seat.
type Tablets = TabletSeat<Compositor>;

/// Handles `event` if it is a tablet's: within `area`, the backend's when
/// it gives one, else every output; returns whether it was.
pub(crate) fn handle<B: InputBackend>(
    compositor: &mut Compositor,
    event: &InputEvent<B>,
    area: Option<Rectangle<i32, Logical>>,
) -> bool {
    let area = area.unwrap_or_else(|| compositor.outputs.bounds());
    let tablets = compositor.seat.tablet_seat();
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "every kind of input but tablets' is the caller's"
    )]
    match event {
        InputEvent::DeviceAdded { device }
            if device.has_capability(DeviceCapability::TabletTool) =>
        {
            tracing::info!(device = device.name(), "tablet found");
            tablets.add_wp_tablet(&compositor.display_handle, &TabletDescriptor::from(device));
            // Other kinds of device may share it.
            return false;
        }
        InputEvent::DeviceRemoved { device }
            if device.has_capability(DeviceCapability::TabletTool) =>
        {
            tablets.remove_tablet(&TabletDescriptor::from(device));
            if tablets.count_tablets() == 0 {
                tablets.clear_tools();
            }
            return false;
        }
        InputEvent::TabletToolAxis { event } => {
            let location = event.position_transformed(area.size) + area.loc.to_f64();
            on_axis::<B>(compositor, &tablets, event, location);
        }
        InputEvent::TabletToolProximity { event } => {
            let location = event.position_transformed(area.size) + area.loc.to_f64();
            on_proximity::<B>(compositor, &tablets, event, location);
        }
        InputEvent::TabletToolTip { event } => on_tip::<B>(compositor, &tablets, event),
        InputEvent::TabletToolButton { event } => {
            if let Some(tool) = tablets.get_tool(&event.tool()) {
                tool.button(
                    compositor,
                    &ButtonEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        button: event.button(),
                        state: event.button_state(),
                        time: event.time(),
                    },
                );
                tool.frame(compositor, event.time());
            }
        }
        _ => return false,
    }
    compositor.schedule_render();
    true
}

/// Follows the stylus moving, or its pressure, tilt and the like changing,
/// with it at `location`.
fn on_axis<B: InputBackend>(
    compositor: &mut Compositor,
    tablets: &Tablets,
    event: &B::TabletToolAxisEvent,
    location: Point<f64, Logical>,
) {
    let under = follow_stylus(compositor, location, event.time());
    if let Some(tool) = tablets.get_tool(&event.tool()) {
        tool.axis(compositor, axis_frame::<B>(event));
        tool.motion(
            compositor,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time(),
            },
        );
        tool.frame(compositor, event.time());
    }
}

/// Follows the stylus coming near the tablet or leaving it, at
/// `location`.
fn on_proximity<B: InputBackend>(
    compositor: &mut Compositor,
    tablets: &Tablets,
    event: &B::TabletToolProximityEvent,
    location: Point<f64, Logical>,
) {
    let under = follow_stylus(compositor, location, event.time());
    let tablet = tablets.get_tablet(&TabletDescriptor::from(&event.device()));
    let display = compositor.display_handle.clone();
    let tool = tablets
        .get_tool(&event.tool())
        .unwrap_or_else(|| tablets.add_wp_tool(compositor, &display, &event.tool()));
    let Some(tablet) = tablet else {
        return;
    };
    match event.state() {
        ProximityState::In => tool.proximity_in(
            compositor,
            under,
            tablet,
            &ProximityInEvent {
                location,
                axis: Some(axis_frame::<B>(event)),
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time(),
            },
        ),
        ProximityState::Out => tool.proximity_out(
            compositor,
            &ProximityOutEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time(),
            },
        ),
    }
    tool.frame(compositor, event.time());
}

/// Follows the stylus's tip pressed on the tablet or lifted: on a client,
/// it is the client's; elsewhere, the pointer's left button.
fn on_tip<B: InputBackend>(
    compositor: &mut Compositor,
    tablets: &Tablets,
    event: &B::TabletToolTipEvent,
) {
    let Some(tool) = tablets.get_tool(&event.tool()) else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    match event.tip_state() {
        TabletToolTipState::Down => {
            let location = compositor.pointer_location();
            let target = compositor.scene().target_at(location);
            let is_on_client = target
                .as_ref()
                .and_then(PointerTarget::client_surface)
                .is_some()
                && !compositor.menus.covers(location);
            if is_on_client {
                if let Some(window) = target.as_ref().and_then(PointerTarget::window) {
                    compositor.window_manager.focus(Some(window));
                    if compositor.config.settings.focus.raise_on_click {
                        compositor.window_manager.raise(window);
                    }
                }
                tool.down(
                    compositor,
                    &DownEvent {
                        serial,
                        time: event.time(),
                    },
                );
                compositor.settle();
            } else {
                compositor.input.is_stylus_the_pointer = true;
                compositor.pointer_button(LEFT_BUTTON, ButtonState::Pressed, event.time());
            }
        }
        TabletToolTipState::Up => {
            if std::mem::take(&mut compositor.input.is_stylus_the_pointer) {
                compositor.pointer_button(LEFT_BUTTON, ButtonState::Released, event.time());
            } else {
                tool.up(
                    compositor,
                    &UpEvent {
                        serial,
                        time: event.time(),
                    },
                );
            }
        }
    }
    tool.frame(compositor, event.time());
}

/// Moves the pointer with the stylus to `location`, at `time`; returns the
/// client surface there, if any, with its origin.
fn follow_stylus(
    compositor: &mut Compositor,
    location: Point<f64, Logical>,
    time: InputTime,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    compositor.move_pointer_to(location, time);
    if compositor.menus.covers(location) {
        return None;
    }
    compositor
        .scene()
        .target_at(location)
        .as_ref()
        .and_then(PointerTarget::client_surface)
}

/// Returns the axes `event` changed.
fn axis_frame<B: InputBackend>(
    event: &impl smithay::backend::input::TabletToolEvent<B>,
) -> AxisFrame {
    AxisFrame {
        pressure: event.pressure_has_changed().then(|| event.pressure()),
        distance: event.distance_has_changed().then(|| event.distance()),
        tilt: event.tilt_has_changed().then(|| event.tilt()),
        rotation: event.rotation_has_changed().then(|| event.rotation()),
        slider: event.slider_has_changed().then(|| event.slider_position()),
        wheel: event
            .wheel_has_changed()
            .then(|| (event.wheel_delta(), event.wheel_delta_discrete())),
    }
}
