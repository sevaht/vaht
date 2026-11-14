//! Input emulation for X11 programs: an EIS server for Xwayland (fluxland's
//! `session.xwayland.inputEmulation`).
//!
//! An X program moves the pointer and presses keys through the XTest
//! extension (Steam Input's desktop controls, xdotool, remote desktop
//! servers). Xwayland cannot do that on Wayland by itself; it passes each
//! program's XTest input to the EIS socket in `LIBEI_SOCKET`, which vaht
//! serves, naming the context after the program. Only vaht's own Xwayland
//! may connect ([`policy::is_xwayland`]); each program is let in, refused
//! or asked about as `[xwayland] input_emulation` says ([`policy`]).
//!
//! [`InputEmulation`] holds the socket and each program's connection, and
//! turns what arrives into [`Emulated`] input for the compositor, which
//! feeds it in as it does a real device's. Keys and buttons a program holds
//! are let go when it stops or leaves ([`held`]).

mod held;
pub(crate) mod policy;

use std::os::fd::AsFd as _;
use std::path::{Path, PathBuf};

use reis::calloop::{EisListenerSource, EisRequestSource, EisRequestSourceEvent};
use reis::eis;
use reis::request::{self, Connection, DeviceCapability, EisRequest, Seat};
use smithay::backend::input::InputEvent;
use smithay::backend::libei::{EiInput, ScrollEvent};
use smithay::input::keyboard::{KeyboardSource, KeymapFile};
use smithay::reexports::calloop::{LoopHandle, PostAction};
use smithay::utils::{Logical, Rectangle};

use self::held::HeldInputs;
pub(crate) use self::policy::InputEmulationSettings;
use self::policy::{Admission, Answers};
use crate::Compositor;
use crate::event_source::EventSource;
use crate::resources::ResourceLedger;

/// The seat vaht offers emulating programs.
const SEAT_NAME: &str = "vaht";

/// What an emulating program did, for the compositor to carry out.
#[derive(Debug)]
pub(crate) enum Emulated {
    /// Pointer input: motion, a button, scrolling.
    Pointer(InputEvent<EiInput>),
    /// A key, as an evdev code, pressed or let go, from the program's own
    /// keyboard source.
    Key {
        /// The key.
        code: u32,
        /// Whether it went down.
        is_pressed: bool,
        /// The program's keyboard, so its keys are let go apart from the
        /// user's.
        source: KeyboardSource,
    },
    /// Keys and buttons the program held, to let go: every key of its
    /// keyboard, and its buttons.
    Release {
        /// The buttons.
        buttons: Vec<u32>,
        /// The program's keyboard.
        source: KeyboardSource,
    },
    /// The user is asked whether the program `name` (its client `id`) may.
    Ask {
        /// The client.
        id: u64,
        /// The program.
        name: String,
    },
}

/// What a program's devices are made with.
#[derive(Debug)]
pub(crate) struct DeviceSetup {
    /// The seat keyboard's keymap.
    pub(crate) keymap: KeymapFile,
    /// The outputs, where absolute motion points.
    pub(crate) regions: Vec<Rectangle<i32, Logical>>,
}

/// Whether a program may emulate input, as far as it is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// It has not finished its handshake.
    Connecting,
    /// The user is being asked.
    Asking,
    /// It may.
    Allowed,
    /// It left or was refused; its source goes once outside its callback.
    Ended,
}

/// One program's connection.
#[derive(Debug)]
struct EmulatingClient {
    /// Identifies it to its source's callback.
    id: u64,
    /// The process that connected.
    pid: Option<i32>,
    /// Its name, as Xwayland gave it, once the handshake is done.
    name: String,
    /// How far it is let in.
    state: State,
    /// Its connection, once the handshake is done.
    connection: Option<Connection>,
    /// The seat offered it, once it is let in.
    seat: Option<Seat>,
    /// The devices made for it.
    devices: Vec<request::Device>,
    /// Its keyboard, apart from the user's.
    keyboard: KeyboardSource,
    /// The keys and buttons it holds.
    held: HeldInputs,
    /// Its connection's registration.
    _source: EventSource<Compositor>,
}

/// The EIS server.
#[derive(Debug)]
pub(crate) struct InputEmulation {
    /// The socket programs connect to.
    socket: PathBuf,
    /// The answers the user gave.
    answers: Answers,
    /// Each program connected.
    clients: Vec<EmulatingClient>,
    /// The id the next program gets.
    next_id: u64,
    /// The client the user is being asked about, while the question
    /// shows.
    prompted: Option<u64>,
    /// The socket's registration.
    _listener: EventSource<Compositor>,
}

impl InputEmulation {
    /// Listens on a socket in `XDG_RUNTIME_DIR` for Xwayland, on `handle`'s
    /// loop.
    ///
    /// # Errors
    ///
    /// Returns why not: there is no runtime directory, or the socket
    /// cannot be made or registered.
    pub(crate) fn start(
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
    ) -> Result<Self, String> {
        let listener = eis::Listener::bind_auto().map_err(|error| error.to_string())?;
        let socket = listener.path().to_path_buf();
        let source = EventSource::insert(
            handle,
            ledger,
            EisListenerSource::new(listener),
            |context, (), compositor| {
                compositor.emulating_client_connected(context);
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| error.error.to_string())?;
        tracing::info!(socket = %socket.display(), "input emulation listening");
        Ok(Self {
            socket,
            answers: Answers::default(),
            clients: Vec::new(),
            next_id: 1,
            prompted: None,
            _listener: source,
        })
    }

    /// Returns the socket's path, for `LIBEI_SOCKET`.
    pub(crate) fn socket(&self) -> &Path {
        &self.socket
    }

    /// Takes the new connection `context`, registering it on `handle`.
    pub(crate) fn add_client(
        &mut self,
        context: eis::Context,
        (handle, ledger): (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) {
        let pid = nix::sys::socket::getsockopt(
            &context.as_fd(),
            nix::sys::socket::sockopt::PeerCredentials,
        )
        .ok()
        .map(|credentials| credentials.pid());
        let id = self.next_id;
        self.next_id += 1;
        let source = EventSource::insert(
            handle,
            ledger,
            EisRequestSource::new(context, 1),
            move |event, connection, compositor| {
                Ok(compositor.follow_emulating_client(id, event, connection))
            },
        );
        match source {
            Ok(source) => self.clients.push(EmulatingClient {
                id,
                pid,
                name: String::new(),
                state: State::Connecting,
                connection: None,
                seat: None,
                devices: Vec::new(),
                keyboard: KeyboardSource::new_auxiliary(),
                held: HeldInputs::default(),
                _source: source,
            }),
            Err(error) => {
                tracing::warn!(error = %error.error, "an emulating program was not taken");
            }
        }
    }

    /// Follows what client `id` sent (`event`), on `connection`. `display`
    /// is Xwayland's (`:1`), which only may connect; `settings` say which
    /// programs may emulate input, and `setup` gives what devices are made
    /// with. Returns what to do with its source, and what the program did.
    pub(crate) fn follow(
        &mut self,
        (id, event): (u64, Result<EisRequestSourceEvent, reis::Error>),
        connection: &Connection,
        (display, settings): (Option<&str>, &InputEmulationSettings),
        setup: impl FnOnce() -> DeviceSetup,
    ) -> (PostAction, Vec<Emulated>) {
        let answers = &self.answers;
        let Some(client) = self.clients.iter_mut().find(|client| client.id == id) else {
            return (PostAction::Remove, Vec::new());
        };
        match event {
            Ok(EisRequestSourceEvent::Connected) => {
                let emulated =
                    client.connected(connection, display, |name| settings.admit(name, answers));
                let action = if client.state == State::Ended {
                    PostAction::Remove
                } else {
                    PostAction::Continue
                };
                (action, emulated)
            }
            Ok(EisRequestSourceEvent::Request(request)) => client.request(request, setup),
            Err(error) => {
                tracing::debug!(%error, program = client.name, "an emulating program failed");
                (PostAction::Remove, client.end())
            }
        }
    }

    /// Lets client `id` emulate input (`is_allowed`), or refuses it, as the
    /// user answered; the answer stands for its program until vaht exits.
    pub(crate) fn answer(&mut self, id: u64, is_allowed: bool) -> Vec<Emulated> {
        let Some(client) = self.clients.iter_mut().find(|client| client.id == id) else {
            return Vec::new();
        };
        if self.prompted == Some(id) {
            self.prompted = None;
        }
        if client.state != State::Asking {
            return Vec::new();
        }
        self.answers.note(&client.name, is_allowed);
        tracing::info!(
            program = client.name,
            is_allowed,
            "input emulation answered"
        );
        if is_allowed {
            client.allow();
            Vec::new()
        } else {
            client.end()
        }
    }

    /// Returns the client whose question shows, if one does.
    pub(crate) const fn prompted(&self) -> Option<u64> {
        self.prompted
    }

    /// Returns the next client to ask the user about, and its program,
    /// if one waits and no question shows; it is noted as shown.
    pub(crate) fn next_question(&mut self) -> Option<(u64, String)> {
        if self.prompted.is_some() {
            return None;
        }
        let client = self
            .clients
            .iter()
            .find(|client| client.state == State::Asking)?;
        self.prompted = Some(client.id);
        Some((client.id, client.name.clone()))
    }

    /// Lets go of what every program holds: the session locked, and none
    /// of it must stay held behind the lock.
    pub(crate) fn release_all(&mut self) -> Vec<Emulated> {
        self.clients
            .iter_mut()
            .flat_map(EmulatingClient::release)
            .collect()
    }

    /// Drops the connections that ended. Never called from a client's own
    /// callback.
    pub(crate) fn prune(&mut self) {
        self.clients.retain(|client| client.state != State::Ended);
    }
}

impl EmulatingClient {
    /// Follows the handshake's end on `connection`: the program is let in,
    /// refused or asked about, as `admit` says for its name, if it came
    /// through vaht's Xwayland on `display`.
    fn connected(
        &mut self,
        connection: &Connection,
        display: Option<&str>,
        admit: impl FnOnce(&str) -> Admission,
    ) -> Vec<Emulated> {
        connection
            .name()
            .unwrap_or_default()
            .clone_into(&mut self.name);
        self.connection = Some(connection.clone());
        let is_xwayland = display.is_some_and(|display| {
            self.pid.is_some_and(|pid| {
                std::fs::read(format!("/proc/{pid}/cmdline"))
                    .is_ok_and(|command_line| policy::is_xwayland(&command_line, display))
            })
        });
        if !is_xwayland {
            tracing::info!(program = self.name, pid = ?self.pid, "input emulation refused: not Xwayland");
            return self.end();
        }
        if connection.context_type() != eis::handshake::ContextType::Sender {
            tracing::info!(program = self.name, "input emulation refused: not a sender");
            return self.end();
        }
        match admit(&self.name) {
            Admission::Allowed => {
                tracing::info!(program = self.name, "input emulation allowed");
                self.allow();
                Vec::new()
            }
            Admission::Refused => {
                tracing::info!(program = self.name, "input emulation refused");
                self.end()
            }
            Admission::Ask => {
                self.state = State::Asking;
                vec![Emulated::Ask {
                    id: self.id,
                    name: self.name.clone(),
                }]
            }
        }
    }

    /// Lets the program in: it is offered a seat.
    fn allow(&mut self) {
        self.state = State::Allowed;
        if let Some(connection) = &self.connection {
            self.seat = Some(connection.add_seat(
                Some(SEAT_NAME),
                DeviceCapability::Pointer
                    | DeviceCapability::PointerAbsolute
                    | DeviceCapability::Keyboard
                    | DeviceCapability::Button
                    | DeviceCapability::Scroll,
            ));
            let _ = connection.flush();
        }
    }

    /// Follows `request`; `setup` gives what devices are made with.
    fn request(
        &mut self,
        request: EisRequest,
        setup: impl FnOnce() -> DeviceSetup,
    ) -> (PostAction, Vec<Emulated>) {
        if self.state != State::Allowed {
            return (PostAction::Continue, Vec::new());
        }
        let emulated = match request {
            EisRequest::Disconnect => return (PostAction::Remove, self.end()),
            EisRequest::Bind(bind) => {
                self.bind(&bind.seat, bind.capabilities, &setup());
                Vec::new()
            }
            EisRequest::DeviceClosed(closed) => {
                closed.device.remove();
                self.devices.retain(|device| *device != closed.device);
                self.release()
            }
            EisRequest::DeviceStopEmulating(_) => self.release(),
            EisRequest::KeyboardKey(key) => {
                let is_pressed = key.state == eis::keyboard::KeyState::Press;
                self.held.key(key.key, is_pressed);
                vec![Emulated::Key {
                    code: key.key,
                    is_pressed,
                    source: self.keyboard,
                }]
            }
            EisRequest::Button(button) => {
                let is_pressed = button.state == eis::button::ButtonState::Press;
                self.held.button(button.button, is_pressed);
                vec![Emulated::Pointer(InputEvent::PointerButton {
                    event: button,
                })]
            }
            EisRequest::PointerMotion(event) => {
                vec![Emulated::Pointer(InputEvent::PointerMotion { event })]
            }
            EisRequest::PointerMotionAbsolute(event) => {
                vec![Emulated::Pointer(InputEvent::PointerMotionAbsolute {
                    event,
                })]
            }
            EisRequest::ScrollDelta(event) => vec![Emulated::Pointer(InputEvent::PointerAxis {
                event: ScrollEvent::Delta(event),
            })],
            EisRequest::ScrollDiscrete(event) => vec![Emulated::Pointer(InputEvent::PointerAxis {
                event: ScrollEvent::Discrete(event),
            })],
            EisRequest::ScrollStop(event) => vec![Emulated::Pointer(InputEvent::PointerAxis {
                event: ScrollEvent::Stop(event),
            })],
            EisRequest::ScrollCancel(event) => vec![Emulated::Pointer(InputEvent::PointerAxis {
                event: ScrollEvent::Cancel(event),
            })],
            // Frames group what came before, which went in as it came;
            // touch and text are not offered.
            EisRequest::Frame(_)
            | EisRequest::RequestDevice(_)
            | EisRequest::Ready(_)
            | EisRequest::DeviceStartEmulating(_)
            | EisRequest::TouchDown(_)
            | EisRequest::TouchUp(_)
            | EisRequest::TouchMotion(_)
            | EisRequest::TouchCancel(_)
            | EisRequest::TextKeysym(_)
            | EisRequest::TextUtf8(_) => Vec::new(),
        };
        (PostAction::Continue, emulated)
    }

    /// Makes the devices the program bound `seat` for: a keyboard with the
    /// seat's keymap, a pointer, and an absolute pointer over the outputs,
    /// as `capabilities` ask, with `setup`.
    fn bind(
        &mut self,
        seat: &Seat,
        capabilities: reis::enumflags2::BitFlags<DeviceCapability>,
        setup: &DeviceSetup,
    ) {
        for device in self.devices.drain(..) {
            device.remove();
        }
        if capabilities.contains(DeviceCapability::Keyboard) {
            let keymap = &setup.keymap;
            let device = seat.add_device(
                Some("vaht keyboard"),
                eis::device::DeviceType::Virtual,
                DeviceCapability::Keyboard.into(),
                |device| {
                    if let Some(keyboard) = device.interface::<eis::Keyboard>() {
                        let _ = keymap.with_fd(true, |fd, size| {
                            keyboard.keymap(
                                eis::keyboard::KeymapType::Xkb,
                                u32::try_from(size).unwrap_or(u32::MAX),
                                fd,
                            );
                        });
                    }
                },
            );
            device.resumed();
            self.devices.push(device);
        }
        if capabilities.contains(DeviceCapability::Pointer) {
            let device = seat.add_device(
                Some("vaht pointer"),
                eis::device::DeviceType::Virtual,
                DeviceCapability::Pointer | DeviceCapability::Button | DeviceCapability::Scroll,
                |_| {},
            );
            device.resumed();
            self.devices.push(device);
        }
        if capabilities.contains(DeviceCapability::PointerAbsolute) {
            let regions = &setup.regions;
            let device = seat.add_device(
                Some("vaht absolute pointer"),
                eis::device::DeviceType::Virtual,
                DeviceCapability::PointerAbsolute
                    | DeviceCapability::Button
                    | DeviceCapability::Scroll,
                |device| {
                    let unsigned = |value: i32| u32::try_from(value).unwrap_or_default();
                    for region in regions {
                        device.device().region(
                            unsigned(region.loc.x),
                            unsigned(region.loc.y),
                            unsigned(region.size.w),
                            unsigned(region.size.h),
                            1.0,
                        );
                    }
                },
            );
            device.resumed();
            self.devices.push(device);
        }
        if let Some(connection) = &self.connection {
            let _ = connection.flush();
        }
    }

    /// Lets go of what the program holds.
    fn release(&mut self) -> Vec<Emulated> {
        let (keys, buttons) = self.held.release_all();
        if keys.is_empty() && buttons.is_empty() {
            return Vec::new();
        }
        vec![Emulated::Release {
            buttons,
            source: self.keyboard,
        }]
    }

    /// Ends the connection: what the program holds is let go, and it is
    /// told it is disconnected if it is still there.
    fn end(&mut self) -> Vec<Emulated> {
        self.state = State::Ended;
        if let Some(connection) = &self.connection {
            connection.disconnected(eis::connection::DisconnectReason::Disconnected, None);
            let _ = connection.flush();
        }
        self.release()
    }
}
