//! A blocking client: connect, ask, read the reply.

use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::message::{Event, EventKind, Reply, Request};
use crate::{MAX_MESSAGE_BYTES, SOCKET_VARIABLE};

/// A connection to vaht's IPC socket.
#[derive(Debug)]
pub struct Client {
    /// Replies, read a line at a time.
    reader: BufReader<UnixStream>,
    /// Requests, written a line at a time.
    writer: UnixStream,
}

impl Client {
    /// Connects to the vaht listening at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if nothing listens there.
    pub fn connect(path: &Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path).map_err(|source| ClientError::Connect {
            path: path.to_owned(),
            source,
        })?;
        let writer = stream.try_clone().map_err(ClientError::Io)?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
        })
    }

    /// Connects to the vaht this program runs under: the one
    /// [`SOCKET_VARIABLE`] names (vaht sets it for the programs it starts),
    /// or else the one serving `WAYLAND_DISPLAY`.
    ///
    /// # Errors
    ///
    /// Returns an error if neither says where vaht is, or nothing listens
    /// there.
    pub fn connect_from_environment() -> Result<Self, ClientError> {
        let named = std::env::var_os(SOCKET_VARIABLE).map(PathBuf::from);
        let path = named
            .or_else(|| {
                let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
                let display = std::env::var_os("WAYLAND_DISPLAY")?;
                Some(crate::socket_path(Path::new(&runtime), &display))
            })
            .ok_or(ClientError::NoSocket)?;
        Self::connect(&path)
    }

    /// Sends `request` and returns vaht's reply.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails, or vaht closes it or sends
    /// something that is not a reply.
    ///
    /// # Panics
    ///
    /// Never: [`MAX_MESSAGE_BYTES`] fits in a `u64`, and requests are plain
    /// data that always encode.
    pub fn request(&mut self, request: &Request) -> Result<Reply, ClientError> {
        let mut line = serde_json::to_vec(request).map_err(ClientError::Encode)?;
        line.push(b'\n');
        self.writer.write_all(&line).map_err(ClientError::Io)?;
        let reply = self.read_line()?;
        serde_json::from_slice(&reply).map_err(ClientError::Decode)
    }

    /// Subscribes to the lists `events` names; [`Client::next_event`] then
    /// reads them, and nothing but them should be asked here after.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails, or vaht refuses.
    pub fn subscribe(&mut self, events: Vec<EventKind>) -> Result<(), ClientError> {
        match self.request(&Request::Subscribe { events })? {
            Reply::Done {} => Ok(()),
            Reply::Error { message } => Err(ClientError::Refused(message)),
            other @ (Reply::Version { .. }
            | Reply::Windows { .. }
            | Reply::Workspaces { .. }
            | Reply::Outputs { .. }) => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Waits for the next list subscribed to.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails, or vaht closes it or sends
    /// something that is not an event.
    pub fn next_event(&mut self) -> Result<Event, ClientError> {
        let line = self.read_line()?;
        serde_json::from_slice(&line).map_err(ClientError::Decode)
    }

    /// Reads one line vaht sent, its newline included.
    ///
    /// # Panics
    ///
    /// Never: [`MAX_MESSAGE_BYTES`] fits in a `u64`.
    fn read_line(&mut self) -> Result<Vec<u8>, ClientError> {
        let mut line = Vec::new();
        let limit = u64::try_from(MAX_MESSAGE_BYTES).expect("the limit fits in u64");
        (&mut self.reader)
            .take(limit)
            .read_until(b'\n', &mut line)
            .map_err(ClientError::Io)?;
        if line.last() != Some(&b'\n') {
            return Err(ClientError::Closed);
        }
        Ok(line)
    }
}

/// Why talking to vaht failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Neither [`SOCKET_VARIABLE`] nor `WAYLAND_DISPLAY` (with
    /// `XDG_RUNTIME_DIR`) is set: this program is not running under vaht.
    #[error("neither {SOCKET_VARIABLE} nor WAYLAND_DISPLAY is set; is this running under vaht?")]
    NoSocket,
    /// Nothing listens at the socket.
    #[error("could not connect to vaht at {}", path.display())]
    Connect {
        /// Where vaht was looked for.
        path: PathBuf,
        /// Why connecting failed.
        #[source]
        source: io::Error,
    },
    /// Reading or writing failed.
    #[error("could not talk to vaht")]
    Io(#[source] io::Error),
    /// vaht closed the connection before replying in full.
    #[error("vaht closed the connection without replying")]
    Closed,
    /// The request could not be written as JSON.
    #[error("could not encode the request")]
    Encode(#[source] serde_json::Error),
    /// vaht's reply was not one this client understands.
    #[error("vaht's reply could not be read")]
    Decode(#[source] serde_json::Error),
    /// vaht refused the request.
    #[error("vaht refused: {0}")]
    Refused(String),
    /// vaht replied with something other than what the request asks for.
    #[error("vaht replied unexpectedly: {0}")]
    Unexpected(String),
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    #[test]
    fn a_request_gets_the_reply_written_back() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("socket");
        let listener = UnixListener::bind(&path).expect("the socket binds");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("a client connects");
            let mut line = String::new();
            BufReader::new(&stream)
                .read_line(&mut line)
                .expect("a request arrives");
            (&stream)
                .write_all(b"{\"reply\":\"version\",\"version\":\"9\"}\n")
                .expect("the reply is written");
            line
        });
        let mut client = Client::connect(&path).expect("connected");
        let reply = client.request(&Request::Version {}).expect("a reply");
        assert_eq!(
            reply,
            Reply::Version {
                version: "9".to_owned()
            }
        );
        let request = server.join().expect("the server thread finishes");
        assert_eq!(request, "{\"request\":\"version\"}\n");
    }

    #[test]
    fn a_closed_connection_is_an_error() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("socket");
        let listener = UnixListener::bind(&path).expect("the socket binds");
        let server = std::thread::spawn(move || drop(listener.accept()));
        let mut client = Client::connect(&path).expect("connected");
        server.join().expect("the server thread finishes");
        assert!(matches!(
            client.request(&Request::Version {}),
            Err(ClientError::Closed | ClientError::Io(_))
        ));
    }
}
