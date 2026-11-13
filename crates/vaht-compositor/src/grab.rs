//! Moving and resizing a window by dragging it, and holding a bound press.
//!
//! A [`WindowGrab`] owns the pointer from the button press until that button
//! is released: while it lasts, no client receives pointer events. It is the
//! one pointer grab vaht implements; what a motion does is its [`GrabKind`],
//! so titlebar drags, border drags, Alt-drags and clients' own move and
//! resize requests all go through the same code (01 §7).
//!
//! A press with a mouse binding starts as a [`GrabKind::Hold`]: the press
//! is vaht's, not the client's. Once the pointer moves a few pixels, the
//! binding's drag command runs, and its `start-moving` or `start-resizing`
//! turns the hold into a move or resize.

use smithay::input::pointer::{
    AxisFrame, ButtonEvent, Focus, GestureHoldBeginEvent, GestureHoldEndEvent,
    GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
    GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
    PointerInnerHandle, RelativeMotionEvent,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Serial};

use crate::Compositor;
use crate::config::Command;
use crate::execute::{ExecutionContext, Press};
use crate::window::{ResizeEdges, WindowId};

/// How far the pointer moves, in pixels, before a held press is a drag.
const DRAG_DISTANCE: f64 = 3.0;

/// What a drag does to its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GrabKind {
    /// Nothing yet: the press is held, and may become a drag.
    Hold,
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
    /// The press's serial.
    serial: Serial,
    /// The window being dragged; none for a press on the desktop.
    window: Option<WindowId>,
    /// What the drag does.
    kind: GrabKind,
    /// The command a held press runs once the pointer moves, if any.
    drag: Option<Command>,
}

impl std::fmt::Debug for WindowGrab {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowGrab")
            .field("window", &self.window)
            .field("kind", &self.kind)
            .field("drag", &self.drag)
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
        if !self.prepare_drag(window, kind) {
            return;
        }
        self.set_grab(WindowGrab {
            start,
            serial,
            window: Some(window),
            kind,
            drag: None,
        });
    }

    /// Holds the bound `press` on `window` (or the desktop) until its
    /// button is released, running `drag` if the pointer moves first.
    pub(crate) fn hold_press(
        &mut self,
        window: Option<WindowId>,
        drag: Option<Command>,
        press: Press,
    ) {
        self.set_grab(WindowGrab {
            start: press.start,
            serial: press.serial,
            window,
            kind: GrabKind::Hold,
            drag,
        });
    }

    /// Makes the held press `grab` on `window` a drag of `kind`.
    pub(crate) fn turn_grab_into(&mut self, window: WindowId, grab: &mut GrabKind, kind: GrabKind) {
        if *grab == GrabKind::Hold && self.prepare_drag(window, kind) {
            *grab = kind;
        }
    }

    /// Readies `window` for a drag of `kind`; returns whether it exists.
    fn prepare_drag(&mut self, window: WindowId, kind: GrabKind) -> bool {
        let Some(target) = self.windows.get_mut(window) else {
            return false;
        };
        if let GrabKind::Resize { edges } = kind {
            target.begin_resize(edges);
        }
        tracing::debug!(window = %window, ?kind, "drag started");
        true
    }

    /// Gives the pointer to `grab`.
    fn set_grab(&mut self, grab: WindowGrab) {
        let serial = grab.serial;
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    /// Applies a drag's pointer movement of `delta` since it began.
    fn drag_window(&mut self, window: WindowId, kind: GrabKind, delta: Point<f64, Logical>) {
        let Some(target) = self.windows.get_mut(window) else {
            return;
        };
        let delta = delta.to_i32_round();
        match kind {
            GrabKind::Hold => return,
            GrabKind::Move { initial_location } => target.set_location(initial_location + delta),
            GrabKind::Resize { .. } => target.resize_by(delta),
        }
        self.schedule_render();
    }

    /// Ends a drag.
    fn end_window_grab(&mut self, window: Option<WindowId>, kind: GrabKind) {
        if kind == GrabKind::Hold {
            return;
        }
        if let (GrabKind::Resize { .. }, Some(target)) =
            (kind, window.and_then(|window| self.windows.get_mut(window)))
        {
            target.finish_resize();
        }
        tracing::debug!(window = ?window, ?kind, "drag ended");
    }
}

impl WindowGrab {
    /// Runs the held press's drag command once the pointer is `delta` away
    /// from the press, far enough to be a drag.
    fn begin_drag_if_moved(&mut self, compositor: &mut Compositor, delta: Point<f64, Logical>) {
        if self.kind != GrabKind::Hold || delta.x.hypot(delta.y) < DRAG_DISTANCE {
            return;
        }
        let Some(drag) = self.drag.take() else {
            return;
        };
        let mut context = ExecutionContext {
            window: self.window,
            press: Some(Press {
                start: self.start.clone(),
                serial: self.serial,
            }),
            grab: Some(&mut self.kind),
        };
        compositor.execute(&drag, &mut context);
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
        let delta = event.location - self.start.location;
        self.begin_drag_if_moved(compositor, delta);
        if let Some(window) = self.window {
            compositor.drag_window(window, self.kind, delta);
        }
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
