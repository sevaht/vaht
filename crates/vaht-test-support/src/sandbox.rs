//! A sandbox engine's view of vaht (`wp_security_context_v1`), as tests
//! drive it.
//!
//! [`Sandbox`] asks vaht for a socket of its own, as Flatpak does for the
//! programs it runs; [`Sandbox::globals`] lists what a program connecting
//! there is offered.

use std::os::fd::{AsFd as _, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::wl_registry;
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, EventQueue, QueueHandle};
use smithay_client_toolkit::reexports::protocols::wp::security_context::v1::client::wp_security_context_manager_v1::WpSecurityContextManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::security_context::v1::client::wp_security_context_v1::WpSecurityContextV1;

/// A socket vaht accepts sandboxed programs on, while this lives.
#[derive(Debug)]
pub struct Sandbox {
    /// The directory holding the socket.
    directory: tempfile::TempDir,
    /// The end whose closing tells vaht the sandbox is gone.
    close: Option<OwnedFd>,
    /// The connection that made the sandbox.
    queue: EventQueue<Registry>,
}

/// Receives the registry's events; the global list is all that is read.
#[derive(Debug)]
struct Registry;

impl Sandbox {
    /// The socket's name in its directory.
    const SOCKET: &str = "sandbox.sock";

    /// Asks vaht at `socket` for a sandbox's socket, as the sandbox engine
    /// `engine`.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot be reached or offers no security contexts.
    #[must_use]
    pub fn create(socket: &Path, engine: &str) -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let listener =
            UnixListener::bind(directory.path().join(Self::SOCKET)).expect("a socket to listen on");
        let (close_read, close) = nix::unistd::pipe().expect("a pipe");
        let stream = UnixStream::connect(socket).expect("vaht's socket accepts");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, mut queue) =
            registry_queue_init::<Registry>(&connection).expect("the registry is read");
        let manager: WpSecurityContextManagerV1 = globals
            .bind(&queue.handle(), 1..=1, ())
            .expect("vaht offers wp_security_context_manager_v1");
        let context =
            manager.create_listener(listener.as_fd(), close_read.as_fd(), &queue.handle(), ());
        context.set_sandbox_engine(engine.to_owned());
        context.set_app_id("vaht.test.sandboxed".to_owned());
        context.commit();
        queue.roundtrip(&mut Registry).expect("vaht answers");
        Self {
            directory,
            close: Some(close),
            queue,
        }
    }

    /// Returns the path sandboxed programs connect to.
    #[must_use]
    pub fn socket(&self) -> PathBuf {
        self.directory.path().join(Self::SOCKET)
    }

    /// Returns the interfaces vaht offers a program connecting at
    /// `socket`, sorted.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot be reached there.
    #[must_use]
    pub fn globals(socket: &Path) -> Vec<String> {
        let stream = UnixStream::connect(socket).expect("the socket accepts");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, _queue) =
            registry_queue_init::<Registry>(&connection).expect("the registry is read");
        let mut names: Vec<String> = globals
            .contents()
            .clone_list()
            .into_iter()
            .map(|global| global.interface)
            .collect();
        names.sort();
        names
    }

    /// Closes the sandbox, as its engine does when its programs end.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn close(&mut self) {
        self.close = None;
        self.queue.roundtrip(&mut Registry).expect("vaht answers");
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Registry {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpSecurityContextManagerV1, ()> for Registry {
    fn event(
        _: &mut Self,
        _: &WpSecurityContextManagerV1,
        _: <WpSecurityContextManagerV1 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpSecurityContextV1, ()> for Registry {
    fn event(
        _: &mut Self,
        _: &WpSecurityContextV1,
        _: <WpSecurityContextV1 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
