//! The Wayland protocols vaht serves, through Smithay.
//!
//! [`ProtocolStates`] holds Smithay's state for each protocol global. The
//! handler traits Smithay calls are implemented on
//! [`crate::Compositor`] in the submodules, one per protocol
//! family. They are gateways (M4): they translate protocol events into calls
//! on vaht's own types and decide nothing themselves.

mod output;
mod seat;
mod surfaces;
mod xdg_shell;

use smithay::input::SeatState;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;

use crate::Compositor;

/// Smithay's state for every protocol global vaht advertises.
pub(crate) struct ProtocolStates {
    /// `wl_compositor` and `wl_subcompositor`: surfaces and their commits.
    pub(crate) compositor: CompositorState,
    /// `xdg_wm_base`: application windows and popups.
    pub(crate) xdg_shell: XdgShellState,
    /// `wl_shm`: shared-memory buffers.
    pub(crate) shm: ShmState,
    /// `wl_output` and `xdg_output`: what clients know about outputs. Held
    /// only to keep the globals alive.
    _output_manager: OutputManagerState,
    /// `wl_seat`: keyboards and pointers.
    pub(crate) seat: SeatState<Compositor>,
    /// `wl_data_device_manager`: the clipboard and drag and drop.
    pub(crate) data_device: DataDeviceState,
}

impl ProtocolStates {
    /// Creates every global on `display`.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        Self {
            compositor: CompositorState::new::<Compositor>(display),
            xdg_shell: XdgShellState::new::<Compositor>(display),
            shm: ShmState::new::<Compositor>(display, Vec::new()),
            _output_manager: OutputManagerState::new_with_xdg_output::<Compositor>(display),
            seat: SeatState::new(),
            data_device: DataDeviceState::new::<Compositor>(display),
        }
    }
}

smithay::delegate_dispatch2!(Compositor);
