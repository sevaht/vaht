//! Waiting for the GPU before showing a client's frame.
//!
//! A client's GPU buffer may still be being drawn when it commits it. vaht
//! holds the commit until the drawing is done: until the acquire point the
//! client named, with explicit sync (`wp_linux_drm_syncobj_v1`, which
//! NVIDIA's driver relies on), or else until the buffer's own fence says
//! it may be read (implicit sync). Shown sooner, a frame could tear or
//! flash half drawn.

use std::cell::Cell;
use std::rc::Rc;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::reexports::calloop::{EventSource as LoopSource, Interest, LoopHandle};
use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::{
    Blocker, BufferAssignment, CompositorHandler as _, SurfaceAttributes, add_blocker, with_states,
};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::drm_syncobj::DrmSyncobjCachedState;

use crate::Compositor;
use crate::event_source::EventSource;
use crate::resources::ResourceLedger;

/// A wait for one buffer's fence.
#[derive(Debug)]
struct FenceWait {
    /// The fence's registration; it removes itself once it fires.
    _source: EventSource<Compositor>,
    /// Set once it has fired, when it may be dropped.
    fired: Rc<Cell<bool>>,
}

/// The waits for buffers' fences under way.
#[derive(Debug, Default)]
pub(crate) struct FenceWaits {
    /// Each wait, fired or not; fired ones are dropped on the next wait.
    waits: Vec<FenceWait>,
}

impl FenceWaits {
    /// Holds `surface`'s commit, if it attaches a GPU buffer, until the GPU
    /// has drawn it. Called as the commit arrives, before it is applied.
    pub(crate) fn hold_until_drawn(
        &mut self,
        surface: &WlSurface,
        (handle, ledger): (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) {
        // Never inside a wait's own callback: commits arrive from clients.
        self.waits.retain(|wait| !wait.fired.get());
        let (dmabuf, acquire_point) = with_states(surface, |states| {
            let acquire_point = states
                .cached_state
                .get::<DrmSyncobjCachedState>()
                .pending()
                .acquire_point
                .clone();
            let dmabuf = states
                .cached_state
                .get::<SurfaceAttributes>()
                .pending()
                .buffer
                .as_ref()
                .and_then(|assignment| match assignment {
                    BufferAssignment::NewBuffer(buffer) => get_dmabuf(buffer).ok().cloned(),
                    BufferAssignment::Removed => None,
                });
            (dmabuf, acquire_point)
        });
        let Some(dmabuf) = dmabuf else {
            return;
        };
        let is_held = acquire_point
            .and_then(|point| point.generate_blocker().ok())
            .is_some_and(|(blocker, source)| self.hold(surface, blocker, source, (handle, ledger)));
        if !is_held {
            self.hold_implicitly(surface, &dmabuf, (handle, ledger));
        }
    }

    /// Holds `surface`'s commit until `dmabuf`'s own fence says it may be
    /// read.
    fn hold_implicitly(
        &mut self,
        surface: &WlSurface,
        dmabuf: &Dmabuf,
        loop_and_ledger: (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) {
        if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) {
            let _ = self.hold(surface, blocker, source, loop_and_ledger);
        }
    }

    /// Places `blocker` on `surface`'s commit, lifted when `source` fires;
    /// returns whether it could.
    fn hold<B, S>(
        &mut self,
        surface: &WlSurface,
        blocker: B,
        source: S,
        (handle, ledger): (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) -> bool
    where
        B: Blocker + Send + 'static,
        S: LoopSource<Event = (), Ret = Result<(), std::io::Error>> + 'static,
    {
        let Some(client) = surface.client() else {
            return false;
        };
        let fired = Rc::new(Cell::new(false));
        let fired_in_callback = Rc::clone(&fired);
        let inserted = EventSource::insert(handle, ledger, source, move |(), _, compositor| {
            fired_in_callback.set(true);
            let display = compositor.display_handle.clone();
            compositor
                .client_compositor_state(&client)
                .blocker_cleared(compositor, &display);
            Ok(())
        });
        match inserted {
            Ok(source) => {
                add_blocker(surface, blocker);
                self.waits.push(FenceWait {
                    _source: source,
                    fired,
                });
                true
            }
            Err(error) => {
                tracing::debug!(error = %error.error, "a buffer's fence could not be waited on");
                false
            }
        }
    }
}
