//! What vaht keeps for each connected Wayland client.

use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::wayland::compositor::CompositorClientState;

use crate::resources::{ResourceKind, ResourceLedger, Tracked};

/// Per-client state; Wayland drops it when the client disconnects.
#[derive(Debug)]
pub(crate) struct ClientState {
    /// Smithay's per-client compositor bookkeeping.
    pub(crate) compositor_state: CompositorClientState,
    /// Counts the client while it is connected.
    _tracked: Tracked,
}

impl ClientState {
    /// Creates the state of a newly connected client.
    pub(crate) fn new(ledger: &ResourceLedger) -> Self {
        Self {
            compositor_state: CompositorClientState::default(),
            _tracked: ledger.track(ResourceKind::Client),
        }
    }
}

impl ClientData for ClientState {
    fn initialized(&self, client: ClientId) {
        tracing::debug!(?client, "client connected");
    }

    fn disconnected(&self, client: ClientId, reason: DisconnectReason) {
        tracing::debug!(?client, ?reason, "client disconnected");
    }
}
