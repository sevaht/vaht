//! Surfaces and buffers: `wl_compositor`, `wl_subcompositor`, `wl_shm`, and
//! the fractional scale clients are told to draw at.

use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::commit_timing::CommitTimerStateUserData;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, add_pre_commit_hook, with_states,
};
use smithay::wayland::fractional_scale::{FractionalScaleHandler, with_fractional_scale};
use smithay::wayland::shm::{ShmHandler, ShmState};

use smithay::xwayland::XWaylandClientData;

use crate::Compositor;
use crate::client::ClientState;

impl CompositorHandler for Compositor {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.protocols.compositor
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // vaht inserts every client with a ClientState, but Smithay inserts
        // Xwayland's with its own.
        if let Some(xwayland) = client.get_data::<XWaylandClientData>() {
            return &xwayland.compositor_state;
        }
        &client
            .get_data::<ClientState>()
            .expect("every other client is inserted with a ClientState")
            .compositor_state
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        // A commit held until a moment (commit timing) wakes nothing by
        // itself; vaht looks for the moment once the commit is in. This hook
        // runs before Smithay's registers the moment, so it asks only
        // whether the surface was ever timed.
        add_pre_commit_hook::<Self, _>(surface, |compositor, _, surface| {
            compositor
                .fence_waits
                .hold_until_drawn(surface, (&compositor.loop_handle, &compositor.ledger));
            let is_timed = with_states(surface, |states| {
                states.data_map.get::<CommitTimerStateUserData>().is_some()
            });
            if is_timed {
                compositor
                    .frame_pacing
                    .check_soon(&compositor.loop_handle, &compositor.ledger);
            }
        });
    }

    fn destroyed(&mut self, _surface: &WlSurface) {
        self.client_cleanup_pending = true;
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.handle_commit(surface);
    }
}

impl BufferHandler for Compositor {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for Compositor {
    fn shm_state(&self) -> &ShmState {
        &self.protocols.shm
    }
}

impl FractionalScaleHandler for Compositor {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        // Every output is at scale 1 until outputs have their own scales.
        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional| fractional.set_preferred_scale(1.0));
        });
    }
}
