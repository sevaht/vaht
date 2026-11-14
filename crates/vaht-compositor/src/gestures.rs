//! Touchpad gestures (`zwp_pointer_gestures_v1`): swipes, pinches and
//! holds, passed to the client under the pointer, as browsers and image
//! viewers zoom and go back by them.
//!
//! vaht binds no gestures itself yet; each one libinput reports goes to
//! the pointer, whose grab (a window move, say) sees it first.

use smithay::backend::input::{
    Event as _, GestureBeginEvent as _, GestureEndEvent as _, GesturePinchUpdateEvent as _,
    GestureSwipeUpdateEvent as _, InputBackend, InputEvent,
};
use smithay::input::pointer::{
    GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
    GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
    PointerHandle,
};
use smithay::utils::SERIAL_COUNTER;

use crate::Compositor;

/// Passes `event` to `pointer`'s focus, if it is a gesture; returns
/// whether it was.
pub(crate) fn forward<B: InputBackend>(
    compositor: &mut Compositor,
    pointer: &PointerHandle<Compositor>,
    event: &InputEvent<B>,
) -> bool {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "every kind of input but gestures is the caller's"
    )]
    match event {
        InputEvent::GestureSwipeBegin { event } => pointer.gesture_swipe_begin(
            compositor,
            &GestureSwipeBeginEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time(),
                fingers: event.fingers(),
            },
        ),
        InputEvent::GestureSwipeUpdate { event } => pointer.gesture_swipe_update(
            compositor,
            &GestureSwipeUpdateEvent {
                time: event.time(),
                delta: event.delta(),
            },
        ),
        InputEvent::GestureSwipeEnd { event } => pointer.gesture_swipe_end(
            compositor,
            &GestureSwipeEndEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time(),
                cancelled: event.cancelled(),
            },
        ),
        InputEvent::GesturePinchBegin { event } => pointer.gesture_pinch_begin(
            compositor,
            &GesturePinchBeginEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time(),
                fingers: event.fingers(),
            },
        ),
        InputEvent::GesturePinchUpdate { event } => pointer.gesture_pinch_update(
            compositor,
            &GesturePinchUpdateEvent {
                time: event.time(),
                delta: event.delta(),
                scale: event.scale(),
                rotation: event.rotation(),
            },
        ),
        InputEvent::GesturePinchEnd { event } => pointer.gesture_pinch_end(
            compositor,
            &GesturePinchEndEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time(),
                cancelled: event.cancelled(),
            },
        ),
        InputEvent::GestureHoldBegin { event } => pointer.gesture_hold_begin(
            compositor,
            &GestureHoldBeginEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time(),
                fingers: event.fingers(),
            },
        ),
        InputEvent::GestureHoldEnd { event } => pointer.gesture_hold_end(
            compositor,
            &GestureHoldEndEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time(),
                cancelled: event.cancelled(),
            },
        ),
        _ => return false,
    }
    true
}
