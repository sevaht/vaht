//! Surfaces and buffers: `wl_compositor`, `wl_subcompositor`, `wl_shm`, and
//! the fractional scale clients are told to draw at.

use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::desktop::PopupManager;
use smithay::output::Output;
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::commit_timing::CommitTimerStateUserData;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, TraversalAction,
    add_pre_commit_hook, send_surface_state, with_states, with_surface_tree_downward,
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
    /// Until vaht knows which output the surface is on (its next frame),
    /// it is told the primary output's scale.
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self.outputs.primary().current_scale().fractional_scale();
        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional| fractional.set_preferred_scale(scale));
        });
    }
}

/// Tells every surface of the tree at `root`, and of its popups, to draw at
/// `output`'s scale and transform ([`tell_tree_scale`]).
pub(crate) fn tell_output_scale(root: &WlSurface, output: &Output) {
    tell_tree_scale(root, output);
    for (popup, _) in PopupManager::popups_for_surface(root) {
        tell_tree_scale(popup.wl_surface(), output);
    }
}

/// Tells every surface of the tree at `root` to draw at `output`'s scale
/// and transform: its fractional scale (`wp_fractional_scale_v1`), for
/// clients that scale themselves exactly, and the whole scale above it
/// (`wl_surface.preferred_buffer_scale`) for the others. Both are sent
/// only when they change, so this runs at every frame.
fn tell_tree_scale(root: &WlSurface, output: &Output) {
    let scale = output.current_scale();
    let (fractional, whole) = (scale.fractional_scale(), scale.integer_scale());
    let transform = output.current_transform();
    with_surface_tree_downward(
        root,
        (),
        |_, _, ()| TraversalAction::DoChildren(()),
        |surface, states, ()| {
            send_surface_state(surface, states, whole, transform);
            crate::protocols::color_management::tell_preferred(states, output);
            with_fractional_scale(states, |state| state.set_preferred_scale(fractional));
        },
        |_, _, ()| true,
    );
}
