//! One IPC client's connection: requests in, replies out, a line each;
//! and the lists it subscribed to, sent whenever they change.

use std::collections::BTreeMap;
use std::io::{self, Read as _, Write as _};
use std::os::unix::net::UnixStream;

use vaht_ipc::{EventKind, MAX_MESSAGE_BYTES, Reply, Request};

use super::{Answer, IpcClient, IpcHandler};
use crate::resources::{ResourceKind, ResourceLedger, Tracked};

/// Whether a connection goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Progress {
    /// It is still open.
    Open,
    /// It ended, for this reason.
    Ended(&'static str),
}

/// What a connection has read but not answered, and answered but not sent.
#[derive(Debug)]
pub(super) struct Connection {
    /// Bytes read after the last complete request.
    input: Vec<u8>,
    /// Replies not yet written.
    output: Vec<u8>,
    /// Its number, by which a reply that came later finds it.
    number: u64,
    /// The process at the other end, if known.
    peer: Option<i32>,
    /// Whether a reply is to come later: requests after it wait.
    is_waiting: bool,
    /// Each list subscribed to, and the line last sent of it, if any.
    subscriptions: BTreeMap<EventKind, Option<Vec<u8>>>,
    /// Counts the connection while it lives.
    _tracked: Tracked,
}

impl Connection {
    /// The most replies a client may leave unread, in bytes, before it is
    /// dropped: a client that never reads would otherwise grow this
    /// without bound (03 §1).
    const MAX_UNSENT_BYTES: usize = 16 * MAX_MESSAGE_BYTES;

    /// Creates empty connection `number` to the process `peer`, counted in
    /// `ledger`.
    pub(super) fn new(number: u64, peer: Option<i32>, ledger: &ResourceLedger) -> Self {
        Self {
            input: Vec::new(),
            output: Vec::new(),
            number,
            peer,
            is_waiting: false,
            subscriptions: BTreeMap::new(),
            _tracked: ledger.track(ResourceKind::IpcConnection),
        }
    }

    /// Returns the lists the client subscribed to.
    pub(super) fn subscriptions(&self) -> impl Iterator<Item = EventKind> + '_ {
        self.subscriptions.keys().copied()
    }

    /// Sends `line`, the list `kind` as it is now, unless the client did
    /// not subscribe to it or was last sent the same; writes what `stream`
    /// takes.
    pub(super) fn push(&mut self, kind: EventKind, line: &[u8], stream: &UnixStream) -> Progress {
        let Some(last) = self.subscriptions.get_mut(&kind) else {
            return Progress::Open;
        };
        if last.as_deref() == Some(line) {
            return Progress::Open;
        }
        *last = Some(line.to_vec());
        self.output.extend_from_slice(line);
        if self.output.len() > Self::MAX_UNSENT_BYTES {
            return Progress::Ended("left too many events unread");
        }
        self.send(stream)
    }

    /// Reads what `stream` has, answers each complete request with
    /// `handler`, and writes what the stream takes.
    pub(super) fn serve(&mut self, stream: &UnixStream, handler: &mut impl IpcHandler) -> Progress {
        let mut chunk = [0; 4096];
        loop {
            match (&mut &*stream).read(&mut chunk) {
                Ok(0) => return Progress::Ended("closed by the client"),
                Ok(read) => {
                    let progress = self.receive(&chunk[..read], handler);
                    if progress != Progress::Open {
                        return progress;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Progress::Ended("could not read"),
            }
        }
        self.send(stream)
    }

    /// Takes the reply that was to come later, then answers the requests
    /// that waited for it.
    pub(super) fn resume(&mut self, reply: &Reply, handler: &mut impl IpcHandler) -> Progress {
        self.is_waiting = false;
        self.queue(reply);
        self.receive(&[], handler)
    }

    /// Adds `reply` to what is to be written.
    fn queue(&mut self, reply: &Reply) {
        let mut encoded =
            serde_json::to_vec(reply).expect("replies are plain data, always encodable");
        encoded.push(b'\n');
        self.output.extend(encoded);
    }

    /// Takes `bytes` read from the client, answering each request they
    /// complete, until one is to be answered later.
    fn receive(&mut self, bytes: &[u8], handler: &mut impl IpcHandler) -> Progress {
        self.input.extend_from_slice(bytes);
        while !self.is_waiting
            && let Some(end) = self.input.iter().position(|byte| *byte == b'\n')
        {
            let line: Vec<u8> = self.input.drain(..=end).collect();
            let answer = match serde_json::from_slice::<Request>(&line) {
                Ok(Request::Subscribe { events }) => {
                    self.subscriptions = events.into_iter().map(|kind| (kind, None)).collect();
                    Answer::Now(Reply::Done {})
                }
                Ok(request) => handler.answer(
                    request,
                    IpcClient {
                        number: self.number,
                        peer: self.peer,
                    },
                ),
                Err(error) => Answer::Now(Reply::Error {
                    message: format!("not a request: {error}"),
                }),
            };
            match answer {
                Answer::Now(reply) => self.queue(&reply),
                Answer::Later => self.is_waiting = true,
            }
        }
        if self.input.len() >= MAX_MESSAGE_BYTES {
            return Progress::Ended("sent a request longer than the protocol allows");
        }
        if self.output.len() > Self::MAX_UNSENT_BYTES {
            return Progress::Ended("left too many replies unread");
        }
        Progress::Open
    }

    /// Writes as much of the waiting replies as `stream` takes.
    pub(super) fn send(&mut self, stream: &UnixStream) -> Progress {
        while !self.output.is_empty() {
            match (&mut &*stream).write(&self.output) {
                Ok(written) => {
                    self.output.drain(..written);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Progress::Ended("could not write"),
            }
        }
        Progress::Open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers every request with its own debug text, as an error.
    struct Echo;

    impl IpcHandler for Echo {
        fn answer(&mut self, request: Request, _: IpcClient) -> Answer {
            if matches!(request, Request::Pick { .. }) {
                return Answer::Later;
            }
            Answer::Now(Reply::Error {
                message: format!("{request:?}"),
            })
        }
    }

    /// Returns the replies `connection` has waiting, as text.
    fn unsent(connection: &Connection) -> String {
        String::from_utf8(connection.output.clone()).expect("replies are UTF-8")
    }

    #[test]
    fn each_complete_line_gets_one_reply_in_order() {
        let mut connection = Connection::new(0, None, &ResourceLedger::new());
        let progress =
            connection.receive(b"{\"request\":\"version\"}\n{\"request\":\"win", &mut Echo);
        assert_eq!(progress, Progress::Open);
        assert_eq!(unsent(&connection).lines().count(), 1, "half a line waits");
        connection.receive(b"dows\"}\n", &mut Echo);
        let replies = unsent(&connection);
        let lines: Vec<&str> = replies.lines().collect();
        assert!(lines[0].contains("Version"), "{replies}");
        assert!(lines[1].contains("Windows"), "{replies}");
    }

    #[test]
    fn requests_after_one_answered_later_wait_for_it_in_order() {
        let mut connection = Connection::new(0, None, &ResourceLedger::new());
        connection.receive(
            b"{\"request\":\"pick\",\"items\":[\"a\"]}\n{\"request\":\"version\"}\n",
            &mut Echo,
        );
        assert_eq!(
            unsent(&connection),
            "",
            "nothing until the pick is answered"
        );
        connection.resume(
            &Reply::Picked {
                choice: Some("a".to_owned()),
            },
            &mut Echo,
        );
        let replies = unsent(&connection);
        let lines: Vec<&str> = replies.lines().collect();
        assert!(lines[0].contains("picked"), "{replies}");
        assert!(
            lines[1].contains("Version"),
            "then the request that waited: {replies}"
        );
    }

    #[test]
    fn a_line_that_is_not_a_request_gets_an_error_reply() {
        let mut connection = Connection::new(0, None, &ResourceLedger::new());
        connection.receive(b"hello\n", &mut Echo);
        assert!(unsent(&connection).contains("not a request"));
    }

    #[test]
    fn a_subscriber_is_sent_a_list_only_when_it_changed() {
        let (stream, mut peer) = UnixStream::pair().expect("a socket pair");
        stream.set_nonblocking(true).expect("non-blocking");
        let mut connection = Connection::new(0, None, &ResourceLedger::new());
        connection.receive(
            b"{\"request\":\"subscribe\",\"events\":[\"windows\"]}\n",
            &mut Echo,
        );
        assert!(unsent(&connection).contains("done"));
        assert_eq!(
            connection.subscriptions().collect::<Vec<_>>(),
            [EventKind::Windows]
        );
        let windows = b"{\"event\":\"windows\",\"windows\":[]}\n";
        connection.push(EventKind::Windows, windows, &stream);
        connection.push(EventKind::Windows, windows, &stream);
        connection.push(EventKind::Workspaces, b"not subscribed\n", &stream);
        drop(stream);
        let mut received = String::new();
        peer.read_to_string(&mut received).expect("read");
        assert_eq!(
            received, "{\"reply\":\"done\"}\n{\"event\":\"windows\",\"windows\":[]}\n",
            "the reply, then the list once"
        );
    }

    #[test]
    fn an_endless_line_ends_the_connection() {
        let mut connection = Connection::new(0, None, &ResourceLedger::new());
        let progress = connection.receive(&vec![b'x'; MAX_MESSAGE_BYTES], &mut Echo);
        assert!(matches!(progress, Progress::Ended(_)));
    }

    #[test]
    fn a_client_that_never_reads_is_dropped() {
        let mut connection = Connection::new(0, None, &ResourceLedger::new());
        let requests = "{\"request\":\"version\"}\n".repeat(Connection::MAX_UNSENT_BYTES / 10);
        let progress = connection.receive(requests.as_bytes(), &mut Echo);
        assert!(matches!(progress, Progress::Ended(_)));
    }
}
