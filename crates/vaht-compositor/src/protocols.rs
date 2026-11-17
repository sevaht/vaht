//! The Wayland protocols vaht serves, through Smithay.
//!
//! [`ProtocolStates`] holds Smithay's state for each protocol global. The
//! handler traits Smithay calls are implemented on
//! [`crate::Compositor`] in the submodules, one per protocol
//! family. They are gateways (M4): they translate protocol events into calls
//! on vaht's own types and decide nothing themselves.

mod activation;
mod decoration;
mod dmabuf;
mod gamma;
mod idle;
mod input_method;
mod layer_shell;
mod output;
mod output_management;
mod output_power;
mod sandbox;
mod screencopy;
mod seat;
mod selection;
mod session_lock;
mod surfaces;
pub(crate) use self::surfaces::tell_output_scale;
mod toplevel_drag;
mod toplevel_management;
mod virtual_input;
mod window_hints;
mod workspace_pager;
mod xdg_shell;

use smithay::input::SeatState;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::alpha_modifier::AlphaModifierState;
use smithay::wayland::commit_timing::CommitTimingManagerState;
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::content_type::ContentTypeState;
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufState};
use smithay::wayland::fifo::FifoManagerState;
use smithay::wayland::fixes::FixesState;
use smithay::wayland::foreign_toplevel_list::ForeignToplevelListState;
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::idle_inhibit::IdleInhibitManagerState;
use smithay::wayland::idle_notify::IdleNotifierState;
use smithay::wayland::image_capture_source::{
    OutputCaptureSourceState, ToplevelCaptureSourceState,
};
use smithay::wayland::image_copy_capture::ImageCopyCaptureState;
use smithay::wayland::input_method::InputMethodManagerState;
use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::pointer_constraints::PointerConstraintsState;
use smithay::wayland::pointer_gestures::PointerGesturesState;
use smithay::wayland::pointer_warp::PointerWarpManager;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::security_context::SecurityContextState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::{ext_data_control, wlr_data_control};
use smithay::wayland::session_lock::SessionLockManagerState;
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::kde::decoration::KdeDecorationState;
use smithay::wayland::shell::xdg::dialog::XdgDialogState;
use smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::Mode as KdeMode;
use smithay::wayland::shm::ShmState;
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::wayland::tablet_manager::TabletManagerState;
use smithay::wayland::text_input::TextInputManagerState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;
use smithay::wayland::virtual_pointer::VirtualPointerManagerState;
use smithay::wayland::xdg_activation::XdgActivationState;
use smithay::wayland::xdg_foreign::XdgForeignState;
use smithay::wayland::xdg_system_bell::XdgSystemBellState;
use smithay::wayland::xdg_toplevel_icon::XdgToplevelIconManager;
use smithay::wayland::xdg_toplevel_tag::XdgToplevelTagManager;
use smithay::wayland::xwayland_keyboard_grab::XWaylandKeyboardGrabState;
use smithay::wayland::xwayland_shell::XWaylandShellState;

use smithay::reexports::calloop::LoopHandle;
use smithay::utils::{ClockSource, Monotonic};

use crate::Compositor;
use crate::client::ClientState;
use crate::gamma::GammaControls;
use crate::output_management::OutputManagement;
use crate::output_power::OutputPower;
use crate::workspace_pager::WorkspacePager;

/// Smithay's state for every protocol global vaht advertises.
pub(crate) struct ProtocolStates {
    /// `wl_compositor` and `wl_subcompositor`: surfaces and their commits.
    pub(crate) compositor: CompositorState,
    /// `xdg_wm_base`: application windows and popups.
    pub(crate) xdg_shell: XdgShellState,
    /// `zwlr_layer_shell_v1`: wallpapers, panels and notifications.
    pub(crate) layer_shell: WlrLayerShellState,
    /// `wl_shm`: shared-memory buffers.
    pub(crate) shm: ShmState,
    /// `wl_output` and `xdg_output`: what clients know about outputs. Held
    /// only to keep the globals alive.
    _output_manager: OutputManagerState,
    /// `wl_seat`: keyboards and pointers.
    pub(crate) seat: SeatState<Compositor>,
    /// `wl_data_device_manager`: the clipboard and drag and drop.
    pub(crate) data_device: DataDeviceState,
    /// `xdg_toplevel_drag_manager_v1`: windows dragged along with a drag
    /// and drop (a browser's tab dragged out).
    pub(crate) toplevel_drags: toplevel_drag::ToplevelDrags,
    /// `zwlr_screencopy_manager_v1`: screen capture as older tools ask.
    pub(crate) screencopies: screencopy::Screencopies,
    /// `zwp_primary_selection_device_manager_v1`: the middle-click paste.
    pub(crate) primary_selection: PrimarySelectionState,
    /// `ext_data_control_manager_v1` and `zwlr_data_control_manager_v1`:
    /// clipboard managers reading and setting both selections.
    pub(crate) data_control: (
        ext_data_control::DataControlState,
        wlr_data_control::DataControlState,
    ),
    /// `xdg_activation_v1`: programs bringing their windows forward.
    pub(crate) activation: XdgActivationState,
    /// `ext_foreign_toplevel_list_v1`: the windows, listed for docks and
    /// window switchers ([`crate::toplevel_list`]).
    pub(crate) foreign_toplevel_list: ForeignToplevelListState,
    /// `zxdg_exporter_v2` and `zxdg_importer_v2`: a window made the parent
    /// of another program's (a portal's dialog).
    pub(crate) xdg_foreign: XdgForeignState,
    /// `zwp_linux_dmabuf_v1`: GPU buffers.
    pub(crate) dmabuf: DmabufState,
    /// The dmabuf global, when the renderer can import GPU buffers.
    pub(crate) dmabuf_global: Option<DmabufGlobal>,
    /// `wp_viewporter`, `wp_fractional_scale_manager_v1`,
    /// `wp_single_pixel_buffer_manager_v1`, `wp_alpha_modifier_v1`,
    /// `wp_content_type_manager_v1` and `wl_fixes`: surfaces scaled and
    /// cropped, told their scale, filled with one color, made translucent,
    /// saying what they show (video, a game), and registry objects
    /// destroyed. Held only to keep the globals alive; Smithay applies them
    /// when drawing.
    _surface_extensions: (
        ViewporterState,
        FractionalScaleManagerState,
        SinglePixelBufferState,
        AlphaModifierState,
        ContentTypeState,
        FixesState,
    ),
    /// `wp_presentation`: clients told when their frames showed, by the
    /// monotonic clock. Held only to keep the global alive.
    _presentation: PresentationState,
    /// `wp_fifo_manager_v1` and `wp_commit_timing_manager_v1`: frames held
    /// until the last one showed, or until a moment
    /// ([`crate::frame_pacing`]). Held only to keep the globals alive.
    _frame_pacing: (FifoManagerState, CommitTimingManagerState),
    /// `xdg_system_bell_v1` and `xdg_toplevel_tag_manager_v1`: a bell
    /// rung, and a window's tag. Held only to keep the globals alive.
    _window_hints: (XdgSystemBellState, XdgToplevelTagManager),
    /// `zwp_relative_pointer_manager_v1`, `zwp_pointer_constraints_v1`,
    /// `wp_pointer_warp_v1` and `zwp_pointer_gestures_v1`: games reading
    /// the mouse's motion, locking or confining the pointer and moving it,
    /// and touchpad gestures; and `zwp_tablet_manager_v2`, drawing tablets.
    /// Held only to keep the globals.
    _pointer_extensions: (
        RelativePointerManagerState,
        PointerConstraintsState,
        PointerWarpManager,
        PointerGesturesState,
        TabletManagerState,
    ),
    /// `zwp_keyboard_shortcuts_inhibit_manager_v1`: a window taking every
    /// key while it has the keyboard.
    pub(crate) keyboard_shortcuts_inhibit: KeyboardShortcutsInhibitState,
    /// `ext_idle_notifier_v1`: tools such as swayidle hearing when the
    /// session goes idle and comes back.
    pub(crate) idle_notifier: IdleNotifierState<Compositor>,
    /// `zwp_idle_inhibit_manager_v1`: a video player keeping the screen
    /// awake. Held only to keep the global.
    _idle_inhibit: IdleInhibitManagerState,
    /// `ext_session_lock_manager_v1`: lockers such as swaylock.
    pub(crate) session_lock: SessionLockManagerState,
    /// `xwayland_shell_v1`: Xwayland pairing X11 windows with surfaces.
    pub(crate) xwayland_shell: XWaylandShellState,
    /// `zwp_xwayland_keyboard_grab_manager_v1`: X programs grabbing the
    /// keyboard. Held only to keep the global alive.
    _xwayland_keyboard_grab: XWaylandKeyboardGrabState,
    /// `wp_cursor_shape_manager_v1`: clients naming the pointer's picture
    /// (`text`, `pointer`…) rather than drawing it. Held only to keep the
    /// global.
    _cursor_shape: CursorShapeManagerState,
    /// `zxdg_decoration_manager_v1`: vaht draws every frame. Held only to
    /// keep the global alive.
    _xdg_decoration: XdgDecorationState,
    /// `org_kde_kwin_server_decoration_manager`: vaht draws every frame,
    /// for programs that ask KDE's way.
    pub(crate) kde_decoration: KdeDecorationState,
    /// `xdg_wm_dialog_v1`: which windows are modal dialogs.
    _xdg_dialog: XdgDialogState,
    /// `ext_output_image_capture_source_manager_v1`: outputs as things to
    /// capture.
    pub(crate) output_capture_source: OutputCaptureSourceState,
    /// `ext_foreign_toplevel_image_capture_source_manager_v1`: windows as
    /// things to capture, for sharing one window.
    pub(crate) toplevel_capture_source: ToplevelCaptureSourceState,
    /// `ext_image_copy_capture_manager_v1`: screenshots and recording.
    pub(crate) image_copy_capture: ImageCopyCaptureState,
    /// Input from programs: `zwlr_virtual_pointer_manager_v1`,
    /// `zwp_virtual_keyboard_manager_v1`, and `zwp_input_method_manager_v2`
    /// with `zwp_text_input_manager_v3` (input methods composing text for
    /// the focused window). Held only to keep the globals alive.
    _input_from_programs: InputFromPrograms,
    /// `ext_workspace_manager_v1`: the workspaces as pagers see and
    /// switch them.
    pub(crate) workspace_pager: WorkspacePager,
    /// `wp_security_context_manager_v1`: sandbox engines giving their
    /// programs a socket of their own. Held only to keep the global alive.
    _security_context: SecurityContextState,
    /// `xdg_toplevel_icon_manager_v1`: windows naming or drawing their
    /// icon, which their commits carry. Held only to keep the global alive.
    _toplevel_icon: XdgToplevelIconManager,
}

/// The globals through which programs give input: virtual pointers and
/// keyboards, and input methods with the text fields they type into.
type InputFromPrograms = (
    VirtualPointerManagerState,
    VirtualKeyboardManagerState,
    InputMethodManagerState,
    TextInputManagerState,
);

impl ProtocolStates {
    /// Creates the globals through which programs give input on `display`;
    /// sandboxed programs may not inject input, nor be an input method,
    /// which reads every key.
    fn input_from_programs(display: &DisplayHandle) -> InputFromPrograms {
        (
            VirtualPointerManagerState::new::<Compositor, _>(display, ClientState::is_unrestricted),
            VirtualKeyboardManagerState::new::<Compositor, _>(
                display,
                ClientState::is_unrestricted,
            ),
            InputMethodManagerState::new::<Compositor, _>(display, ClientState::is_unrestricted),
            TextInputManagerState::new::<Compositor>(display),
        )
    }

    /// The icon sizes windows are asked to draw, in pixels: around the
    /// taskbar's (18 by default) and larger, which read well scaled down.
    const TOPLEVEL_ICON_SIZES: [i32; 4] = [16, 24, 32, 48];

    /// Creates every global on `display`; idle notifications time out on
    /// `handle`'s loop.
    pub(crate) fn new(display: &DisplayHandle, handle: &LoopHandle<'static, Compositor>) -> Self {
        let primary_selection = PrimarySelectionState::new::<Compositor>(display);
        Self {
            compositor: CompositorState::new::<Compositor>(display),
            xdg_shell: XdgShellState::new::<Compositor>(display),
            layer_shell: WlrLayerShellState::new_with_filter::<Compositor, _>(
                display,
                ClientState::is_unrestricted,
            ),
            shm: ShmState::new::<Compositor>(display, Vec::new()),
            _output_manager: OutputManagerState::new_with_xdg_output::<Compositor>(display),
            seat: SeatState::new(),
            data_device: DataDeviceState::new::<Compositor>(display),
            toplevel_drags: toplevel_drag::ToplevelDrags::new(display),
            screencopies: screencopy::Screencopies::new(display),
            // Sandboxed programs may not read every selection, nor the rest
            // filtered so below.
            data_control: (
                ext_data_control::DataControlState::new::<Compositor, _>(
                    display,
                    Some(&primary_selection),
                    ClientState::is_unrestricted,
                ),
                wlr_data_control::DataControlState::new::<Compositor, _>(
                    display,
                    Some(&primary_selection),
                    ClientState::is_unrestricted,
                ),
            ),
            primary_selection,
            activation: XdgActivationState::new::<Compositor>(display),
            xdg_foreign: XdgForeignState::new::<Compositor>(display),
            foreign_toplevel_list: ForeignToplevelListState::new_with_filter::<Compositor>(
                display,
                ClientState::is_unrestricted,
            ),
            dmabuf: DmabufState::new(),
            dmabuf_global: None,
            _surface_extensions: (
                ViewporterState::new::<Compositor>(display),
                FractionalScaleManagerState::new::<Compositor>(display),
                SinglePixelBufferState::new::<Compositor>(display),
                AlphaModifierState::new::<Compositor>(display),
                ContentTypeState::new::<Compositor>(display),
                FixesState::new::<Compositor>(display),
            ),
            _presentation: PresentationState::new::<Compositor>(
                display,
                <Monotonic as ClockSource>::ID as u32,
            ),
            _frame_pacing: (
                FifoManagerState::new::<Compositor>(display),
                CommitTimingManagerState::new::<Compositor>(display),
            ),
            _window_hints: (
                XdgSystemBellState::new::<Compositor>(display),
                XdgToplevelTagManager::new::<Compositor>(display),
            ),
            _xdg_decoration: XdgDecorationState::new::<Compositor>(display),
            _xdg_dialog: XdgDialogState::new::<Compositor>(display),
            kde_decoration: KdeDecorationState::new::<Compositor>(display, KdeMode::Server),
            _cursor_shape: CursorShapeManagerState::new::<Compositor>(display),
            xwayland_shell: XWaylandShellState::new::<Compositor>(display),
            _xwayland_keyboard_grab: XWaylandKeyboardGrabState::new::<Compositor>(display),
            keyboard_shortcuts_inhibit: KeyboardShortcutsInhibitState::new::<Compositor>(display),
            idle_notifier: IdleNotifierState::new(display, handle.clone()),
            session_lock: SessionLockManagerState::new::<Compositor, _>(
                display,
                ClientState::is_unrestricted,
            ),
            _idle_inhibit: IdleInhibitManagerState::new::<Compositor>(display),
            _pointer_extensions: (
                RelativePointerManagerState::new::<Compositor>(display),
                PointerConstraintsState::new::<Compositor>(display),
                PointerWarpManager::new::<Compositor>(display),
                PointerGesturesState::new::<Compositor>(display),
                TabletManagerState::new::<Compositor>(display),
            ),
            output_capture_source: OutputCaptureSourceState::new_with_filter::<Compositor, _>(
                display,
                ClientState::is_unrestricted,
            ),
            toplevel_capture_source: ToplevelCaptureSourceState::new_with_filter::<Compositor, _>(
                display,
                ClientState::is_unrestricted,
            ),
            image_copy_capture: ImageCopyCaptureState::new_with_filter::<Compositor, _>(
                display,
                ClientState::is_unrestricted,
            ),
            _input_from_programs: Self::input_from_programs(display),
            workspace_pager: WorkspacePager::new(display),
            // A sandboxed program may not make a sandbox of its own.
            _security_context: SecurityContextState::new::<Compositor, _>(
                display,
                ClientState::is_unrestricted,
            ),
            _toplevel_icon: {
                let mut icons = XdgToplevelIconManager::new::<Compositor>(display);
                icons.replace_icon_sizes(Self::TOPLEVEL_ICON_SIZES);
                icons
            },
        }
    }
}

/// What programs outside vaht do to the outputs: arrange them
/// (`zwlr_output_manager_v1`), turn them off (`zwlr_output_power_v1`) and
/// change their colors (`zwlr_gamma_control_v1`).
#[derive(Debug)]
pub(crate) struct OutputControls {
    /// The outputs as programs arranging them see them.
    pub(crate) management: OutputManagement,
    /// Which outputs are off.
    pub(crate) power: OutputPower,
    /// Which program controls each output's colors.
    pub(crate) gamma: GammaControls,
}

impl OutputControls {
    /// Offers the globals on `display`.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        Self {
            management: OutputManagement::new(display),
            power: OutputPower::new(display),
            gamma: GammaControls::new(display),
        }
    }
}

smithay::delegate_dispatch2!(Compositor);
