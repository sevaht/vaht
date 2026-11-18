//! What a test client knows and has, and the event handling that keeps it
//! up to date: the toolkit's handlers, and the protocol objects whose events
//! the client records or ignores.

use std::time::Duration;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, FrameCallbackData};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::globals::GlobalList;
use smithay_client_toolkit::reexports::client::protocol::{wl_buffer, wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::protocols::wp::pointer_constraints::zv1::client::zwp_locked_pointer_v1::{self, ZwpLockedPointerV1};
use smithay_client_toolkit::reexports::protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1;
use smithay_client_toolkit::reexports::protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1;
use smithay_client_toolkit::reexports::protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{self, ExtIdleNotificationV1};
use smithay_client_toolkit::reexports::protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;
use smithay_client_toolkit::reexports::protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use smithay_client_toolkit::reexports::protocols::ext::session_lock::v1::client::ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1};
use smithay_client_toolkit::reexports::protocols::ext::session_lock::v1::client::ext_session_lock_v1::{self, ExtSessionLockV1};
use smithay_client_toolkit::reexports::protocols::wp::idle_inhibit::zv1::client::zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{self, WpFractionalScaleV1};
use smithay_client_toolkit::reexports::protocols::wp::idle_inhibit::zv1::client::zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1;
use smithay_client_toolkit::reexports::protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use smithay_client_toolkit::reexports::protocols::wp::text_input::zv3::client::zwp_text_input_v3::{self, ZwpTextInputV3};
use smithay_client_toolkit::reexports::protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_v1::{self, ZwpRelativePointerV1};
use smithay_client_toolkit::reexports::protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use smithay_client_toolkit::reexports::protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle, WEnum};
use smithay_client_toolkit::reexports::protocols::wp::presentation_time::client::wp_presentation::{self, WpPresentation};
use smithay_client_toolkit::reexports::protocols::wp::pointer_warp::v1::client::wp_pointer_warp_v1::WpPointerWarpV1;
use smithay_client_toolkit::reexports::protocols::wp::fifo::v1::client::wp_fifo_manager_v1::WpFifoManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::fifo::v1::client::wp_fifo_v1::WpFifoV1;
use smithay_client_toolkit::reexports::protocols::wp::commit_timing::v1::client::wp_commit_timing_manager_v1::WpCommitTimingManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::commit_timing::v1::client::wp_commit_timer_v1::WpCommitTimerV1;
use smithay_client_toolkit::reexports::protocols::wp::presentation_time::client::wp_presentation_feedback::{self, WpPresentationFeedback};
use smithay_client_toolkit::reexports::protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
use smithay_client_toolkit::reexports::protocols::ext::image_capture_source::v1::client::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1;
use smithay_client_toolkit::reexports::protocols::xdg::dialog::v1::client::xdg_dialog_v1::XdgDialogV1;
use smithay_client_toolkit::reexports::protocols::xdg::dialog::v1::client::xdg_wm_dialog_v1::XdgWmDialogV1;
use smithay_client_toolkit::reexports::protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1};
use smithay_client_toolkit::reexports::protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1;
use smithay_client_toolkit::reexports::protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1};
use smithay_client_toolkit::reexports::protocols::xdg::toplevel_icon::v1::client::xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1;
use smithay_client_toolkit::reexports::protocols::xdg::toplevel_icon::v1::client::xdg_toplevel_icon_v1::XdgToplevelIconV1;
use smithay_client_toolkit::reexports::protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use smithay_client_toolkit::reexports::protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::WaylandSurface as _;
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::xdg::window::{Window, WindowConfigure, WindowHandler};
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::data_device_manager::WritePipe;
use smithay_client_toolkit::data_device_manager::data_device::DataDeviceHandler;
use smithay_client_toolkit::data_device_manager::data_offer::{DataOfferHandler, DragOffer};
use smithay_client_toolkit::data_device_manager::data_source::DataSourceHandler;
use smithay_client_toolkit::reexports::client::protocol::wl_data_device::WlDataDevice;
use smithay_client_toolkit::reexports::client::protocol::wl_data_device_manager::DndAction;
use smithay_client_toolkit::reexports::client::protocol::wl_data_source::WlDataSource;
use smithay_client_toolkit::reexports::protocols::xdg::toplevel_drag::v1::client::xdg_toplevel_drag_manager_v1::XdgToplevelDragManagerV1;
use smithay_client_toolkit::reexports::protocols::xdg::toplevel_drag::v1::client::xdg_toplevel_drag_v1::XdgToplevelDragV1;
use smithay_client_toolkit::reexports::protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1};
use smithay_client_toolkit::reexports::protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;
use smithay_client_toolkit::reexports::protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::{self, ExtBackgroundEffectManagerV1};
use smithay_client_toolkit::reexports::protocols::ext::background_effect::v1::client::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1;
use smithay_client_toolkit::reexports::protocols::wp::color_management::v1::client::wp_color_management_output_v1::WpColorManagementOutputV1;
use smithay_client_toolkit::reexports::protocols::wp::color_management::v1::client::wp_color_management_surface_v1::WpColorManagementSurfaceV1;
use smithay_client_toolkit::reexports::protocols::wp::color_management::v1::client::wp_color_manager_v1::{self, WpColorManagerV1};
use smithay_client_toolkit::reexports::protocols::wp::color_management::v1::client::wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1;
use smithay_client_toolkit::reexports::protocols::wp::color_management::v1::client::wp_image_description_info_v1::{self, WpImageDescriptionInfoV1};
use smithay_client_toolkit::reexports::protocols::wp::color_management::v1::client::wp_image_description_v1::{self, WpImageDescriptionV1};
use smithay_client_toolkit::{delegate_registry, registry_handlers};

use super::{BYTES_PER_PIXEL, Presentation, WINDOW_COLOR, WINDOW_SIZE};

/// Binds the global `name`, which vaht must offer.
pub(super) fn bind<I>(globals: &GlobalList, handle: &QueueHandle<ClientState>, name: &str) -> I
where
    I: smithay_client_toolkit::reexports::client::Proxy + 'static,
    NoEvents: Dispatch2<I, ClientState>,
{
    globals
        .bind(handle, 1..=1, NoEvents)
        .unwrap_or_else(|error| panic!("vaht offers {name}: {error}"))
}

/// The client's state, which the toolkit's handlers update.
#[derive(Debug)]
pub(super) struct ClientState {
    /// The globals vaht offers.
    pub(super) registry: RegistryState,
    /// vaht's outputs.
    pub(super) outputs: OutputState,
    /// Creates surfaces.
    pub(super) compositor: CompositorState,
    /// Creates windows.
    pub(super) xdg_shell: XdgShell,
    /// Turns outputs into capture sources.
    pub(super) capture_sources: ExtOutputImageCaptureSourceManagerV1,
    /// Captures them.
    pub(super) capture: ExtImageCopyCaptureManagerV1,
    /// Gives windows icons.
    pub(super) toplevel_icons: XdgToplevelIconManagerV1,
    /// Makes dialogs modal.
    pub(super) dialogs: XdgWmDialogV1,
    /// The buffers of icons the client drew, kept while vaht may read them.
    pub(super) icon_buffers: Vec<smithay_client_toolkit::shm::slot::Buffer>,
    /// Shared-memory buffers.
    pub(super) shm: Shm,
    /// Where buffers are allocated.
    pub(super) pool: SlotPool,
    /// The open windows.
    pub(super) windows: Vec<Window>,
    /// Makes layer surfaces.
    pub(super) layer_shell: LayerShell,
    /// The open layer surfaces.
    pub(super) layers: Vec<LayerSurface>,
    /// Each layer surface's color.
    pub(super) layer_colors: Vec<(wl_surface::WlSurface, [u8; 3])>,
    /// How many windows have drawn for the first time.
    pub(super) first_draws: usize,
    /// The buffer scale vaht last preferred for any surface
    /// (`wl_surface.preferred_buffer_scale`), if it said one.
    pub(super) preferred_scale: Option<i32>,
    /// The serial of the last button press vaht sent this client, which
    /// a drag it starts names.
    pub(super) last_button_serial: Option<u32>,
    /// The fractional scale vaht last preferred, in 120ths
    /// (`wp_fractional_scale_v1.preferred_scale`), if it said one.
    pub(super) fractional_scale: Option<u32>,
    /// The surfaces of the windows that have drawn.
    pub(super) drawn: Vec<wl_surface::WlSurface>,
    /// How many times vaht asked to close a window.
    pub(super) close_requests: usize,
    /// How many of its layer surfaces vaht has closed.
    pub(super) closed_layers: usize,
    /// The buffer size the current capture session asked for.
    pub(super) capture_size: Option<(u32, u32)>,
    /// How the current capture ended.
    pub(super) capture_result: Option<Result<(), String>>,
    /// The shared-memory buffer the current wlr screen copy asked for:
    /// width, height and stride.
    pub(super) screencopy_buffer: Option<(u32, u32, u32)>,
    /// Whether the current wlr screen copy described all its buffers.
    pub(super) screencopy_described: bool,
    /// How the current wlr screen copy ended.
    pub(super) screencopy_result: Option<Result<(), ()>>,
    /// Whether vaht said it blurs what is behind surfaces, once it said.
    pub(super) blurs: Option<bool>,
    /// The transfer functions vaht says it converts, once it said.
    pub(super) transfer_functions: Option<Vec<wp_color_manager_v1::TransferFunction>>,
    /// Those it is saying, until it says it is done.
    pub(super) transfer_functions_told: Vec<wp_color_manager_v1::TransferFunction>,
    /// The identity of the image description made last, once ready.
    pub(super) description_ready: Option<u32>,
    /// What vaht said an image description is: its transfer function and
    /// named primaries, once done.
    pub(super) description_info: Option<(
        Option<wp_color_manager_v1::TransferFunction>,
        Option<wp_color_manager_v1::Primaries>,
    )>,
    /// What it is saying, until done.
    pub(super) description_info_told: (
        Option<wp_color_manager_v1::TransferFunction>,
        Option<wp_color_manager_v1::Primaries>,
    ),
    /// How the current capture's image is turned from the output's own
    /// orientation.
    pub(super) capture_transform: wl_output::Transform,
    /// The surface with keyboard focus, as vaht last told.
    pub(super) keyboard_focus: Option<wl_surface::WlSurface>,
    /// How far the mouse moved, as relative motion told, in all.
    pub(super) relative_motion: (f64, f64),
    /// Whether vaht holds the pointer locked for the client.
    pub(super) is_pointer_locked: bool,
    /// The keys pressed while the client had the keyboard, as Linux key
    /// codes, in order.
    pub(super) received_keys: Vec<u32>,
    /// Whether vaht says the session is idle, once it has said.
    pub(super) is_idle: Option<bool>,
    /// Whether vaht says the session is locked for the client, once it has
    /// said.
    pub(super) is_session_locked: Option<bool>,
    /// The serial of the pointer's last entering a surface of the client,
    /// while it is on one.
    pub(super) pointer_entered: Option<u32>,
    /// Where the pointer is on the surface it is on, as vaht last said.
    pub(super) pointer_position: Option<(f64, f64)>,
    /// The clock presentation times are told by, once vaht has said.
    pub(super) presentation_clock: Option<u32>,
    /// How vaht reported the last frame asked about, once it has.
    pub(super) presentation: Option<Presentation>,
    /// The text an input method typed into the client's text field.
    pub(super) typed_text: String,
    /// Text an input method committed, until its `done`.
    pending_text: Option<String>,
}

impl ClientState {
    /// Binds every global the client needs from `globals`.
    ///
    /// # Panics
    ///
    /// Panics if vaht lacks one of them.
    pub(super) fn bind(globals: &GlobalList, handle: &QueueHandle<Self>) -> Self {
        let shm = Shm::bind(globals, handle).expect("vaht offers wl_shm");
        let pool_size = usize::try_from(WINDOW_SIZE.0 * WINDOW_SIZE.1 * BYTES_PER_PIXEL)
            .expect("a small buffer size fits in usize");
        Self {
            registry: RegistryState::new(globals),
            outputs: OutputState::new(globals, handle),
            compositor: CompositorState::bind(globals, handle).expect("vaht offers wl_compositor"),
            xdg_shell: XdgShell::bind(globals, handle).expect("vaht offers xdg_wm_base"),
            layer_shell: LayerShell::bind(globals, handle).expect("vaht offers a layer shell"),
            layers: Vec::new(),
            layer_colors: Vec::new(),
            capture_sources: bind(
                globals,
                handle,
                "ext_output_image_capture_source_manager_v1",
            ),
            capture: bind(globals, handle, "ext_image_copy_capture_manager_v1"),
            toplevel_icons: bind(globals, handle, "xdg_toplevel_icon_manager_v1"),
            dialogs: bind(globals, handle, "xdg_wm_dialog_v1"),
            icon_buffers: Vec::new(),
            pool: SlotPool::new(pool_size, &shm).expect("a shared-memory pool"),
            shm,
            windows: Vec::new(),
            first_draws: 0,
            preferred_scale: None,
            fractional_scale: None,
            last_button_serial: None,
            screencopy_buffer: None,
            screencopy_described: false,
            screencopy_result: None,
            blurs: None,
            transfer_functions: None,
            transfer_functions_told: Vec::new(),
            description_ready: None,
            description_info: None,
            description_info_told: (None, None),
            drawn: Vec::new(),
            close_requests: 0,
            closed_layers: 0,
            capture_size: None,
            capture_result: None,
            capture_transform: wl_output::Transform::Normal,
            keyboard_focus: None,
            relative_motion: (0.0, 0.0),
            is_pointer_locked: false,
            received_keys: Vec::new(),
            is_idle: None,
            pointer_entered: None,
            pointer_position: None,
            presentation_clock: None,
            presentation: None,
            is_session_locked: None,
            typed_text: String::new(),
            pending_text: None,
        }
    }

    /// Fills `window`, `size` pixels large, with [`WINDOW_COLOR`] and asks
    /// to hear when it is shown.
    pub(super) fn draw(&mut self, window: &Window, handle: &QueueHandle<Self>, size: (u32, u32)) {
        self.fill(window.wl_surface(), handle, size, WINDOW_COLOR);
    }

    /// Returns a buffer `size` pixels square, all `color`, for an icon.
    pub(super) fn icon_buffer(&mut self, size: u32, color: [u8; 3]) -> wl_buffer::WlBuffer {
        let extent = i32::try_from(size).expect("icon sizes fit in i32");
        let stride = extent * i32::try_from(BYTES_PER_PIXEL).expect("4 fits in i32");
        let (buffer, canvas) = self
            .pool
            .create_buffer(extent, extent, stride, wl_shm::Format::Argb8888)
            .expect("the pool has room");
        let [red, green, blue] = color;
        let (pixels, _) = canvas.as_chunks_mut::<4>();
        for pixel in pixels {
            *pixel = [blue, green, red, u8::MAX];
        }
        let wl_buffer = buffer.wl_buffer().clone();
        self.icon_buffers.push(buffer);
        wl_buffer
    }

    /// Fills `surface` with `color` at `size`, and commits it.
    pub(super) fn fill(
        &mut self,
        surface: &wl_surface::WlSurface,
        handle: &QueueHandle<Self>,
        size: (u32, u32),
        color: [u8; 3],
    ) {
        let [red, green, blue] = color;
        self.fill_pixels(surface, handle, size, [blue, green, red, u8::MAX]);
    }

    /// Fills `surface` with `pixel` (premultiplied BGRA, as ARGB8888 is in
    /// memory) at `size`, and commits it.
    pub(super) fn fill_pixels(
        &mut self,
        surface: &wl_surface::WlSurface,
        handle: &QueueHandle<Self>,
        size: (u32, u32),
        pixel: [u8; 4],
    ) {
        let width = i32::try_from(size.0).expect("window sizes fit in i32");
        let height = i32::try_from(size.1).expect("window sizes fit in i32");
        let stride = width * i32::try_from(BYTES_PER_PIXEL).expect("4 fits in i32");
        let (buffer, canvas) = self
            .pool
            .create_buffer(width, height, stride, wl_shm::Format::Argb8888)
            .expect("the pool has room");
        let (pixels, _) = canvas.as_chunks_mut::<4>();
        pixels.fill(pixel);
        surface.frame(handle, FrameCallbackData(surface.clone()));
        buffer.attach_to(surface).expect("a fresh buffer attaches");
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
    }
}

/// User data for objects whose events the client ignores or has none.
#[derive(Debug, Clone, Copy)]
pub(super) struct NoEvents;

/// Implements [`Dispatch2`] for interfaces whose events are ignored.
macro_rules! ignore_events {
    ($($interface:ty),+ $(,)?) => {
        $(
            impl Dispatch2<$interface, ClientState> for NoEvents {
                fn event(
                    &self,
                    _: &mut ClientState,
                    _: &$interface,
                    _: <$interface as smithay_client_toolkit::reexports::client::Proxy>::Event,
                    _: &Connection,
                    _: &QueueHandle<ClientState>,
                ) {
                }
            }
        )+
    };
}

ignore_events!(
    wl_seat::WlSeat,
    WpPointerWarpV1,
    WpFifoManagerV1,
    WpFifoV1,
    WpCommitTimingManagerV1,
    WpCommitTimerV1,
    ExtSessionLockManagerV1,
    ExtIdleNotifierV1,
    ZwpIdleInhibitManagerV1,
    ZwpIdleInhibitorV1,
    WpFractionalScaleManagerV1,
    XdgToplevelDragManagerV1,
    XdgToplevelDragV1,
    ZwlrScreencopyManagerV1,
    ExtBackgroundEffectSurfaceV1,
    WpColorManagementOutputV1,
    WpColorManagementSurfaceV1,
    WpImageDescriptionCreatorParamsV1,
    ZwpKeyboardShortcutsInhibitManagerV1,
    ZwpKeyboardShortcutsInhibitorV1,
    ZwpRelativePointerManagerV1,
    ZwpTextInputManagerV3,
    ZwpPointerConstraintsV1,
    ZwpVirtualKeyboardManagerV1,
    ZwpVirtualKeyboardV1,
    ZwlrVirtualPointerManagerV1,
    ZwlrVirtualPointerV1,
    ExtOutputImageCaptureSourceManagerV1,
    ExtImageCaptureSourceV1,
    ExtImageCopyCaptureManagerV1,
    XdgToplevelIconManagerV1,
    XdgToplevelIconV1,
    XdgWmDialogV1,
    XdgDialogV1,
);

/// User data of the seat's keyboard: records where keyboard focus is.
#[derive(Debug, Clone, Copy)]
pub(super) struct KeyboardEvents;

impl Dispatch2<wl_keyboard::WlKeyboard, ClientState> for KeyboardEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "only where focus is matters to a test, not the keys"
        )]
        match event {
            wl_keyboard::Event::Enter { surface, .. } => state.keyboard_focus = Some(surface),
            wl_keyboard::Event::Leave { .. } => state.keyboard_focus = None,
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => state.received_keys.push(key),
            _ => {}
        }
    }
}

/// User data of the relative pointer: adds up how far the mouse moved.
#[derive(Debug, Clone, Copy)]
pub(super) struct RelativeMotionEvents;

impl Dispatch2<ZwpRelativePointerV1, ClientState> for RelativeMotionEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &ZwpRelativePointerV1,
        event: zwp_relative_pointer_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        if let zwp_relative_pointer_v1::Event::RelativeMotion { dx, dy, .. } = event {
            state.relative_motion.0 += dx;
            state.relative_motion.1 += dy;
        }
    }
}

/// User data of the seat's pointer: records where the pointer is on the
/// client's surfaces.
#[derive(Debug, Clone, Copy)]
pub(super) struct PointerEvents;

impl Dispatch2<wl_pointer::WlPointer, ClientState> for PointerEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "only where the pointer is matters to a test, and the serial of a press"
        )]
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_entered = Some(serial);
                state.pointer_position = Some((surface_x, surface_y));
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => state.pointer_position = Some((surface_x, surface_y)),
            wl_pointer::Event::Leave { .. } => {
                state.pointer_entered = None;
                state.pointer_position = None;
            }
            wl_pointer::Event::Button {
                serial,
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => state.last_button_serial = Some(serial),
            _ => {}
        }
    }
}

/// User data of the background effect manager: records whether vaht
/// blurs.
#[derive(Debug, Clone, Copy)]
pub(super) struct BackgroundEffectEvents;

impl Dispatch2<ExtBackgroundEffectManagerV1, ClientState> for BackgroundEffectEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        if let ext_background_effect_manager_v1::Event::Capabilities { flags } = event {
            state.blurs = Some(flags.into_result().is_ok_and(|flags| {
                flags.contains(ext_background_effect_manager_v1::Capability::Blur)
            }));
        }
    }
}

/// User data of the colour manager, image descriptions and their
/// information: records what vaht says.
#[derive(Debug, Clone, Copy)]
pub(super) struct ColorEvents;

impl Dispatch2<WpColorManagerV1, ClientState> for ColorEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &WpColorManagerV1,
        event: wp_color_manager_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "only the transfer functions matter to a test"
        )]
        match event {
            wp_color_manager_v1::Event::SupportedTfNamed { tf } => {
                if let Ok(tf) = tf.into_result() {
                    state.transfer_functions_told.push(tf);
                }
            }
            wp_color_manager_v1::Event::Done => {
                state.transfer_functions = Some(std::mem::take(&mut state.transfer_functions_told));
            }
            _ => {}
        }
    }
}

impl Dispatch2<WpImageDescriptionV1, ClientState> for ColorEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &WpImageDescriptionV1,
        event: wp_image_description_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        if let wp_image_description_v1::Event::Ready { identity } = event {
            state.description_ready = Some(identity);
        }
    }
}

impl Dispatch2<WpImageDescriptionInfoV1, ClientState> for ColorEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &WpImageDescriptionInfoV1,
        event: wp_image_description_info_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "the transfer function and primaries are what a test checks"
        )]
        match event {
            wp_image_description_info_v1::Event::TfNamed { tf } => {
                state.description_info_told.0 = tf.into_result().ok();
            }
            wp_image_description_info_v1::Event::PrimariesNamed { primaries } => {
                state.description_info_told.1 = primaries.into_result().ok();
            }
            wp_image_description_info_v1::Event::Done => {
                state.description_info = Some(std::mem::take(&mut state.description_info_told));
            }
            _ => {}
        }
    }
}

/// User data of a wlr screen copy frame: records its buffer and outcome.
#[derive(Debug, Clone, Copy)]
pub(super) struct ScreencopyEvents;

impl Dispatch2<ZwlrScreencopyFrameV1, ClientState> for ScreencopyEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "flags, damage and GPU buffers do not matter to a test"
        )]
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                width,
                height,
                stride,
                ..
            } => state.screencopy_buffer = Some((width, height, stride)),
            zwlr_screencopy_frame_v1::Event::BufferDone => state.screencopy_described = true,
            zwlr_screencopy_frame_v1::Event::Ready { .. } => state.screencopy_result = Some(Ok(())),
            zwlr_screencopy_frame_v1::Event::Failed => state.screencopy_result = Some(Err(())),
            _ => {}
        }
    }
}

/// User data of a surface's fractional scale: records the scale vaht
/// prefers.
#[derive(Debug, Clone, Copy)]
pub(super) struct FractionalScaleEvents;

impl Dispatch2<WpFractionalScaleV1, ClientState> for FractionalScaleEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.fractional_scale = Some(scale);
        }
    }
}

/// User data of the presentation global: records its clock.
#[derive(Debug, Clone, Copy)]
pub(super) struct PresentationEvents;

impl Dispatch2<WpPresentation, ClientState> for PresentationEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &WpPresentation,
        event: wp_presentation::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.presentation_clock = Some(clk_id);
        }
    }
}

/// User data of a frame's presentation feedback: records how vaht
/// reported the frame.
#[derive(Debug, Clone, Copy)]
pub(super) struct FeedbackEvents;

impl Dispatch2<WpPresentationFeedback, ClientState> for FeedbackEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "which output showed the frame does not matter to a test"
        )]
        match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                ..
            } => {
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                state.presentation = Some(Presentation::Shown {
                    clock: state.presentation_clock.unwrap_or_default(),
                    time: Duration::new(seconds, tv_nsec),
                    refresh: Duration::from_nanos(u64::from(refresh)),
                    sequence: (u64::from(seq_hi) << 32) | u64::from(seq_lo),
                });
            }
            wp_presentation_feedback::Event::Discarded => {
                state.presentation = Some(Presentation::Discarded);
            }
            _ => {}
        }
    }
}

/// User data of the client's text field: it takes text whenever one of
/// the client's windows has keyboard focus, and records what an input
/// method types.
#[derive(Debug, Clone, Copy)]
pub(super) struct TextFieldEvents;

impl Dispatch2<ZwpTextInputV3, ClientState> for TextFieldEvents {
    fn event(
        &self,
        state: &mut ClientState,
        text_input: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        match event {
            zwp_text_input_v3::Event::Enter { .. } => {
                text_input.enable();
                text_input.commit();
            }
            zwp_text_input_v3::Event::CommitString { text } => state.pending_text = text,
            zwp_text_input_v3::Event::Done { .. } => {
                if let Some(text) = state.pending_text.take() {
                    state.typed_text.push_str(&text);
                }
            }
            zwp_text_input_v3::Event::Leave { .. }
            | zwp_text_input_v3::Event::PreeditString { .. }
            | zwp_text_input_v3::Event::DeleteSurroundingText { .. }
            | zwp_text_input_v3::Event::Action { .. }
            | zwp_text_input_v3::Event::Language { .. }
            | zwp_text_input_v3::Event::PreeditHint { .. }
            | _ => {}
        }
    }
}

/// User data of an idle notification: records whether the session is
/// idle.
#[derive(Debug, Clone, Copy)]
pub(super) struct IdleEvents;

impl Dispatch2<ExtIdleNotificationV1, ClientState> for IdleEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.is_idle = Some(true),
            ext_idle_notification_v1::Event::Resumed => state.is_idle = Some(false),
            _ => {}
        }
    }
}

/// User data of a session lock: records whether it holds.
#[derive(Debug, Clone, Copy)]
pub(super) struct SessionLockEvents;

impl Dispatch2<ExtSessionLockV1, ClientState> for SessionLockEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        match event {
            ext_session_lock_v1::Event::Locked => state.is_session_locked = Some(true),
            ext_session_lock_v1::Event::Finished => state.is_session_locked = Some(false),
            _ => {}
        }
    }
}

/// User data of a lock surface: its surface and color, which it is filled
/// with at the size vaht configures.
#[derive(Debug, Clone)]
pub(super) struct LockSurfaceEvents {
    /// The surface.
    pub(super) surface: wl_surface::WlSurface,
    /// Its color.
    pub(super) color: [u8; 3],
}

impl Dispatch2<ExtSessionLockSurfaceV1, ClientState> for LockSurfaceEvents {
    fn event(
        &self,
        state: &mut ClientState,
        lock_surface: &ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        _: &Connection,
        handle: &QueueHandle<ClientState>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            lock_surface.ack_configure(serial);
            state.fill(&self.surface, handle, (width, height), self.color);
        }
    }
}

/// User data of a pointer lock: records whether it holds.
#[derive(Debug, Clone, Copy)]
pub(super) struct LockEvents;

impl Dispatch2<ZwpLockedPointerV1, ClientState> for LockEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &ZwpLockedPointerV1,
        event: zwp_locked_pointer_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        match event {
            zwp_locked_pointer_v1::Event::Locked => state.is_pointer_locked = true,
            zwp_locked_pointer_v1::Event::Unlocked => state.is_pointer_locked = false,
            _ => {}
        }
    }
}

/// User data of a capture session: records the buffer size it asks for.
#[derive(Debug, Clone, Copy)]
pub(super) struct CaptureSessionEvents;

impl Dispatch2<ExtImageCopyCaptureSessionV1, ClientState> for CaptureSessionEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        if let ext_image_copy_capture_session_v1::Event::BufferSize { width, height } = event {
            state.capture_size = Some((width, height));
        }
    }
}

/// User data of a capture frame: records how it ended.
#[derive(Debug, Clone, Copy)]
pub(super) struct CaptureFrameEvents;

impl Dispatch2<ExtImageCopyCaptureFrameV1, ClientState> for CaptureFrameEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "only the frame's outcome matters to a test"
        )]
        match event {
            ext_image_copy_capture_frame_v1::Event::Transform {
                transform: WEnum::Value(transform),
            } => state.capture_transform = transform,
            ext_image_copy_capture_frame_v1::Event::Ready => state.capture_result = Some(Ok(())),
            ext_image_copy_capture_frame_v1::Event::Failed { reason } => {
                state.capture_result = Some(Err(format!("{reason:?}")));
            }
            _ => {}
        }
    }
}

impl CompositorHandler for ClientState {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        self.preferred_scale = Some(new_factor);
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for ClientState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for ClientState {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.close_requests += 1;
    }

    fn configure(
        &mut self,
        _: &Connection,
        handle: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        // Take the size vaht asks for, as real clients do; with none, pick.
        if !self.drawn.contains(window.wl_surface()) {
            self.drawn.push(window.wl_surface().clone());
            self.first_draws += 1;
        }
        let (width, height) = configure.new_size;
        let size = (
            width.map_or(WINDOW_SIZE.0, std::num::NonZeroU32::get),
            height.map_or(WINDOW_SIZE.1, std::num::NonZeroU32::get),
        );
        self.draw(window, handle, size);
    }
}

impl ShmHandler for ClientState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for ClientState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState];
}

impl LayerShellHandler for ClientState {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.closed_layers += 1;
    }

    fn configure(
        &mut self,
        _: &Connection,
        handle: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let surface = layer.wl_surface().clone();
        if !self.drawn.contains(&surface) {
            self.drawn.push(surface.clone());
            self.first_draws += 1;
        }
        let color = self
            .layer_colors
            .iter()
            .find(|(known, _)| *known == surface)
            .map_or(WINDOW_COLOR, |(_, color)| *color);
        let (width, height) = configure.new_size;
        self.fill(&surface, handle, (width.max(1), height.max(1)), color);
    }
}

/// The client starts drags but takes no drops: what vaht offers is ignored.
impl DataDeviceHandler for ClientState {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataDevice,
        _: f64,
        _: f64,
        _: &wl_surface::WlSurface,
    ) {
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}

    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
}

impl DataOfferHandler for ClientState {
    fn source_actions(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }

    fn selected_action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
}

/// What the client's drag sources hear; a test only watches windows move.
impl DataSourceHandler for ClientState {
    fn accept_mime(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataSource,
        _: Option<String>,
    ) {
    }

    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataSource,
        _: String,
        _: WritePipe,
    ) {
    }

    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}

    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}

    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}

    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
}

delegate_registry!(ClientState);
smithay_client_toolkit::delegate_dispatch2!(ClientState);
