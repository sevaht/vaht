//! vaht's IPC socket: programs ask it things and tell it what to do
//! (02 §12).
//!
//! [`IpcServer`] listens on a Unix socket beside the Wayland one
//! ([`vaht_ipc::socket_path`]) and keeps an event source per connected
//! client. Each connection reads requests a line at a time and writes a
//! reply line for each, in order ([`Connection`]); what a request does is
//! the compositor's to decide ([`IpcHandler`]). A client that subscribed
//! is sent the lists it subscribed to whenever they change
//! ([`IpcServer::publish`]). Nothing is held on a client's behalf: a
//! connection that closes, fails, or sends or leaves unread more than the
//! protocol allows is dropped.

mod answers;
mod connection;

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use calloop::generic::Generic;
use calloop::{Interest, LoopHandle, Mode, PostAction};
use vaht_ipc::{Event, EventKind, Reply, Request};

use self::connection::{Connection, Progress};
use crate::Compositor;
use crate::event_source::EventSource;
use crate::resources::ResourceLedger;

/// What answers IPC requests: the compositor.
pub(crate) trait IpcHandler {
    /// Carries out `request`, from `client`, and returns the reply, or
    /// that it comes later ([`IpcServer::answer_later`]).
    fn answer(&mut self, request: Request, client: IpcClient) -> Answer;
}

/// An IPC client asking something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IpcClient {
    /// Its connection's number, to answer it later by.
    pub(crate) number: u64,
    /// Its process, if known.
    pub(crate) peer: Option<i32>,
}

/// How a request is answered.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Answer {
    /// With this reply, now.
    Now(Reply),
    /// Later, once the user has answered: the connection's further
    /// requests wait meanwhile, so replies stay in order.
    Later,
}

/// A client connected.
#[derive(Debug)]
struct Served {
    /// Its socket's source.
    _source: EventSource<Compositor>,
    /// Its socket, to send it what it subscribed to.
    stream: UnixStream,
    /// What it has read and is to write; out only while it is served.
    connection: Option<Connection>,
}

/// The IPC socket and the clients connected to it.
#[derive(Debug)]
pub(crate) struct IpcServer {
    /// Where the socket is; removed when the server is dropped.
    path: PathBuf,
    /// Accepts clients.
    _listener: EventSource<Compositor>,
    /// Each connected client, by connection number.
    connections: BTreeMap<u64, Served>,
    /// The number the next connection gets.
    next_number: u64,
    /// Connections that ended during the last dispatch, to be dropped after
    /// it: a source is never dropped from inside its own callback.
    ended: Vec<u64>,
}

impl IpcServer {
    /// Listens at `path`, replacing a socket file left there: the path is
    /// named after a Wayland socket that vaht holds the lock of, so no
    /// other vaht can be using it.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket cannot be made or registered.
    pub(crate) fn open(
        path: PathBuf,
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
    ) -> io::Result<Self> {
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            Ok(()) | Err(_) => {}
        }
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let source = Generic::new(listener, Interest::READ, Mode::Level);
        let listener = EventSource::insert(handle, ledger, source, |_, listener, compositor| {
            Self::accept_all(listener, compositor);
            Ok(PostAction::Continue)
        })
        .map_err(|error| io::Error::other(error.error))?;
        Ok(Self {
            path,
            _listener: listener,
            connections: BTreeMap::new(),
            next_number: 0,
            ended: Vec::new(),
        })
    }

    /// Returns where the socket is.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Drops the connections that ended.
    pub(crate) fn release_ended(&mut self) {
        for number in self.ended.drain(..) {
            self.connections.remove(&number);
        }
    }

    /// Returns the lists some client subscribed to.
    pub(crate) fn subscribed(&self) -> BTreeSet<EventKind> {
        self.connections
            .values()
            .filter_map(|served| served.connection.as_ref())
            .flat_map(Connection::subscriptions)
            .collect()
    }

    /// Sends `event` to each client subscribed to it whose last copy
    /// differs; a client that can take no more is dropped.
    pub(crate) fn publish(&mut self, event: &Event) {
        let mut line = serde_json::to_vec(event).expect("events are plain data, always encodable");
        line.push(b'\n');
        let kind = event.kind();
        self.connections.retain(|number, served| {
            let Some(connection) = &mut served.connection else {
                return true;
            };
            match connection.push(kind, &line, &served.stream) {
                Progress::Open => true,
                Progress::Ended(reason) => {
                    tracing::debug!(number, reason, "IPC client dropped");
                    false
                }
            }
        });
    }

    /// Returns whether connection `number` is still open.
    pub(crate) fn is_connected(&self, number: u64) -> bool {
        self.connections.contains_key(&number) && !self.ended.contains(&number)
    }

    /// Sends connection `number`, of `compositor`'s server, the reply it
    /// was told comes later, then answers what it asked meanwhile. Nothing
    /// is sent to a connection gone.
    pub(crate) fn answer_later(compositor: &mut Compositor, number: u64, reply: &Reply) {
        let Some(mut connection) = compositor
            .ipc
            .as_mut()
            .and_then(|server| server.take(number))
        else {
            return;
        };
        let progress = connection.resume(reply, compositor);
        let Some(server) = &mut compositor.ipc else {
            return;
        };
        let progress = match (progress, server.connections.get(&number)) {
            (Progress::Open, Some(client)) => connection.send(&client.stream),
            (progress, _) => progress,
        };
        match progress {
            Progress::Open => server.put_back(number, connection),
            Progress::Ended(reason) => {
                tracing::debug!(number, reason, "IPC client dropped");
                server.ended.push(number);
            }
        }
    }

    /// Takes connection `number` out to serve it.
    fn take(&mut self, number: u64) -> Option<Connection> {
        self.connections.get_mut(&number)?.connection.take()
    }

    /// Puts connection `number` back after serving it.
    fn put_back(&mut self, number: u64, connection: Connection) {
        if let Some(served) = self.connections.get_mut(&number) {
            served.connection = Some(connection);
        }
    }

    /// Accepts every client waiting on `listener`.
    fn accept_all(listener: &UnixListener, compositor: &mut Compositor) {
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Some(server) = &mut compositor.ipc {
                        server.add(stream, &compositor.loop_handle, &compositor.ledger);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                Err(error) => {
                    tracing::warn!(%error, "could not accept an IPC client");
                    return;
                }
            }
        }
    }

    /// Serves the client connected by `stream`.
    fn add(
        &mut self,
        stream: UnixStream,
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
    ) {
        if let Err(error) = stream.set_nonblocking(true) {
            tracing::warn!(%error, "could not serve an IPC client");
            return;
        }
        let number = self.next_number;
        self.next_number += 1;
        // The process at the other end, which commands may depend on.
        let peer =
            nix::sys::socket::getsockopt(&stream, nix::sys::socket::sockopt::PeerCredentials)
                .ok()
                .map(|credentials| credentials.pid());
        let connection = Connection::new(number, peer, ledger);
        let pushing = match stream.try_clone() {
            Ok(pushing) => pushing,
            Err(error) => {
                tracing::warn!(%error, "could not serve an IPC client");
                return;
            }
        };
        // Edge-triggered: each wake-up reads and writes until the socket
        // would block, and writing waits for the socket to drain without
        // waking the loop while there is nothing to write.
        let source = Generic::new(stream, Interest::BOTH, Mode::Edge);
        let inserted = EventSource::insert(handle, ledger, source, move |_, stream, compositor| {
            let Some(mut connection) = compositor
                .ipc
                .as_mut()
                .and_then(|server| server.take(number))
            else {
                return Ok(PostAction::Remove);
            };
            let progress = connection.serve(stream, compositor);
            let Some(server) = &mut compositor.ipc else {
                return Ok(PostAction::Remove);
            };
            Ok(match progress {
                Progress::Open => {
                    server.put_back(number, connection);
                    PostAction::Continue
                }
                Progress::Ended(reason) => {
                    tracing::debug!(number, reason, "IPC client gone");
                    server.ended.push(number);
                    PostAction::Remove
                }
            })
        });
        match inserted {
            Ok(source) => {
                tracing::debug!(number, "IPC client connected");
                self.connections.insert(
                    number,
                    Served {
                        _source: source,
                        stream: pushing,
                        connection: Some(connection),
                    },
                );
            }
            Err(error) => tracing::warn!(
                error = &error.error as &dyn std::error::Error,
                "could not serve an IPC client"
            ),
        }
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.path) {
            tracing::warn!(%error, path = %self.path.display(), "could not remove the IPC socket");
        }
    }
}
