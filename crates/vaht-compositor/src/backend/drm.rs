//! Running directly on the hardware, from a virtual terminal: DRM for the
//! monitors, libinput for input, and libseat for the right to both.
//!
//! The seat (logind or seatd) hands vaht the GPUs and input devices and
//! takes them back when the user switches to another terminal
//! ([`DrmBackend::pause`], [`DrmBackend::resume`]). udev says when a GPU
//! comes or goes; each GPU's connectors are scanned for monitors
//! ([`GpuDevice`]). Every GPU that can render is added to Smithay's
//! `GpuManager`: the primary one composes, and a monitor on another GPU is
//! drawn there or copied across. Each monitor's frames follow its vertical
//! blank ([`plan::FrameState`]). The decisions are in [`plan`], tested
//! without hardware; this module carries them out.

mod device;
pub(crate) mod plan;

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::{
    DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmNode, NodeType,
};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::input::InputEvent;
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::multigpu::gbm::GbmGlesBackend;
use smithay::backend::renderer::multigpu::{GpuManager, MultiRenderer};
use smithay::backend::renderer::{ImportDma as _, Renderer as _};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session as _};
use smithay::backend::udev::{self, UdevBackend, UdevEvent};
use smithay::output::Output;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::drm::Device as _;
use smithay::reexports::drm::control::crtc;
use smithay::reexports::input::DeviceCapability;
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::{DeviceFd, Transform};

use self::device::{DeviceContext, GpuDevice, GpuOutputManager};
use self::plan::{GpuQuirks, TopWindow};
use super::{BackendError, OutputEvent, RenderError};
use crate::Compositor;
use crate::capture::{self, CaptureError, CapturedImage};
use crate::config::{OutputMode, Vrr};
use crate::event_source::{EventSource, Timer};
use crate::outputs::{OutputControl, Outputs};
use crate::resources::ResourceLedger;
use crate::scene::Scene;

/// The renderers of every GPU.
type GpuRenderers = GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>;

/// A renderer drawing on one GPU, perhaps for another's monitor.
pub(crate) type DrmRenderer<'gpus> = MultiRenderer<
    'gpus,
    'gpus,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
>;

/// The colour formats monitors are driven in, best first: 10 bits per
/// channel where the GPU and monitor can, else 8.
const COLOR_FORMATS: [Fourcc; 4] = [
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];

/// The monitors, GPUs and input devices of the seat vaht runs on.
pub(crate) struct DrmBackend {
    /// The right to the devices, from logind or seatd.
    session: LibSeatSession,
    /// The GPU that composes.
    primary_gpu: DrmNode,
    /// Every GPU's renderer.
    gpus: GpuRenderers,
    /// Every GPU's display side.
    devices: HashMap<DrmNode, GpuDevice>,
    /// The input devices.
    libinput: Libinput,
    /// Where outputs' globals are advertised.
    display: DisplayHandle,
    /// Registers frames and devices' events.
    loop_handle: LoopHandle<'static, Compositor>,
    /// Counts what is registered.
    ledger: ResourceLedger,
    /// The draw arranged for outputs that became due, if one is.
    render_soon: Option<Timer<Compositor>>,
    /// Whether vaht holds the devices (its terminal is the one shown).
    is_active: bool,
    /// The keyboards, whose lights follow the lock keys.
    keyboards: Vec<smithay::reexports::input::Device>,
    /// The lights the keyboards show.
    leds: smithay::reexports::input::Led,
    /// udev, libinput and the session's events, registered while the
    /// backend lives.
    _sources: Vec<EventSource<Compositor>>,
}

impl DrmBackend {
    /// Opens the seat, its GPUs and input devices.
    ///
    /// # Errors
    ///
    /// Returns an error if vaht cannot have the seat (it is not run from a
    /// virtual terminal, or neither logind nor seatd is there), or there is
    /// no GPU.
    pub(crate) fn new(
        handle: &LoopHandle<'static, Compositor>,
        display: &DisplayHandle,
        ledger: &ResourceLedger,
    ) -> Result<Self, BackendError> {
        let (session, notifier) =
            LibSeatSession::new().map_err(|error| BackendError::Seat(error.to_string()))?;
        let seat = session.seat();
        let primary_gpu = Self::find_primary_gpu(&seat)?;
        tracing::info!(seat, gpu = %primary_gpu, "running on the seat");
        // Smithay makes each GPU's EGL display and renderer itself, so vaht
        // needs no unsafe code for them (R-X3).
        let gpus = GpuManager::new(GbmGlesBackend::with_context_priority(ContextPriority::High))
            .map_err(|error| BackendError::Gpu(error.to_string()))?;
        let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
            session.clone().into(),
        );
        libinput
            .udev_assign_seat(&seat)
            .map_err(|()| BackendError::Seat(format!("libinput cannot use seat {seat}")))?;
        let udev =
            UdevBackend::new(&seat).map_err(|error| BackendError::Seat(error.to_string()))?;
        let initial_devices: Vec<_> = udev
            .device_list()
            .map(|(id, path)| (id, path.to_owned()))
            .collect();
        let sources = Self::register(handle, ledger, (udev, notifier), &libinput)?;
        let mut backend = Self {
            session,
            primary_gpu,
            gpus,
            devices: HashMap::new(),
            libinput,
            display: display.clone(),
            loop_handle: handle.clone(),
            ledger: ledger.clone(),
            render_soon: None,
            is_active: true,
            keyboards: Vec::new(),
            leds: smithay::reexports::input::Led::empty(),
            _sources: sources,
        };
        // The primary GPU first, so a display-only one can borrow its
        // renderer.
        let mut ordered = initial_devices;
        ordered.sort_by_key(|(id, _)| DrmNode::from_dev_id(*id).ok() != Some(primary_gpu));
        for (id, path) in ordered {
            let _ = backend.add_device(id, &path);
        }
        if backend.devices.is_empty() {
            return Err(BackendError::Gpu("no GPU could be opened".to_owned()));
        }
        Ok(backend)
    }

    /// Returns the GPU that composes: the one udev calls primary (the one
    /// the firmware booted on), else the first.
    fn find_primary_gpu(seat: &str) -> Result<DrmNode, BackendError> {
        let primary = udev::primary_gpu(seat)
            .ok()
            .flatten()
            .and_then(|path| DrmNode::from_path(path).ok())
            .or_else(|| {
                udev::all_gpus(seat)
                    .ok()?
                    .into_iter()
                    .find_map(|path| DrmNode::from_path(path).ok())
            })
            .ok_or_else(|| BackendError::Gpu(format!("seat {seat} has no GPU")))?;
        Ok(primary
            .node_with_type(NodeType::Primary)
            .and_then(Result::ok)
            .unwrap_or(primary))
    }

    /// Registers udev's, the session's and libinput's events.
    fn register(
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
        (udev, notifier): (
            UdevBackend,
            smithay::backend::session::libseat::LibSeatSessionNotifier,
        ),
        libinput: &Libinput,
    ) -> Result<Vec<EventSource<Compositor>>, BackendError> {
        let register = |error: calloop::Error| BackendError::RegisterEvents(error);
        let udev = EventSource::insert(handle, ledger, udev, |event, _, compositor| {
            compositor.drm_device_event(event);
        })
        .map_err(|error| register(error.error))?;
        let session =
            EventSource::insert(
                handle,
                ledger,
                notifier,
                |event, (), compositor| match event {
                    SessionEvent::PauseSession => compositor.drm_session(false),
                    SessionEvent::ActivateSession => compositor.drm_session(true),
                },
            )
            .map_err(|error| register(error.error))?;
        let input = EventSource::insert(
            handle,
            ledger,
            LibinputInputBackend::new(libinput.clone()),
            |event, (), compositor| {
                if let super::Backend::Drm(backend) = &mut compositor.backend {
                    backend.input_device_event(&event);
                }
                compositor.handle_input(event);
            },
        )
        .map_err(|error| register(error.error))?;
        Ok(vec![udev, session, input])
    }

    /// Opens the GPU `id` at `path` and sets up its monitors; returns the
    /// outputs it brought.
    fn add_device(&mut self, id: DeviceNumber, path: &Path) -> Vec<OutputEvent> {
        let added = DrmNode::from_dev_id(id)
            .map_err(|error| error.to_string())
            .and_then(|node| self.open_device(node, path).map(|()| node));
        match added {
            Ok(node) => self.scan(node),
            Err(error) => {
                tracing::warn!(error, path = %path.display(), "a GPU is not used");
                Vec::new()
            }
        }
    }

    /// Opens the GPU `node` at `path`: its display side, and its renderer
    /// if it can render.
    fn open_device(&mut self, node: DrmNode, path: &Path) -> Result<(), String> {
        let fd = self
            .session
            .open(
                path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
            )
            .map_err(|error| format!("the seat would not open it: {error}"))?;
        let fd = DrmDeviceFd::new(DeviceFd::from(fd));
        let (drm, notifier) = DrmDevice::new(fd.clone(), true)
            .map_err(|error| format!("not a DRM device: {error}"))?;
        let gbm = GbmDevice::new(fd).map_err(|error| format!("no GBM: {error}"))?;
        let render_node = self.add_renderer(node, &gbm);
        let allocator = match render_node {
            Some(_) => GbmAllocator::new(
                gbm.clone(),
                GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
            ),
            None => self
                .devices
                .get(&self.primary_gpu)
                .map(|primary| primary.manager.allocator().clone())
                .ok_or("it cannot render, and there is no primary GPU to render for it")?,
        };
        let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node.into());
        let render_formats: FormatSet = self
            .gpus
            .single_renderer(&render_node.unwrap_or(self.primary_gpu))
            .map_err(|error| format!("no renderer: {error}"))?
            .as_mut()
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .copied()
            .collect();
        let quirks = drm.get_driver().map_or(
            GpuQuirks {
                allows_overlay_planes: true,
            },
            |driver| {
                GpuQuirks::for_driver(
                    &driver.name().to_string_lossy(),
                    &driver.description().to_string_lossy(),
                )
            },
        );
        let manager: GpuOutputManager = GpuOutputManager::new(
            drm,
            allocator,
            exporter,
            Some(gbm),
            COLOR_FORMATS,
            render_formats,
        );
        let events = EventSource::insert(
            &self.loop_handle,
            &self.ledger,
            notifier,
            move |event, metadata, compositor| match event {
                DrmEvent::VBlank(crtc) => compositor.drm_frame_shown(node, crtc, metadata.as_ref()),
                DrmEvent::Error(error) => tracing::warn!(%error, gpu = %node, "GPU error"),
            },
        )
        .map_err(|error| format!("its events cannot be registered: {}", error.error))?;
        tracing::info!(gpu = %node, ?quirks, "GPU opened");
        self.devices.insert(
            node,
            GpuDevice::new(node, render_node, manager, quirks, events),
        );
        Ok(())
    }

    /// Adds `node`'s renderer, if it is a GPU that renders (it has a render
    /// node: a display-only device such as `simpledrm` has none); returns
    /// its render node.
    fn add_renderer(&mut self, node: DrmNode, gbm: &GbmDevice<DrmDeviceFd>) -> Option<DrmNode> {
        let Some(render_node) = node.node_with_type(NodeType::Render).and_then(Result::ok) else {
            tracing::info!(gpu = %node, "display only: another GPU renders for it");
            return None;
        };
        self.gpus
            .as_mut()
            .add_node(render_node, gbm.clone())
            .inspect_err(|error| tracing::warn!(%error, gpu = %node, "cannot render with it"))
            .ok()?;
        Some(render_node)
    }

    /// Scans the GPU `node`'s connectors; returns what changed.
    fn scan(&mut self, node: DrmNode) -> Vec<OutputEvent> {
        let primary = self.primary_gpu;
        let Some(device) = self.devices.get_mut(&node) else {
            return Vec::new();
        };
        let render_node = device.render_node.unwrap_or(primary);
        let Ok(mut renderer) = self.gpus.single_renderer(&render_node) else {
            return Vec::new();
        };
        let mut context = DeviceContext {
            display: &self.display,
            renderer: &mut renderer,
        };
        let events = device.scan(&mut context);
        self.schedule_render();
        events
    }

    /// Follows udev: a GPU added, changed (a monitor plugged or
    /// unplugged) or removed; returns what changed in the outputs.
    pub(crate) fn device_event(&mut self, event: UdevEvent) -> Vec<OutputEvent> {
        match event {
            UdevEvent::Added { device_id, path } => self.add_device(device_id, &path),
            UdevEvent::Changed { device_id } => DrmNode::from_dev_id(device_id)
                .map(|node| self.scan(node))
                .unwrap_or_default(),
            UdevEvent::Removed { device_id } => {
                let Ok(node) = DrmNode::from_dev_id(device_id) else {
                    return Vec::new();
                };
                let Some(mut device) = self.devices.remove(&node) else {
                    return Vec::new();
                };
                if let Some(render_node) = device.render_node {
                    self.gpus.as_mut().remove_node(&render_node);
                }
                tracing::info!(gpu = %node, "GPU removed");
                device.drain(&self.display)
            }
        }
    }

    /// Returns the monitors, laid out side by side; with none plugged in,
    /// a placeholder no client sees, until one is.
    pub(crate) fn outputs(&self) -> Outputs {
        let mut outputs: Vec<Output> = self
            .devices
            .values()
            .flat_map(|device| device.screens.values().map(|screen| screen.output.clone()))
            .collect();
        outputs.sort_by_key(Output::name);
        if outputs.is_empty() {
            return Outputs::waiting();
        }
        let first = outputs.remove(0);
        Outputs::in_a_row(first, outputs)
    }

    /// Lets go of the devices: the user switched to another terminal.
    pub(crate) fn pause(&mut self) {
        tracing::info!("another terminal is shown; pausing");
        self.is_active = false;
        self.libinput.suspend();
        for device in self.devices.values_mut() {
            device.manager.pause();
            for screen in device.screens.values_mut() {
                screen.frames.reset();
                screen.throttle = None;
            }
        }
    }

    /// Takes the devices back: vaht's terminal is shown again. Every
    /// output is drawn anew, and monitors changed meanwhile are found.
    pub(crate) fn resume(&mut self) -> Vec<OutputEvent> {
        tracing::info!("vaht's terminal is shown again; resuming");
        self.is_active = true;
        if self.libinput.resume().is_err() {
            tracing::warn!("input devices could not be resumed");
        }
        let nodes: Vec<DrmNode> = self.devices.keys().copied().collect();
        let mut events = Vec::new();
        for node in nodes {
            if let Some(device) = self.devices.get_mut(&node)
                && let Err(error) = device.manager.lock().activate(false)
            {
                tracing::warn!(%error, gpu = %node, "could not take the GPU back");
            }
            events.extend(self.scan(node));
        }
        self.schedule_render();
        events
    }

    /// Switches to virtual terminal `terminal`.
    pub(crate) fn switch_terminal(&mut self, terminal: i32) {
        if let Err(error) = self.session.change_vt(terminal) {
            tracing::warn!(%error, terminal, "could not switch terminals");
        }
    }

    /// Asks for a frame on every output; those not waiting for one are
    /// drawn soon.
    pub(crate) fn schedule_render(&mut self) {
        let mut is_any_due = false;
        for device in self.devices.values_mut() {
            for screen in device.screens.values_mut() {
                is_any_due |= screen.frames.request();
                is_any_due |= screen.frames.is_due();
            }
        }
        if is_any_due {
            self.render_soon();
        }
    }

    /// Arranges a draw at once, unless one is arranged.
    fn render_soon(&mut self) {
        if !self.is_active
            || self
                .render_soon
                .as_ref()
                .is_some_and(|timer| !timer.has_fired())
        {
            return;
        }
        match Timer::once(
            &self.loop_handle,
            &self.ledger,
            Duration::ZERO,
            Compositor::render,
        ) {
            Ok(timer) => self.render_soon = Some(timer),
            Err(error) => tracing::warn!(%error, "could not arrange a frame"),
        }
    }

    /// Notes that GPU `node`'s output on `crtc` showed its frame, as its
    /// page flip's `metadata` says; draws again if one is due.
    pub(crate) fn frame_shown(
        &mut self,
        node: DrmNode,
        crtc: crtc::Handle,
        metadata: Option<&DrmEventMetadata>,
    ) {
        let is_due = self
            .devices
            .get_mut(&node)
            .is_some_and(|device| device.frame_shown(crtc, metadata));
        if is_due {
            self.render_soon();
        }
    }

    /// Notes that GPU `node`'s output on `crtc` waited out a refresh after
    /// an unchanged frame; draws again if one is due.
    pub(crate) fn wait_over(&mut self, node: DrmNode, crtc: crtc::Handle) {
        let is_due = self
            .devices
            .get_mut(&node)
            .is_some_and(|device| device.wait_over(crtc));
        if is_due {
            self.render_soon();
        }
    }

    /// Follows libinput's news of an input device: keyboards are kept, so
    /// their lights can show Caps Lock and Num Lock.
    pub(crate) fn input_device_event(&mut self, event: &InputEvent<LibinputInputBackend>) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "only devices coming and going matter here; handle_input takes the rest"
        )]
        match event {
            InputEvent::DeviceAdded { device }
                if device.has_capability(DeviceCapability::Keyboard) =>
            {
                let mut device = device.clone();
                device.led_update(self.leds);
                self.keyboards.push(device);
            }
            InputEvent::DeviceRemoved { device } => {
                self.keyboards.retain(|keyboard| keyboard != device);
            }
            _ => {}
        }
    }

    /// Lights each keyboard's Caps Lock, Num Lock and Scroll Lock as `leds`
    /// says.
    pub(crate) fn set_keyboard_leds(&mut self, leds: smithay::input::keyboard::LedState) {
        self.leds = leds.into();
        for keyboard in &mut self.keyboards {
            keyboard.led_update(self.leds);
        }
    }

    /// Draws `scene` on every output that is due.
    ///
    /// # Errors
    ///
    /// Returns the first error; the other outputs are still drawn.
    pub(crate) fn render(&mut self, scene: &Scene<'_>) -> Result<(), RenderError> {
        if !self.is_active {
            return Ok(());
        }
        let primary = self.primary_gpu;
        let mut result = Ok(());
        let nodes: Vec<DrmNode> = self.devices.keys().copied().collect();
        for node in nodes {
            let Some(device) = self.devices.get_mut(&node) else {
                continue;
            };
            let render_node = device.render_node.unwrap_or(primary);
            let renderer = if render_node == primary {
                self.gpus.single_renderer(&primary)
            } else {
                self.gpus.renderer(&primary, &render_node, Fourcc::Argb8888)
            };
            let mut renderer = match renderer {
                Ok(renderer) => renderer,
                Err(error) => {
                    result = result.and(Err(RenderError::Hardware(error.to_string())));
                    continue;
                }
            };
            match device.render(&mut renderer, scene) {
                Ok(unchanged) => {
                    for crtc in unchanged {
                        Self::throttle(device, crtc, (&self.loop_handle, &self.ledger));
                    }
                }
                Err(error) => result = result.and(Err(error)),
            }
        }
        result
    }

    /// Waits one refresh interval after `crtc`'s unchanged frame before
    /// letting it draw again.
    fn throttle(
        device: &mut GpuDevice,
        crtc: crtc::Handle,
        (handle, ledger): (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) {
        let node = device.node;
        let Some(screen) = device.screens.get_mut(&crtc) else {
            return;
        };
        match Timer::once(handle, ledger, screen.frame_interval(), move |compositor| {
            compositor.drm_wait_over(node, crtc);
        }) {
            Ok(timer) => screen.throttle = Some(timer),
            Err(error) => tracing::warn!(%error, "could not wait for the next frame"),
        }
    }

    /// Turns variable refresh on `output` on or off as `vrr` says, with
    /// `top` the window on top there.
    pub(crate) fn set_vrr(&mut self, output: &Output, vrr: Vrr, top: TopWindow) {
        if let Some(screen) = self
            .devices
            .values_mut()
            .find_map(|device| device.screen_mut(output))
        {
            screen.set_vrr(vrr, top);
        }
    }

    /// Returns the primary GPU's renderer.
    pub(crate) fn renderer(&mut self) -> Result<DrmRenderer<'_>, RenderError> {
        self.gpus
            .single_renderer(&self.primary_gpu)
            .map_err(|error| RenderError::Hardware(error.to_string()))
    }

    /// Returns the GPU buffer formats the primary GPU can import.
    pub(crate) fn dmabuf_formats(&mut self) -> Vec<smithay::backend::allocator::Format> {
        self.renderer()
            .map(|renderer| renderer.dmabuf_formats().into_iter().collect())
            .unwrap_or_default()
    }

    /// Imports a client's GPU buffer; returns whether it can be drawn.
    pub(crate) fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        self.renderer()
            .is_ok_and(|mut renderer| renderer.import_dmabuf(dmabuf, None).is_ok())
    }

    /// Frees textures cached for client buffers that no longer exist.
    pub(crate) fn release_unused_textures(&mut self) {
        if let Ok(mut renderer) = self.renderer()
            && let Err(error) = renderer.cleanup_texture_cache()
        {
            tracing::warn!(%error, "could not free unused textures");
        }
    }

    /// Draws `scene` as `output` shows it, for a screenshot.
    ///
    /// # Errors
    ///
    /// Returns why the image could not be made.
    pub(crate) fn capture(
        &mut self,
        scene: &Scene<'_>,
        output: &Output,
    ) -> Result<CapturedImage, CaptureError> {
        let mut renderer = self
            .renderer()
            .map_err(|error| CaptureError::Draw(error.to_string()))?;
        capture::draw_capture::<_, smithay::backend::renderer::gles::GlesTexture>(
            &mut renderer,
            scene,
            output,
        )
    }
}

impl OutputControl for DrmBackend {
    fn set_enabled(&mut self, output: &Output, is_enabled: bool) {
        let display = self.display.clone();
        if let Some(screen) = self
            .devices
            .values_mut()
            .find_map(|device| device.screen_mut(output))
        {
            screen.set_enabled(is_enabled, &display);
        }
        if is_enabled {
            self.render_soon();
        }
    }

    fn set_mode(&mut self, output: &Output, mode: Option<OutputMode>) {
        let primary = self.primary_gpu;
        for device in self.devices.values_mut() {
            let render_node = device.render_node.unwrap_or(primary);
            if let Some(screen) = device.screen_mut(output)
                && let Ok(mut renderer) = self.gpus.single_renderer(&render_node)
            {
                screen.set_mode(mode, &mut renderer);
            }
        }
    }

    fn set_transform(&mut self, output: &Output, transform: Transform) {
        // The output's compositor follows the output's own state.
        output.change_current_state(None, Some(transform), None, None);
    }
}

impl std::fmt::Debug for DrmBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DrmBackend")
            .field("primary_gpu", &self.primary_gpu)
            .field("devices", &self.devices.len())
            .field("is_active", &self.is_active)
            .finish_non_exhaustive()
    }
}

/// A device number, as udev gives one.
type DeviceNumber = smithay::reexports::rustix::fs::Dev;
