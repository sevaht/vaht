//! Moving and resizing a window by dragging it.
//!
//! A [`WindowGrab`] owns the pointer from the button press until that button
//! is released: while it lasts, no client receives pointer events, and every
//! motion moves or resizes one window. It is the one pointer grab vaht
//! implements; whether a drag moves or resizes is its [`GrabKind`], so
//! titlebar drags, border drags, Alt-drags and clients' own move and resize
//! requests all go through the same code (01 §7).

use smithay::input::pointer::{
    AxisFrame, ButtonEvent, Focus, GestureHoldBeginEvent, GestureHoldEndEvent,
    GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
    GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
    PointerInnerHandle, RelativeMotionEvent,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Serial};

use crate::Compositor;
use crate::window::{ResizeEdges, WindowId};

/// What a drag does to its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GrabKind {
    /// Moves it; the content started at `initial_location`.
    Move {
        /// Where the content's top-left corner was when the drag began.
        initial_location: Point<i32, Logical>,
    },
    /// Resizes it by dragging `edges`.
    Resize {
        /// The edges that follow the pointer.
        edges: ResizeEdges,
    },
}

/// A drag in progress.
pub(crate) struct WindowGrab {
    /// Where and with which button the drag began.
    start: GrabStartData<Compositor>,
    /// The window being dragged.
    window: WindowId,
    /// What the drag does.
    kind: GrabKind,
}

impl std::fmt::Debug for WindowGrab {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowGrab")
            .field("window", &self.window)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Compositor {
    /// Starts dragging `window` with the pointer button held since `start`;
    /// `kind` says whether the drag moves or resizes it.
    pub(crate) fn start_window_grab(
        &mut self,
        window: WindowId,
        kind: GrabKind,
        start: GrabStartData<Self>,
        serial: Serial,
    ) {
        let Some(target) = self.windows.get_mut(window) else {
            return;
        };
        if let GrabKind::Resize { edges } = kind {
            target.begin_resize(edges);
        }
        tracing::debug!(window = %window, ?kind, "drag started");
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        pointer.set_grab(
            self,
            WindowGrab {
                start,
                window,
                kind,
            },
            serial,
            Focus::Clear,
        );
    }

    /// Applies a drag's pointer movement of `delta` since it began.
    fn drag_window(&mut self, window: WindowId, kind: GrabKind, delta: Point<f64, Logical>) {
        let Some(target) = self.windows.get_mut(window) else {
            return;
        };
        let delta = delta.to_i32_round();
        match kind {
            GrabKind::Move { initial_location } => target.set_location(initial_location + delta),
            GrabKind::Resize { .. } => target.resize_by(delta),
        }
        self.schedule_render();
    }

    /// Ends a drag.
    fn end_window_grab(&mut self, window: WindowId, kind: GrabKind) {
        if let (GrabKind::Resize { .. }, Some(target)) = (kind, self.windows.get_mut(window)) {
            target.finish_resize();
        }
        tracing::debug!(window = %window, ?kind, "drag ended");
    }
}

impl PointerGrab<Compositor> for WindowGrab {
    fn motion(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the drag lasts, no client has the pointer.
        handle.motion(compositor, None, event);
        compositor.drag_window(self.window, self.kind, event.location - self.start.location);
    }

    fn relative_motion(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(compositor, None, event);
    }

    fn button(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        event: &ButtonEvent,
    ) {
        handle.button(compositor, event);
        // The drag lasts while the button that began it is held.
        if !handle.current_pressed().contains(&self.start.button) {
            handle.unset_grab(self, compositor, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        details: AxisFrame,
    ) {
        handle.axis(compositor, details);
    }

    fn frame(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
    ) {
        handle.frame(compositor);
    }

    fn gesture_swipe_begin(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(compositor, event);
    }

    fn gesture_swipe_update(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(compositor, event);
    }

    fn gesture_swipe_end(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(compositor, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(compositor, event);
    }

    fn gesture_pinch_update(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(compositor, event);
    }

    fn gesture_pinch_end(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(compositor, event);
    }

    fn gesture_hold_begin(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(compositor, event);
    }

    fn gesture_hold_end(
        &mut self,
        compositor: &mut Compositor,
        handle: &mut PointerInnerHandle<'_, Compositor>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(compositor, event);
    }

    fn start_data(&self) -> &GrabStartData<Compositor> {
        &self.start
    }

    fn unset(&mut self, compositor: &mut Compositor) {
        compositor.end_window_grab(self.window, self.kind);
    }
}
