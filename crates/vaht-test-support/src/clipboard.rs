//! A clipboard manager, as tests drive one: `ext_data_control_v1`, which
//! `wl-copy`, `wl-paste --watch` and clipboard histories use.
//!
//! [`ClipboardManager`] copies text to the clipboard or the primary
//! selection, and pastes what is there, without a window or the keyboard:
//! it reads and writes the selection whoever holds it, Wayland client or X
//! program.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::os::fd::{AsFd as _, OwnedFd};
use std::path::Path;
use std::time::{Duration, Instant};

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use smithay_client_toolkit::reexports::client::backend::ObjectId;
use smithay_client_toolkit::reexports::client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::{wl_registry, wl_seat};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, EventQueue, Proxy as _, QueueHandle, event_created_child};
use smithay_client_toolkit::reexports::protocols::ext::data_control::v1::client::ext_data_control_device_v1::{self, ExtDataControlDeviceV1};
use smithay_client_toolkit::reexports::protocols::ext::data_control::v1::client::ext_data_control_manager_v1::ExtDataControlManagerV1;
use smithay_client_toolkit::reexports::protocols::ext::data_control::v1::client::ext_data_control_offer_v1::{self, ExtDataControlOfferV1};
use smithay_client_toolkit::reexports::protocols::ext::data_control::v1::client::ext_data_control_source_v1::{self, ExtDataControlSourceV1};

use crate::wait::PATIENCE;

/// The type text is offered and asked for as.
const TEXT: &str = "text/plain;charset=utf-8";

/// Which selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// The clipboard: what Ctrl+C copies.
    Clipboard,
    /// The primary selection: what was last selected, pasted with the
    /// middle button.
    Primary,
}

/// A clipboard manager's connection to vaht.
#[derive(Debug)]
pub struct ClipboardManager {
    /// The events vaht sends.
    queue: EventQueue<ManagerState>,
    /// What the events said.
    state: ManagerState,
    /// The manager, which makes sources.
    manager: ExtDataControlManagerV1,
    /// The seat's selections.
    device: ExtDataControlDeviceV1,
}

/// What vaht's events said.
#[derive(Debug, Default)]
struct ManagerState {
    /// Each offer's types, by offer.
    types: HashMap<ObjectId, Vec<String>>,
    /// The clipboard's offer, if anything is on it.
    clipboard: Option<ExtDataControlOfferV1>,
    /// The primary selection's offer, if anything is selected.
    primary: Option<ExtDataControlOfferV1>,
    /// The text each of this manager's sources holds, by source.
    copied: HashMap<ObjectId, String>,
    /// How many of this manager's sources were replaced.
    cancelled: usize,
}

impl ClipboardManager {
    /// Connects to vaht at `socket` as a clipboard manager.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot be reached or does not offer data control.
    #[must_use]
    pub fn connect(socket: &Path) -> Self {
        let stream =
            std::os::unix::net::UnixStream::connect(socket).expect("vaht's socket accepts");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, mut queue) =
            registry_queue_init::<ManagerState>(&connection).expect("the registry is read");
        let handle = queue.handle();
        let seat: wl_seat::WlSeat = bind(&globals, &handle, 1);
        let manager: ExtDataControlManagerV1 = bind(&globals, &handle, 1);
        let device = manager.get_data_device(&seat, &handle, ());
        let mut state = ManagerState::default();
        queue.roundtrip(&mut state).expect("vaht answers");
        Self {
            queue,
            state,
            manager,
            device,
        }
    }

    /// Puts `text` on `selection`, and waits until vaht has it.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn copy(&mut self, selection: Selection, text: &str) {
        let handle = self.queue.handle();
        let source = self.manager.create_data_source(&handle, ());
        source.offer(TEXT.to_owned());
        source.offer("text/plain".to_owned());
        source.offer("UTF8_STRING".to_owned());
        self.state.copied.insert(source.id(), text.to_owned());
        match selection {
            Selection::Clipboard => self.device.set_selection(Some(&source)),
            Selection::Primary => self.device.set_primary_selection(Some(&source)),
        }
        self.queue.roundtrip(&mut self.state).expect("vaht answers");
    }

    /// Returns the text on `selection`, if there is any; it waits for
    /// whoever holds it to give it, answering meanwhile if it is this
    /// manager.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails, or the text does not come in
    /// time.
    pub fn paste(&mut self, selection: Selection) -> Option<String> {
        self.queue.roundtrip(&mut self.state).expect("vaht answers");
        let offer = match selection {
            Selection::Clipboard => self.state.clipboard.clone(),
            Selection::Primary => self.state.primary.clone(),
        }?;
        let types = self
            .state
            .types
            .get(&offer.id())
            .cloned()
            .unwrap_or_default();
        let wanted = [TEXT, "text/plain", "UTF8_STRING", "STRING"]
            .into_iter()
            .find(|wanted| types.iter().any(|offered| offered == wanted))?;
        let (reader, writer) = nix::unistd::pipe().expect("a pipe");
        offer.receive(wanted.to_owned(), writer.as_fd());
        drop(writer);
        self.queue.flush().expect("vaht answers");
        Some(self.read_answering(&reader))
    }

    /// Runs `work` on its own thread, answering meanwhile whoever pastes
    /// what this manager copied; returns what `work` returns.
    ///
    /// # Panics
    ///
    /// Panics if `work` panics, or the connection to vaht fails.
    pub fn answer_while<T: Send>(&mut self, work: impl FnOnce() -> T + Send) -> T {
        let (done, finished) = nix::unistd::pipe().expect("a pipe");
        std::thread::scope(|scope| {
            let worker = scope.spawn(move || {
                let result = work();
                drop(finished);
                result
            });
            // The pipe holds nothing: it reads to its end when `work` ends.
            self.read_answering(&done);
            worker.join().expect("the work finishes")
        })
    }

    /// Returns how many of this manager's copies were replaced by another
    /// program's.
    #[must_use]
    pub const fn replaced(&self) -> usize {
        self.state.cancelled
    }

    /// Waits until the text on `selection` is `expected`, pasting each
    /// time vaht says the selection changed; returns whether it came.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn wait_for_text(&mut self, selection: Selection, expected: &str) -> bool {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if self.paste(selection).as_deref() == Some(expected) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            self.wait_for_events(left.min(Duration::from_millis(500)));
        }
    }

    /// Reads `reader` to its end, while answering vaht's events (this
    /// manager may be who gives the text).
    fn read_answering(&mut self, reader: &OwnedFd) -> String {
        let mut file = std::fs::File::from(reader.try_clone().expect("the pipe can be shared"));
        let mut text = Vec::new();
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.queue
                .dispatch_pending(&mut self.state)
                .expect("vaht answers");
            self.queue.flush().expect("vaht answers");
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the pipe was never closed");
            let Some(guard) = self.queue.prepare_read() else {
                continue;
            };
            let mut fds = [
                PollFd::new(guard.connection_fd(), PollFlags::POLLIN),
                PollFd::new(reader.as_fd(), PollFlags::POLLIN),
            ];
            let millis = u16::try_from(left.as_millis()).unwrap_or(u16::MAX);
            poll(&mut fds, PollTimeout::from(millis)).expect("the pipe can be waited on");
            let is_ready = |fd: &PollFd<'_>| fd.revents().is_some_and(|events| !events.is_empty());
            let (is_event, is_text) = (is_ready(&fds[0]), is_ready(&fds[1]));
            if is_event {
                let _ = guard.read();
            } else {
                drop(guard);
            }
            if is_text {
                // Readable: this read does not block, and reads nothing
                // only at the pipe's end.
                let mut chunk = [0; 4096];
                let read = file.read(&mut chunk).expect("the selection can be read");
                if read == 0 {
                    break;
                }
                text.extend_from_slice(&chunk[..read]);
            }
        }
        String::from_utf8(text).expect("the selection is text")
    }

    /// Reads what vaht sends for at most `patience`.
    fn wait_for_events(&mut self, patience: Duration) {
        self.queue.flush().expect("vaht answers");
        if let Some(guard) = self.queue.prepare_read() {
            let mut fds = [PollFd::new(guard.connection_fd(), PollFlags::POLLIN)];
            let millis = u16::try_from(patience.as_millis()).unwrap_or(u16::MAX);
            if poll(&mut fds, PollTimeout::from(millis)).unwrap_or(0) > 0 {
                let _ = guard.read();
            }
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .expect("vaht answers");
    }
}

/// Binds the global of interface `I` at `version`.
fn bind<I>(globals: &GlobalList, handle: &QueueHandle<ManagerState>, version: u32) -> I
where
    I: smithay_client_toolkit::reexports::client::Proxy + 'static,
    ManagerState: Dispatch<I, ()>,
{
    globals
        .bind(handle, version..=version, ())
        .unwrap_or_else(|error| panic!("vaht offers {}: {error}", I::interface().name))
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ManagerState {
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

impl Dispatch<wl_seat::WlSeat, ()> for ManagerState {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtDataControlManagerV1, ()> for ManagerState {
    fn event(
        _: &mut Self,
        _: &ExtDataControlManagerV1,
        _: <ExtDataControlManagerV1 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for ManagerState {
    fn event(
        state: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "the device's ending, and events of later versions, need nothing"
        )]
        match event {
            ext_data_control_device_v1::Event::DataOffer { id } => {
                state.types.insert(id.id(), Vec::new());
            }
            ext_data_control_device_v1::Event::Selection { id } => {
                let replaced = std::mem::replace(&mut state.clipboard, id);
                state.forget(replaced);
            }
            ext_data_control_device_v1::Event::PrimarySelection { id } => {
                let replaced = std::mem::replace(&mut state.primary, id);
                state.forget(replaced);
            }
            _ => {}
        }
    }

    event_created_child!(ManagerState, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl ManagerState {
    /// Lets go of `offer`, which vaht replaced.
    fn forget(&mut self, offer: Option<ExtDataControlOfferV1>) {
        if let Some(offer) = offer {
            self.types.remove(&offer.id());
            offer.destroy();
        }
    }
}

impl Dispatch<ExtDataControlOfferV1, ()> for ManagerState {
    fn event(
        state: &mut Self,
        proxy: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            state.types.entry(proxy.id()).or_default().push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for ManagerState {
    fn event(
        state: &mut Self,
        proxy: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { fd, .. } => {
                if let Some(text) = state.copied.get(&proxy.id()) {
                    let mut pipe = std::fs::File::from(fd);
                    // A reader that left early is no failure of the copy.
                    let _ = pipe.write_all(text.as_bytes());
                }
            }
            ext_data_control_source_v1::Event::Cancelled => {
                state.copied.remove(&proxy.id());
                state.cancelled += 1;
                proxy.destroy();
            }
            _ => {}
        }
    }
}
