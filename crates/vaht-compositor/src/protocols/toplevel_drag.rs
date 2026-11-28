//! `xdg_toplevel_drag_manager_v1`: dragging a window along with a drag and
//! drop, as browsers drag a tab out into a window of its own.
//!
//! The program starts an ordinary drag of a data source, makes an
//! `xdg_toplevel_drag_v1` of that source, and attaches a window to it (the
//! tab's new window, often before mapping it) at an offset from the window
//! geometry's corner. While that drag lasts, the window follows the pointer
//! at that offset and is no drop target itself; when the drag ends, it
//! stays where it was let go. [`ToplevelDrags`] keeps each drag object, and
//! the compositor asks it which window, if any, the drag in progress
//! carries.

use std::any::Any;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use smithay::input::dnd::{DndAction, Source, SourceMetadata};
use smithay::utils::IsAlive;

use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel;
use smithay::reexports::wayland_protocols::xdg::toplevel_drag::v1::server::xdg_toplevel_drag_manager_v1::{self, XdgToplevelDragManagerV1};
use smithay::reexports::wayland_protocols::xdg::toplevel_drag::v1::server::xdg_toplevel_drag_v1::{self, XdgToplevelDragV1};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource as _,
};
use smithay::utils::{Logical, Point};

use crate::Compositor;

/// A drag's source, which is told it was cancelled only once the grab has
/// said why it ended.
///
/// A drag dropped where no window accepts it is cancelled, which a program
/// that dragged a window out (a browser's tab) takes as "put it back". But
/// the user let go of a window, wherever that was: it is dropped, and stays.
/// So the cancelling is held until [`EndsDrag::end`] tells which it was.
#[derive(Debug)]
pub(crate) struct HeldCancel<S: Source> {
    /// The source.
    source: S,
    /// Whether the source asked to be cancelled, and was not yet told.
    is_held: AtomicBool,
}

impl<S: Source> HeldCancel<S> {
    /// Holds the cancelling of `source`.
    pub(crate) const fn new(source: S) -> Self {
        Self {
            source,
            is_held: AtomicBool::new(false),
        }
    }
}

/// What the compositor tells a drag's source when the drag has ended.
pub(crate) trait EndsDrag: Send + Sync {
    /// Tells the source it was dropped (`as_drop`), or cancelled, if it
    /// asked to be cancelled and was not told.
    fn end(&self, as_drop: bool);
}

impl<S: Source> EndsDrag for HeldCancel<S> {
    fn end(&self, as_drop: bool) {
        if self.is_held.swap(false, Ordering::AcqRel) {
            if as_drop {
                self.source.drop_performed();
            } else {
                self.source.cancel();
            }
        }
    }
}

impl<S: Source> Drop for HeldCancel<S> {
    fn drop(&mut self) {
        // A drag that ended in no way the compositor was told of is cancelled.
        self.end(false);
    }
}

/// The source of a drag, as the drag handles it: a [`HeldCancel`] shared
/// with the compositor, which ends it.
#[derive(Debug)]
pub(crate) struct HeldSource<S: Source>(pub(crate) Arc<HeldCancel<S>>);

impl<S: Source> IsAlive for HeldSource<S> {
    fn alive(&self) -> bool {
        self.0.source.alive()
    }
}

impl<S: Source> Source for HeldSource<S> {
    fn is_client_local(&self, target: &dyn Any) -> bool {
        self.0.source.is_client_local(target)
    }

    fn metadata(&self) -> Option<SourceMetadata> {
        self.0.source.metadata()
    }

    fn accepted(&self, mime_type: Option<String>) {
        self.0.source.accepted(mime_type);
    }

    fn choose_action(&self, action: DndAction) {
        self.0.source.choose_action(action);
    }

    fn send(&self, mime_type: &str, fd: OwnedFd) {
        self.0.source.send(mime_type, fd);
    }

    fn drop_performed(&self) {
        self.0.source.drop_performed();
    }

    fn cancel(&self) {
        self.0.is_held.store(true, Ordering::Release);
    }

    fn finished(&self) {
        self.0.source.finished();
    }
}

/// A window attached to a drag, and where the pointer holds it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AttachedWindow {
    /// The window's surface.
    pub(crate) surface: WlSurface,
    /// Where the pointer is on the window, from its geometry's corner.
    pub(crate) offset: Point<i32, Logical>,
}

/// What a drag object knows: its source, and the window attached to it.
#[derive(Debug)]
pub(crate) struct DragData {
    /// The data source being dragged.
    source: WlDataSource,
    /// The window attached, if one is.
    attached: Mutex<Option<AttachedWindow>>,
}

/// Every window drag object clients hold.
#[derive(Debug, Default)]
pub(crate) struct ToplevelDrags {
    /// The drag objects.
    drags: Vec<XdgToplevelDragV1>,
}

impl ToplevelDrags {
    /// The protocol version vaht speaks.
    const VERSION: u32 = 1;

    /// Offers the manager on `display`.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        display.create_global::<Compositor, XdgToplevelDragManagerV1, _>(Self::VERSION, ());
        Self::default()
    }

    /// Returns the window attached to the drag of `source`, if there is one.
    pub(crate) fn attached_to(&self, source: &WlDataSource) -> Option<AttachedWindow> {
        self.drags
            .iter()
            .filter_map(|drag| drag.data::<DragData>())
            .find(|data| data.source == *source)
            .and_then(|data| data.attached.lock().ok()?.clone())
    }

    /// Returns whether a drag object was made of `source` already.
    fn has_source(&self, source: &WlDataSource) -> bool {
        self.drags
            .iter()
            .filter_map(|drag| drag.data::<DragData>())
            .any(|data| data.source == *source)
    }

    /// Returns whether `surface` is attached to a drag other than `drag`.
    fn is_attached_elsewhere(&self, surface: &WlSurface, drag: &XdgToplevelDragV1) -> bool {
        self.drags
            .iter()
            .filter(|other| *other != drag)
            .filter_map(|other| other.data::<DragData>())
            .any(|data| {
                data.attached.lock().is_ok_and(|attached| {
                    attached.as_ref().is_some_and(|it| it.surface == *surface)
                })
            })
    }
}

impl GlobalDispatch<XdgToplevelDragManagerV1, ()> for Compositor {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<XdgToplevelDragManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<XdgToplevelDragManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &XdgToplevelDragManagerV1,
        request: xdg_toplevel_drag_manager_v1::Request,
        (): &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_toplevel_drag_manager_v1::Request::GetXdgToplevelDrag { id, data_source } =
            request
        {
            if state.protocols.toplevel_drags.has_source(&data_source) {
                resource.post_error(
                    xdg_toplevel_drag_manager_v1::Error::InvalidSource,
                    "the data source already has a toplevel drag",
                );
                return;
            }
            let drag = data_init.init(
                id,
                DragData {
                    source: data_source,
                    attached: Mutex::new(None),
                },
            );
            state.protocols.toplevel_drags.drags.push(drag);
        }
    }
}

impl Dispatch<XdgToplevelDragV1, DragData> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &XdgToplevelDragV1,
        request: xdg_toplevel_drag_v1::Request,
        data: &DragData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_toplevel_drag_v1::Request::Attach {
            toplevel,
            x_offset,
            y_offset,
        } = request
        {
            let Some(surface) = state.surface_of_toplevel(&toplevel) else {
                return;
            };
            if state
                .protocols
                .toplevel_drags
                .is_attached_elsewhere(&surface, resource)
            {
                resource.post_error(
                    xdg_toplevel_drag_v1::Error::ToplevelAttached,
                    "the window is attached to another drag",
                );
                return;
            }
            tracing::debug!(x_offset, y_offset, "a window attached to a drag");
            if let Ok(mut attached) = data.attached.lock() {
                *attached = Some(AttachedWindow {
                    surface,
                    offset: (x_offset, y_offset).into(),
                });
            }
            state.follow_toplevel_drag();
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, resource: &XdgToplevelDragV1, _: &DragData) {
        state
            .protocols
            .toplevel_drags
            .drags
            .retain(|drag| drag != resource);
    }
}

impl Compositor {
    /// Returns the surface of the window whose `xdg_toplevel` is `toplevel`.
    fn surface_of_toplevel(&self, toplevel: &XdgToplevel) -> Option<WlSurface> {
        self.protocols
            .xdg_shell
            .toplevel_surfaces()
            .iter()
            .find(|surface| surface.xdg_toplevel() == toplevel)
            .map(|surface| surface.wl_surface().clone())
    }
}
