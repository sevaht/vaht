//! Wayland clients: the socket they connect to, and what vaht keeps for
//! each.
//!
//! [`ClientListener`] opens the socket (`WAYLAND_DISPLAY`) and accepts
//! clients on it; it also wakes the event loop when connected clients send
//! something, which the compositor then dispatches. [`ClientState`] is what
//! each client carries while it is connected: a client that connected from
//! a sandbox's socket ([`crate::protocols`]' security contexts) is
//! restricted, and the globals that would let it read the selections,
//! inject input, capture the screen, list windows, lock the session or
//! draw panels are hidden from it.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::AsFd as _;
use std::sync::Arc;

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::{Client, Display};
use smithay::wayland::compositor::CompositorClientState;
use smithay::wayland::security_context::SecurityContext;
use smithay::wayland::socket::ListeningSocketSource;

use crate::Compositor;
use crate::event_source::EventSource;
use crate::resources::{ResourceKind, ResourceLedger, Tracked};

/// The socket clients connect to, accepting them while it lives.
#[derive(Debug)]
pub(crate) struct ClientListener {
    /// The socket's name, which clients find in `WAYLAND_DISPLAY`.
    socket_name: OsString,
    /// Accepts clients on the socket.
    _socket: EventSource<Compositor>,
    /// Wakes the event loop when connected clients send something.
    _display_wakeup: EventSource<Compositor>,
}

impl ClientListener {
    /// Opens a socket for clients of `display` and registers it, and the
    /// display's wake-up, with the event loop.
    ///
    /// # Errors
    ///
    /// Returns an error if no socket can be opened or the event loop
    /// refuses a registration.
    pub(crate) fn open(
        handle: &LoopHandle<'static, Compositor>,
        display: &Display<Compositor>,
        ledger: &ResourceLedger,
    ) -> Result<Self, ListenError> {
        let socket = ListeningSocketSource::new_auto().map_err(ListenError::Bind)?;
        let socket_name = socket.socket_name().to_os_string();
        let socket = EventSource::insert(handle, ledger, socket, |stream, (), compositor| {
            let state = Arc::new(ClientState::new(&compositor.ledger));
            if let Err(error) = compositor
                .display_handle
                .clone()
                .insert_client(stream, state)
            {
                tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    "could not accept a client"
                );
            }
        })
        .map_err(|error| ListenError::Register(error.error))?;
        // Readiness only wakes the loop; the compositor dispatches clients
        // after every wake-up.
        let display_descriptor = display
            .as_fd()
            .try_clone_to_owned()
            .map_err(ListenError::DuplicateDisplayDescriptor)?;
        let display_wakeup = EventSource::insert(
            handle,
            ledger,
            Generic::new(display_descriptor, Interest::READ, Mode::Level),
            |_, _, _| Ok(PostAction::Continue),
        )
        .map_err(|error| ListenError::Register(error.error))?;
        Ok(Self {
            socket_name,
            _socket: socket,
            _display_wakeup: display_wakeup,
        })
    }

    /// Returns the socket's name, which clients find in `WAYLAND_DISPLAY`.
    pub(crate) fn socket_name(&self) -> &OsStr {
        &self.socket_name
    }
}

/// Why vaht could not listen for clients.
#[derive(Debug, thiserror::Error)]
pub enum ListenError {
    /// No socket for clients could be opened.
    #[error("could not open a socket for clients")]
    Bind(#[source] smithay::reexports::wayland_server::BindError),

    /// The display's descriptor could not be duplicated for the event loop.
    #[error("could not watch the Wayland display for client activity")]
    DuplicateDisplayDescriptor(#[source] io::Error),

    /// The socket or the display's descriptor could not be registered.
    #[error("could not register with the event loop")]
    Register(#[source] calloop::Error),
}

/// Per-client state; Wayland drops it when the client disconnects.
#[derive(Debug)]
pub(crate) struct ClientState {
    /// Smithay's per-client compositor bookkeeping.
    pub(crate) compositor_state: CompositorClientState,
    /// The sandbox the client connected from, if any
    /// (`wp_security_context_v1`).
    security_context: Option<SecurityContext>,
    /// Counts the client while it is connected.
    _tracked: Tracked,
}

impl ClientState {
    /// Creates the state of a newly connected client.
    pub(crate) fn new(ledger: &ResourceLedger) -> Self {
        Self {
            compositor_state: CompositorClientState::default(),
            security_context: None,
            _tracked: ledger.track(ResourceKind::Client),
        }
    }

    /// Creates the state of a client newly connected from the sandbox
    /// `context`.
    pub(crate) fn sandboxed(ledger: &ResourceLedger, context: SecurityContext) -> Self {
        Self {
            security_context: Some(context),
            ..Self::new(ledger)
        }
    }

    /// Returns whether `client` may use what sandboxed programs may not:
    /// it did not connect from a sandbox. Xwayland, which vaht starts, may.
    pub(crate) fn is_unrestricted(client: &Client) -> bool {
        client
            .get_data::<Self>()
            .is_none_or(|state| state.security_context.is_none())
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
