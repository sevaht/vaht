//! What a test client knows and has, and the event handling that keeps it
//! up to date: the toolkit's handlers, and the protocol objects whose events
//! the client records or ignores.

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, FrameCallbackData};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::globals::GlobalList;
use smithay_client_toolkit::reexports::client::protocol::{wl_buffer, wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::protocols::wp::pointer_constraints::zv1::client::zwp_locked_pointer_v1::{self, ZwpLockedPointerV1};
use smithay_client_toolkit::reexports::protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1;
use smithay_client_toolkit::reexports::protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_v1::{self, ZwpRelativePointerV1};
use smithay_client_toolkit::reexports::protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use smithay_client_toolkit::reexports::protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::reexports::protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
use smithay_client_toolkit::reexports::protocols::ext::image_capture_source::v1::client::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1;
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
use smithay_client_toolkit::{delegate_registry, registry_handlers};

use super::{BYTES_PER_PIXEL, WINDOW_COLOR, WINDOW_SIZE};

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
    /// The surface with keyboard focus, as vaht last told.
    pub(super) keyboard_focus: Option<wl_surface::WlSurface>,
    /// How far the mouse moved, as relative motion told, in all.
    pub(super) relative_motion: (f64, f64),
    /// Whether vaht holds the pointer locked for the client.
    pub(super) is_pointer_locked: bool,
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
            icon_buffers: Vec::new(),
            pool: SlotPool::new(pool_size, &shm).expect("a shared-memory pool"),
            shm,
            windows: Vec::new(),
            first_draws: 0,
            drawn: Vec::new(),
            close_requests: 0,
            closed_layers: 0,
            capture_size: None,
            capture_result: None,
            keyboard_focus: None,
            relative_motion: (0.0, 0.0),
            is_pointer_locked: false,
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
    wl_pointer::WlPointer,
    ZwpRelativePointerManagerV1,
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
        _: i32,
    ) {
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

delegate_registry!(ClientState);
smithay_client_toolkit::delegate_dispatch2!(ClientState);
