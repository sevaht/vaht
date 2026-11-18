//! A dialog in a Wayland window: an xdg-shell toplevel vaht frames, one
//! line tall, drawn with vaht-ui into shared memory.
//!
//! The window waits for events and nothing else (General F9): key repeat
//! comes from smithay-client-toolkit's repeat timer, armed only while a key
//! is held.

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::wl_keyboard::WlKeyboard;
use smithay_client_toolkit::reexports::client::protocol::wl_output::{Transform, WlOutput};
use smithay_client_toolkit::reexports::client::protocol::wl_seat::WlSeat;
use smithay_client_toolkit::reexports::client::protocol::wl_shm::Format;
use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface as _;
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::xdg::window::{
    Window, WindowConfigure, WindowDecorations, WindowHandler,
};
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};
use vaht_ui::{
    Panel, PanelContent, Size, Style, StyleElement, TextField, Typesetter, Widget, WidgetState,
};

use crate::{EditKey, LineDialog, Outcome};

/// Shows `dialog` in a window styled by `style`, and runs it until it
/// closes.
///
/// # Errors
///
/// Returns an error if there is no Wayland compositor to show it on, it
/// lacks what a window needs, the connection fails, or the dialog could not
/// do what it was asked.
pub fn show(dialog: Box<dyn LineDialog>, style: Style) -> Result<(), DialogError> {
    let connection = Connection::connect_to_env().map_err(DialogError::Connect)?;
    let (globals, queue) =
        registry_queue_init::<DialogWindow>(&connection).map_err(DialogError::Globals)?;
    let handle = queue.handle();
    let mut event_loop: EventLoop<'static, DialogWindow> =
        EventLoop::try_new().map_err(DialogError::EventLoop)?;
    WaylandSource::new(connection, queue)
        .insert(event_loop.handle())
        .map_err(|error| DialogError::EventLoop(error.error))?;
    let missing = |global| move |source| DialogError::Missing { global, source };
    let compositor = CompositorState::bind(&globals, &handle).map_err(missing("wl_compositor"))?;
    let shell = XdgShell::bind(&globals, &handle).map_err(missing("xdg_wm_base"))?;
    let shm = Shm::bind(&globals, &handle).map_err(missing("wl_shm"))?;

    let typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
    let field = style.element_style(StyleElement::DialogField, WidgetState::Focused);
    // A line of the font and a little room, as fluxland-run has.
    let height = typesetter.line_height(&field.text.font) + field.padding.vertical() + 4;
    let size = Size::new(DialogWindow::WIDTH, height);
    let pool =
        SlotPool::new(DialogWindow::buffer_bytes(size), &shm).map_err(DialogError::Memory)?;

    let surface = compositor.create_surface(&handle);
    let window = shell.create_window(surface, WindowDecorations::RequestServer, &handle);
    window.set_title(dialog.title());
    window.set_app_id(dialog.app_id());
    // A window of one size.
    window.set_min_size(Some((size.width, size.height)));
    window.set_max_size(Some((size.width, size.height)));
    window.commit();

    let mut state = DialogWindow {
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &handle),
        outputs: OutputState::new(&globals, &handle),
        shm,
        pool,
        buffer: None,
        window,
        keyboard: None,
        modifiers: Modifiers::default(),
        panel: Panel::new(FieldContent(dialog.field()), size),
        title: dialog.title().to_owned(),
        dialog,
        style,
        typesetter,
        is_configured: false,
        ending: None,
        loop_handle: event_loop.handle(),
    };
    while state.ending.is_none() {
        event_loop
            .dispatch(None, &mut state)
            .map_err(DialogError::Dispatch)?;
    }
    state.ending.unwrap_or(Ok(()))
}

/// Why a dialog could not be shown, or failed.
#[derive(Debug, thiserror::Error)]
pub enum DialogError {
    /// There is no Wayland compositor to connect to.
    #[error("could not connect to the Wayland compositor")]
    Connect(#[source] smithay_client_toolkit::reexports::client::ConnectError),
    /// The compositor's globals could not be listed.
    #[error("could not list the compositor's globals")]
    Globals(#[source] smithay_client_toolkit::reexports::client::globals::GlobalError),
    /// The compositor lacks something a window needs.
    #[error("the compositor lacks {global}")]
    Missing {
        /// What it lacks.
        global: &'static str,
        /// Why binding it failed.
        #[source]
        source: smithay_client_toolkit::reexports::client::globals::BindError,
    },
    /// The event loop could not be set up.
    #[error("could not set up the event loop")]
    EventLoop(#[source] smithay_client_toolkit::reexports::calloop::Error),
    /// Waiting for events failed.
    #[error("could not wait for events")]
    Dispatch(#[source] smithay_client_toolkit::reexports::calloop::Error),
    /// Memory to draw in could not be had.
    #[error("could not allocate memory to draw in")]
    Memory(#[source] smithay_client_toolkit::shm::CreatePoolError),
    /// The dialog could not do what was asked.
    #[error("{0}")]
    Dialog(String),
}

/// A dialog's line, as a panel shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FieldContent(TextField);

impl PanelContent for FieldContent {
    type Hit = ();

    fn build(&self) -> Widget<()> {
        Widget::field(StyleElement::DialogField, self.0.clone())
    }

    fn state(&self) -> WidgetState {
        WidgetState::Focused
    }
}

/// The window, and what it shows.
struct DialogWindow {
    /// The compositor's globals.
    registry: RegistryState,
    /// The seats, for the keyboard.
    seats: SeatState,
    /// The outputs, which surfaces keep track of.
    outputs: OutputState,
    /// Shared memory.
    shm: Shm,
    /// The memory buffers are drawn in.
    pool: SlotPool,
    /// The buffer last shown; dropping it frees it once the compositor
    /// lets go of it.
    buffer: Option<Buffer>,
    /// The window.
    window: Window,
    /// The keyboard, once the seat has one.
    keyboard: Option<WlKeyboard>,
    /// The modifiers held.
    modifiers: Modifiers,
    /// Lays out and paints the line.
    panel: Panel<FieldContent>,
    /// The dialog.
    dialog: Box<dyn LineDialog>,
    /// The window's title, as last set.
    title: String,
    /// How the line looks.
    style: Style,
    /// Draws text.
    typesetter: Typesetter,
    /// Whether the compositor has configured the window, so it may draw.
    is_configured: bool,
    /// Set when the dialog is done: how it ended.
    ending: Option<Result<(), DialogError>>,
    /// Registers the key repeat timer.
    loop_handle: LoopHandle<'static, Self>,
}

impl DialogWindow {
    /// The window's width, in pixels: fbrun's and fluxland-run's.
    const WIDTH: u32 = 200;

    /// Returns the bytes a buffer of `size` takes, as ARGB8888.
    fn buffer_bytes(size: Size) -> usize {
        usize::try_from(u64::from(size.width) * u64::from(size.height) * 4)
            .expect("a dialog's buffer is a few kilobytes")
    }

    /// Does what `event`'s key asks, and draws the result.
    fn key(&mut self, event: &KeyEvent) {
        let Some(key) = Self::edit_key(event, self.modifiers) else {
            return;
        };
        match self.dialog.press(key) {
            Ok(Outcome::Open) => {
                if self.title != self.dialog.title() {
                    self.dialog.title().clone_into(&mut self.title);
                    self.window.set_title(self.title.clone());
                }
                let field = self.dialog.field();
                self.panel.update_if(|content| {
                    let changed = content.0 != field;
                    content.0 = field;
                    changed
                });
                self.draw();
            }
            Ok(Outcome::Close) => self.ending = Some(Ok(())),
            Err(message) => self.ending = Some(Err(DialogError::Dialog(message))),
        }
    }

    /// Returns what a key asks of the line, given the modifiers held; none
    /// for a key that does nothing.
    fn edit_key(event: &KeyEvent, modifiers: Modifiers) -> Option<EditKey> {
        let key = match event.keysym {
            Keysym::Return | Keysym::KP_Enter => EditKey::Accept,
            Keysym::Escape => EditKey::Cancel,
            Keysym::Tab => EditKey::Complete,
            Keysym::Up | Keysym::KP_Up => EditKey::Older,
            Keysym::Down | Keysym::KP_Down => EditKey::Newer,
            Keysym::BackSpace => EditKey::DeleteBack,
            Keysym::Delete | Keysym::KP_Delete => EditKey::DeleteForward,
            Keysym::Left | Keysym::KP_Left => EditKey::Left,
            Keysym::Right | Keysym::KP_Right => EditKey::Right,
            Keysym::Home | Keysym::KP_Home => EditKey::Home,
            Keysym::End | Keysym::KP_End => EditKey::End,
            // Ctrl+A, E, U and K, as a shell has them.
            Keysym::a | Keysym::A if modifiers.ctrl => EditKey::Home,
            Keysym::e | Keysym::E if modifiers.ctrl => EditKey::End,
            Keysym::u | Keysym::U if modifiers.ctrl => EditKey::KillToStart,
            Keysym::k | Keysym::K if modifiers.ctrl => EditKey::KillToEnd,
            _ if modifiers.ctrl || modifiers.alt || modifiers.logo => return None,
            _ => {
                let text = event.utf8.as_deref()?;
                if text.chars().any(char::is_control) || text.is_empty() {
                    return None;
                }
                EditKey::Insert(text.to_owned())
            }
        };
        Some(key)
    }

    /// Paints the line if it changed and shows it.
    fn draw(&mut self) {
        if !self.is_configured {
            return;
        }
        self.panel.paint(&self.style, &mut self.typesetter);
        let Some(pixels) = self.panel.pixels() else {
            return;
        };
        let size = self.panel.size();
        let (Ok(width), Ok(height)) = (i32::try_from(size.width), i32::try_from(size.height))
        else {
            return;
        };
        let Ok((buffer, canvas)) =
            self.pool
                .create_buffer(width, height, width * 4, Format::Argb8888)
        else {
            self.ending = Some(Err(DialogError::Dialog(
                "could not get memory to draw in".to_owned(),
            )));
            return;
        };
        // tiny-skia's premultiplied RGBA to Wayland's ARGB8888, which is
        // BGRA in memory on the little-endian machines vaht runs on.
        let (to_pixels, _) = canvas.as_chunks_mut::<4>();
        let (from_pixels, _) = pixels.data().as_chunks::<4>();
        for (to, [red, green, blue, alpha]) in to_pixels.iter_mut().zip(from_pixels) {
            *to = [*blue, *green, *red, *alpha];
        }
        let surface = self.window.wl_surface();
        surface.damage_buffer(0, 0, width, height);
        if buffer.attach_to(surface).is_ok() {
            self.window.commit();
        }
        self.buffer = Some(buffer);
    }
}

impl CompositorHandler for DialogWindow {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }
}

impl WindowHandler for DialogWindow {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.ending = Some(Ok(()));
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &Window,
        _: WindowConfigure,
        _: u32,
    ) {
        // The window keeps its one size, whatever is suggested.
        if !self.is_configured {
            self.is_configured = true;
            self.draw();
        }
    }
}

impl SeatHandler for DialogWindow {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Keyboard || self.keyboard.is_some() {
            return;
        }
        let keyboard = self.seats.get_keyboard_with_repeat(
            qh,
            &seat,
            None,
            self.loop_handle.clone(),
            Box::new(|window, _, event| window.key(&event)),
        );
        match keyboard {
            Ok(keyboard) => self.keyboard = Some(keyboard),
            Err(error) => {
                self.ending = Some(Err(DialogError::Dialog(format!(
                    "could not use the keyboard: {error}"
                ))));
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.keyboard.take()
        {
            keyboard.release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl KeyboardHandler for DialogWindow {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: &WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: &WlSurface,
        _: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(&event);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(&event);
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.modifiers = modifiers;
    }
}

impl OutputHandler for DialogWindow {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}

impl ShmHandler for DialogWindow {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for DialogWindow {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState, SeatState];
}

delegate_registry!(DialogWindow);
delegate_dispatch2!(DialogWindow);
