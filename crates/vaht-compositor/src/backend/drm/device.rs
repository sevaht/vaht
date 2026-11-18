//! One GPU's display side: its connectors, the outputs on them, and their
//! frames.
//!
//! A [`GpuDevice`] scans its connectors and makes an [`Output`] for each
//! monitor plugged in, with a `DrmOutput` that composes the scene's
//! elements onto the monitor's planes (a window on a plane of its own
//! skips the GPU entirely). Its decisions (which mode, which planes, when
//! the next frame) are the planner's ([`super::plan`]); this only carries
//! them out.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::GbmAllocator;
use smithay::backend::drm::compositor::{DrmCompositor, FrameFlags};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{DrmDeviceFd, DrmEventMetadata, DrmNode, DrmSurface, VrrSupport};
use smithay::backend::renderer::ImportDma as _;
use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags;
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::output::{Mode as OutputModeInfo, Output, PhysicalProperties};
use smithay::reexports::drm::control::{Device as _, Mode, ModeTypeFlags, connector, crtc};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay_drm_extras::display_info;
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};

use super::DrmRenderer;
use super::hdr::{self, HdrCapability};
use super::lease::Headset;
use super::plan::{self, ConnectorMode, FrameState, GpuQuirks, ModeFit, TopWindow};
use crate::Compositor;
use crate::backend::{OutputEvent, RenderError};
use crate::color::ImageDescription;
use crate::config::{HdrRequest, OutputMode, Vrr};
use crate::event_source::{EventSource, Timer};
use crate::gamma::GammaRamps;
use crate::presentation;
use crate::scene::SurfaceDmabufFeedback;
use crate::scene::{self, OutputElement, Scene};

/// How a GPU's outputs are driven.
pub(super) type GpuOutputManager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    OutputPresentationFeedback,
    DrmDeviceFd,
>;

/// One output's driver.
type GpuOutput = DrmOutput<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    OutputPresentationFeedback,
    DrmDeviceFd,
>;

/// One GPU's display side.
pub(super) struct GpuDevice {
    /// The GPU's display node.
    pub(super) node: DrmNode,
    /// The node that renders for it: its own, or none for a display-only
    /// device another GPU renders for.
    pub(super) render_node: Option<DrmNode>,
    /// Drives its outputs.
    pub(super) manager: GpuOutputManager,
    /// Notices connectors plugged and unplugged.
    scanner: DrmScanner,
    /// What its driver needs done differently.
    quirks: GpuQuirks,
    /// Each monitor's output, by the CRTC driving it.
    pub(super) screens: HashMap<crtc::Handle, Screen>,
    /// The headsets plugged in, left dark for programs to lease.
    pub(super) headsets: Vec<Headset>,
    /// Its vertical blank events, registered while it lives.
    pub(super) _events: EventSource<Compositor>,
}

/// Whether a monitor is in HDR, and how.
#[derive(Debug, Clone, Copy, PartialEq)]
enum HdrMode {
    /// In SDR.
    Sdr,
    /// In HDR10.
    Hdr {
        /// Its peak, in cd/m².
        peak: f64,
        /// SDR white, in cd/m².
        sdr_white: f64,
    },
}

impl Drop for GpuDevice {
    fn drop(&mut self) {
        // A monitor left in HDR would show the console, or the next
        // compositor, washed out.
        self.leave_hdr();
    }
}

/// One monitor's output.
pub(super) struct Screen {
    /// The output clients see.
    pub(super) output: Output,
    /// The connector it is on.
    connector: connector::Info,
    /// Composes its frames.
    drm_output: GpuOutput,
    /// Its global while it is on.
    global: Option<GlobalId>,
    /// Where its frames are.
    pub(super) frames: FrameState,
    /// The wait after a frame that changed nothing.
    pub(super) throttle: Option<Timer<Compositor>>,
    /// When the frame on its way went to the monitor, for the trace of
    /// how long the monitor took to show it.
    queued_at: Option<Instant>,
    /// Whether its monitor is awake (`zwlr_output_power_v1`).
    is_powered: bool,
    /// The colors a night-light tool set, if one did.
    gamma: Option<GammaRamps>,
    /// What its monitor can show in HDR, if it can.
    hdr_capability: Option<HdrCapability>,
    /// Whether it is in HDR, and how.
    hdr: HdrMode,
    /// What the kernel last refused, so the refusal is logged once.
    hdr_refused: Option<HdrMode>,
    /// What clients on it are told of buffers, as their surfaces are
    /// scanned out or composited.
    dmabuf_feedback: Option<SurfaceDmabufFeedback>,
}

/// What a GPU device needs from the rest of the backend to change its
/// outputs: where globals go, and a renderer for the device.
pub(super) struct DeviceContext<'context, 'renderer> {
    /// Where the outputs' globals are advertised.
    pub(super) display: &'context DisplayHandle,
    /// A renderer for this device.
    pub(super) renderer: &'context mut DrmRenderer<'renderer>,
    /// The render nodes of the primary GPU and of the one rendering for
    /// this device.
    pub(super) nodes: (DrmNode, DrmNode),
    /// The formats the primary GPU's renderer reads.
    pub(super) primary_formats: &'context FormatSet,
}

impl GpuDevice {
    /// A GPU's display side, before its connectors are scanned.
    pub(super) fn new(
        node: DrmNode,
        render_node: Option<DrmNode>,
        manager: GpuOutputManager,
        quirks: GpuQuirks,
        events: EventSource<Compositor>,
    ) -> Self {
        Self {
            node,
            render_node,
            manager,
            scanner: DrmScanner::new(),
            quirks,
            screens: HashMap::new(),
            headsets: Vec::new(),
            _events: events,
        }
    }

    /// Scans the connectors again, setting up each monitor plugged in
    /// since and dropping each unplugged; returns what changed.
    pub(super) fn scan(&mut self, context: &mut DeviceContext<'_, '_>) -> Vec<OutputEvent> {
        let scan = match self.scanner.scan_connectors(self.manager.device()) {
            Ok(scan) => scan,
            Err(error) => {
                tracing::warn!(%error, node = %self.node, "could not scan the connectors");
                return Vec::new();
            }
        };
        let mut events = Vec::new();
        for event in scan {
            match event {
                DrmScanEvent::Connected {
                    connector,
                    crtc: Some(crtc),
                } => {
                    if let Some(output) = self.connect(connector, crtc, context) {
                        events.push(OutputEvent::Connected(output));
                    }
                }
                DrmScanEvent::Disconnected {
                    crtc: Some(crtc), ..
                } => {
                    self.headsets.retain(|headset| headset.crtc != crtc);
                    if let Some(mut screen) = self.screens.remove(&crtc) {
                        if let Some(global) = screen.global.take() {
                            context.display.remove_global::<Compositor>(global);
                        }
                        events.push(OutputEvent::Disconnected(screen.output));
                    }
                }
                DrmScanEvent::Changed { connector, .. } => {
                    if let Some(screen) = self
                        .screens
                        .values_mut()
                        .find(|screen| screen.connector.handle() == connector.handle())
                    {
                        screen.connector = connector;
                        screen.set_preferred_mode();
                        events.push(OutputEvent::Changed(screen.output.clone()));
                    }
                }
                DrmScanEvent::Connected {
                    crtc: None,
                    connector,
                } => tracing::warn!(
                    connector = Screen::connector_name(&connector),
                    "no CRTC is free to drive a monitor plugged in"
                ),
                DrmScanEvent::Disconnected { crtc: None, .. } => {}
            }
        }
        events
    }

    /// Returns every output, for when the device goes away.
    pub(super) fn drain(&mut self, display: &DisplayHandle) -> Vec<OutputEvent> {
        self.screens
            .drain()
            .map(|(_, mut screen)| {
                if let Some(global) = screen.global.take() {
                    display.remove_global::<Compositor>(global);
                }
                OutputEvent::Disconnected(screen.output)
            })
            .collect()
    }

    /// Sets up the monitor on `connector`, driven by `crtc`; none if it is
    /// not a desktop monitor or cannot be driven.
    fn connect(
        &mut self,
        connector: connector::Info,
        crtc: crtc::Handle,
        context: &mut DeviceContext<'_, '_>,
    ) -> Option<Output> {
        let name = Screen::connector_name(&connector);
        if self.is_non_desktop(connector.handle()) {
            self.add_headset(&connector, crtc, name);
            return None;
        }
        let modes = Screen::modes_of(&connector);
        let Some((index, _)) = plan::choose_mode(&modes, None) else {
            tracing::warn!(connector = name, "the monitor offers no mode");
            return None;
        };
        let mode = connector.modes()[index];
        let info = display_info::for_connector(self.manager.device(), connector.handle());
        let hdr_capability = info.as_ref().and_then(|info| {
            let metadata = info.hdr_static_metadata();
            let takes_hdr10 = metadata.pq && info.supported_signal_colorimetry().bt2020_rgb;
            HdrCapability::of(
                takes_hdr10,
                [
                    metadata.desired_content_max_luminance,
                    metadata.desired_content_max_frame_avg_luminance,
                    metadata.desired_content_min_luminance,
                ],
            )
        });
        let (make, display_model, serial_number) = info.map_or_else(Default::default, |info| {
            (
                info.make().unwrap_or_default(),
                info.model().unwrap_or_default(),
                info.serial().unwrap_or_default(),
            )
        });
        let (width, height) = connector.size().unwrap_or((0, 0));
        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: (
                    i32::try_from(width).unwrap_or_default(),
                    i32::try_from(height).unwrap_or_default(),
                )
                    .into(),
                subpixel: connector.subpixel().into(),
                make,
                model: display_model,
                serial_number,
            },
        );
        let output_mode = OutputModeInfo::from(mode);
        output.set_preferred(output_mode);
        output.change_current_state(Some(output_mode), None, None, None);
        let planes = self.manager.device().planes(&crtc).ok().map(|mut planes| {
            if !self.quirks.allows_overlay_planes {
                planes.overlay.clear();
            }
            planes
        });
        let drm_output = match self
            .manager
            .lock()
            .initialize_output::<_, OutputElement<DrmRenderer<'_>>>(
                crtc,
                mode,
                &[connector.handle()],
                &output,
                planes,
                context.renderer,
                &DrmOutputRenderElements::default(),
            ) {
            Ok(drm_output) => drm_output,
            Err(error) => {
                tracing::warn!(?error, connector = name, "could not drive the monitor");
                return None;
            }
        };
        let dmabuf_feedback = drm_output
            .with_compositor(|compositor| Self::dmabuf_feedback(context, compositor.surface()));
        let global = output.create_global::<Compositor>(context.display);
        tracing::info!(connector = name, mode = ?output_mode, "monitor set up");
        self.screens.insert(
            crtc,
            Screen {
                output: output.clone(),
                connector,
                drm_output,
                global: Some(global),
                frames: FrameState::Due,
                throttle: None,
                queued_at: None,
                is_powered: true,
                gamma: None,
                hdr_capability,
                hdr: HdrMode::Sdr,
                hdr_refused: None,
                dmabuf_feedback,
            },
        );
        Some(output)
    }

    /// The `linux-dmabuf` versions a tranche for another GPU's sampling
    /// is sent to, as Smithay's own compositor has them.
    const SAMPLING_VERSIONS: std::ops::RangeInclusive<u32> = 3..=6;

    /// The `linux-dmabuf` versions the scan-out tranches are sent to.
    const SCANOUT_VERSIONS: std::ops::RangeInclusive<u32> = 4..=6;

    /// Returns the `linux-dmabuf` feedback for client surfaces on the
    /// display `surface` drives, as `context`'s GPUs read and its planes
    /// scan out: the scan-out tranche lists only formats the renderer
    /// reads too, so a buffer that cannot go on a plane can always be
    /// drawn instead.
    fn dmabuf_feedback(
        context: &DeviceContext<'_, '_>,
        surface: &DrmSurface,
    ) -> Option<SurfaceDmabufFeedback> {
        let (primary, render_node) = context.nodes;
        let render_formats = context.renderer.dmabuf_formats();
        let readable: FormatSet = context
            .primary_formats
            .iter()
            .chain(render_formats.iter())
            .copied()
            .collect();
        let planes = surface.planes();
        let plane_formats: FormatSet = surface
            .plane_info()
            .formats
            .iter()
            .copied()
            .chain(
                planes
                    .overlay
                    .iter()
                    .flat_map(|plane| plane.formats.iter().copied()),
            )
            .collect::<FormatSet>()
            .intersection(&readable)
            .copied()
            .collect();
        let builder = DmabufFeedbackBuilder::new(primary.dev_id(), context.primary_formats.clone());
        let render = if render_node == primary {
            builder.clone().build()
        } else {
            builder
                .clone()
                .add_preference_tranche(
                    render_node.dev_id(),
                    TrancheFlags::Sampling,
                    render_formats.clone(),
                    Self::SAMPLING_VERSIONS,
                )
                .build()
        };
        let scanout = builder
            .add_preference_tranche(
                surface.device_fd().dev_id().ok()?,
                TrancheFlags::Scanout,
                plane_formats,
                Self::SCANOUT_VERSIONS,
            )
            .add_preference_tranche(
                render_node.dev_id(),
                TrancheFlags::Sampling,
                render_formats,
                Self::SCANOUT_VERSIONS,
            )
            .build();
        match (render, scanout) {
            (Ok(render), Ok(scanout)) => Some(SurfaceDmabufFeedback { render, scanout }),
            (Err(error), _) | (_, Err(error)) => {
                tracing::warn!(%error, "no dmabuf feedback for the monitor");
                None
            }
        }
    }

    /// Leaves the headset on `connector` dark, driven by `crtc` once a
    /// program leases it.
    fn add_headset(&mut self, connector: &connector::Info, crtc: crtc::Handle, name: String) {
        tracing::info!(
            connector = name,
            "not a desktop monitor (a headset); left for leasing"
        );
        let description = display_info::for_connector(self.manager.device(), connector.handle())
            .map(|info| {
                format!(
                    "{} {}",
                    info.make().unwrap_or_default(),
                    info.model().unwrap_or_default()
                )
                .trim()
                .to_owned()
            })
            .unwrap_or_default();
        self.headsets
            .retain(|headset| headset.connector != connector.handle());
        self.headsets.push(Headset {
            connector: connector.handle(),
            crtc,
            name,
            description,
        });
    }

    /// Returns whether the connector `handle` says its monitor is not for
    /// the desktop (a VR headset, leased to the program that drives it).
    fn is_non_desktop(&self, handle: connector::Handle) -> bool {
        let device = self.manager.device();
        let Ok(properties) = device.get_properties(handle) else {
            return false;
        };
        properties.into_iter().any(|(property, value)| {
            device.get_property(property).is_ok_and(|info| {
                info.name().to_str() == Ok("non-desktop")
                    && info.value_type().convert_value(value).as_boolean() == Some(true)
            })
        })
    }

    /// Returns the screen showing `output`.
    pub(super) fn screen_mut(&mut self, output: &Output) -> Option<&mut Screen> {
        self.screens
            .values_mut()
            .find(|screen| screen.output == *output)
    }

    /// Returns how many values each of `output`'s gamma ramps has, if it
    /// is on this device.
    pub(super) fn gamma_size(&self, output: &Output) -> Option<u32> {
        let (crtc, _) = self
            .screens
            .iter()
            .find(|(_, screen)| screen.output == *output)?;
        let size = self.manager.device().get_crtc(*crtc).ok()?.gamma_length();
        (size > 0).then_some(size)
    }

    /// Changes `output`'s colors by `ramps`, or back to normal without.
    pub(super) fn set_gamma(&mut self, output: &Output, ramps: Option<&GammaRamps>) {
        let Some(size) = self.gamma_size(output) else {
            return;
        };
        let Some((crtc, screen)) = self
            .screens
            .iter_mut()
            .find(|(_, screen)| screen.output == *output)
        else {
            return;
        };
        screen.gamma = ramps.cloned();
        let ramps = ramps.cloned().unwrap_or_else(|| GammaRamps::identity(size));
        Self::apply_gamma(&self.manager, *crtc, &ramps);
    }

    /// Applies the colors night-light tools set again, as after another
    /// program had the GPU.
    pub(super) fn reapply_gamma(&self) {
        for (crtc, screen) in &self.screens {
            if let Some(ramps) = &screen.gamma {
                Self::apply_gamma(&self.manager, *crtc, ramps);
            }
        }
    }

    /// Puts `output`'s monitor, if it is this device's, into HDR as
    /// `request` says, or out of it with none or when it cannot; returns
    /// whether that changed what it shows.
    pub(super) fn set_hdr(&mut self, output: &Output, request: Option<HdrRequest>) -> bool {
        let device = self.manager.device();
        let Some(screen) = self
            .screens
            .values_mut()
            .find(|screen| screen.output == *output)
        else {
            return false;
        };
        let wanted =
            request
                .zip(screen.hdr_capability)
                .map_or(HdrMode::Sdr, |(request, capability)| HdrMode::Hdr {
                    peak: request.peak.unwrap_or(capability.peak),
                    sdr_white: request.sdr_white,
                });
        if wanted == screen.hdr {
            return false;
        }
        let signal = match (wanted, screen.hdr_capability.as_ref()) {
            (HdrMode::Hdr { peak, .. }, Some(capability)) => Some((capability, peak)),
            (HdrMode::Sdr | HdrMode::Hdr { .. }, _) => None,
        };
        let signalled = hdr::signal(device, screen.connector.handle(), signal);
        if let Err(error) = signalled {
            if screen.hdr_refused != Some(wanted) {
                tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    output = screen.output.name(),
                    ?wanted,
                    "could not change HDR; trying again each frame"
                );
                screen.hdr_refused = Some(wanted);
            }
            return false;
        }
        tracing::info!(output = screen.output.name(), ?wanted, "HDR changed");
        screen.hdr = wanted;
        screen.hdr_refused = None;
        let description = match wanted {
            HdrMode::Sdr => ImageDescription::SRGB,
            HdrMode::Hdr { peak, sdr_white } => ImageDescription::hdr10(peak, sdr_white),
        };
        description.set_for_output(&screen.output);
        // Every pixel's encoding changed: draw everything anew.
        screen.drm_output.reset_buffers();
        true
    }

    /// Takes every monitor of this device out of HDR, as vaht leaves.
    pub(super) fn leave_hdr(&mut self) {
        let device = self.manager.device();
        for screen in self.screens.values_mut() {
            if std::mem::replace(&mut screen.hdr, HdrMode::Sdr) != HdrMode::Sdr
                && let Err(error) = hdr::signal(device, screen.connector.handle(), None)
            {
                tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    output = screen.output.name(),
                    "could not take the monitor out of HDR"
                );
            }
        }
    }

    /// Sets `crtc`'s gamma ramps to `ramps`.
    fn apply_gamma(manager: &GpuOutputManager, crtc: crtc::Handle, ramps: &GammaRamps) {
        if let Err(error) = manager
            .device()
            .set_gamma(crtc, &ramps.red, &ramps.green, &ramps.blue)
        {
            tracing::warn!(%error, "could not change the monitor's colors");
        }
    }

    /// Draws `scene` on each of this device's outputs that is due and on;
    /// returns the CRTCs of those that changed nothing, which wait a
    /// refresh before the next frame.
    pub(super) fn render(
        &mut self,
        renderer: &mut DrmRenderer<'_>,
        scene: &Scene<'_>,
    ) -> Result<Vec<crtc::Handle>, RenderError> {
        let mut unchanged = Vec::new();
        for (crtc, screen) in &mut self.screens {
            if screen.global.is_none() || !screen.is_powered || !screen.frames.is_due() {
                continue;
            }
            if !screen.render(renderer, scene)? {
                unchanged.push(*crtc);
            }
        }
        Ok(unchanged)
    }

    /// Notes that `crtc`'s wait after an unchanged frame is over; returns
    /// whether a frame is due. The wait's timer, which calls this, stays
    /// until replaced: a timer is never dropped in its own callback.
    pub(super) fn wait_over(&mut self, crtc: crtc::Handle) -> bool {
        self.screens
            .get_mut(&crtc)
            .is_some_and(|screen| screen.frames.finished())
    }

    /// Notes that `crtc`'s frame was shown, as its page flip's `metadata`
    /// says, and tells the clients in it; returns whether another is due.
    pub(super) fn frame_shown(
        &mut self,
        crtc: crtc::Handle,
        metadata: Option<&DrmEventMetadata>,
    ) -> bool {
        let Some(screen) = self.screens.get_mut(&crtc) else {
            return false;
        };
        match screen.drm_output.frame_submitted() {
            Ok(Some(feedback)) => {
                let is_variable = screen
                    .drm_output
                    .with_compositor(|compositor| compositor.vrr_enabled());
                presentation::report_shown(
                    feedback,
                    presentation::shown_at(metadata, presentation::now()),
                    metadata.map_or(0, |metadata| u64::from(metadata.sequence)),
                    (screen.frame_interval(), is_variable),
                );
            }
            Ok(None) => {}
            Err(error) => tracing::debug!(?error, "a frame was not shown"),
        }
        if let Some(queued_at) = screen.queued_at.take() {
            tracing::trace!(
                output = screen.output.name(),
                after = ?queued_at.elapsed(),
                "frame shown"
            );
        }
        screen.frames.finished()
    }
}

impl Screen {
    /// Returns the name clients and `outputs.toml` know the connector by,
    /// such as `HDMI-A-1`.
    fn connector_name(connector: &connector::Info) -> String {
        format!(
            "{}-{}",
            connector.interface().as_str(),
            connector.interface_id()
        )
    }

    /// Returns the connector's modes as the planner reads them.
    fn modes_of(connector: &connector::Info) -> Vec<ConnectorMode> {
        connector
            .modes()
            .iter()
            .map(|mode| {
                let (width, height) = mode.size();
                ConnectorMode {
                    width: i32::from(width),
                    height: i32::from(height),
                    refresh_millihertz: OutputModeInfo::from(*mode).refresh,
                    is_preferred: mode.mode_type().contains(ModeTypeFlags::PREFERRED),
                }
            })
            .collect()
    }

    /// Notes the monitor's preferred mode anew (its EDID arrived late).
    fn set_preferred_mode(&self) {
        let modes = Self::modes_of(&self.connector);
        if let Some((index, _)) = plan::choose_mode(&modes, None) {
            self.output
                .set_preferred(OutputModeInfo::from(self.connector.modes()[index]));
        }
    }

    /// Returns the frame interval at the output's refresh rate.
    pub(super) fn frame_interval(&self) -> Duration {
        presentation::frame_interval(&self.output)
    }

    /// Turns the output on or off: off, its monitor sleeps and clients no
    /// longer see it.
    pub(super) fn set_enabled(&mut self, is_enabled: bool, display: &DisplayHandle) {
        match (is_enabled, self.global.take()) {
            (true, None) => {
                self.global = Some(self.output.create_global::<Compositor>(display));
                self.frames.reset();
                let _ = self.frames.request();
            }
            (false, Some(global)) => {
                display.remove_global::<Compositor>(global);
                if let Err(error) = self.drm_output.with_compositor(DrmCompositor::clear) {
                    tracing::warn!(%error, "could not turn the monitor off");
                }
                self.frames.reset();
            }
            (true, Some(global)) => self.global = Some(global),
            (false, None) => {}
        }
    }

    /// Wakes the monitor, or puts it to sleep (`is_on` false); the output
    /// stays on for clients either way.
    pub(super) fn set_powered(&mut self, is_on: bool) {
        if self.is_powered == is_on {
            return;
        }
        self.is_powered = is_on;
        self.frames.reset();
        if is_on {
            // The whole frame is drawn anew, so the monitor wakes to it.
            self.drm_output
                .with_compositor(DrmCompositor::reset_buffers);
            let _ = self.frames.request();
        } else if let Err(error) = self.drm_output.with_compositor(DrmCompositor::clear) {
            tracing::warn!(%error, "could not put the monitor to sleep");
        }
    }

    /// Sets the mode nearest `wanted` (the preferred one without it).
    pub(super) fn set_mode(&mut self, wanted: Option<OutputMode>, renderer: &mut DrmRenderer<'_>) {
        let modes = Self::modes_of(&self.connector);
        let Some((index, fit)) = plan::choose_mode(&modes, wanted) else {
            return;
        };
        let name = self.output.name();
        match fit {
            ModeFit::Unavailable => tracing::warn!(
                output = name,
                wanted = ?wanted,
                "the monitor has no mode of that size; its preferred one is used"
            ),
            ModeFit::NearestRefresh => tracing::info!(
                output = name,
                wanted = ?wanted,
                "the monitor lacks that refresh rate; the nearest is used"
            ),
            ModeFit::Preferred | ModeFit::Exact => {}
        }
        let mode: Mode = self.connector.modes()[index];
        let output_mode = OutputModeInfo::from(mode);
        if self.output.current_mode() == Some(output_mode) {
            return;
        }
        match self
            .drm_output
            .use_mode::<_, OutputElement<DrmRenderer<'_>>>(
                mode,
                renderer,
                &DrmOutputRenderElements::default(),
            ) {
            Ok(()) => {
                self.output
                    .change_current_state(Some(output_mode), None, None, None);
                tracing::info!(output = name, mode = ?output_mode, "mode set");
            }
            Err(error) => tracing::warn!(?error, output = name, "could not set the mode"),
        }
    }

    /// Turns variable refresh on or off as `vrr` says, with `top` the
    /// window on top on it.
    pub(super) fn set_vrr(&mut self, vrr: Vrr, top: TopWindow) {
        let handle = self.connector.handle();
        let support = self
            .drm_output
            .with_compositor(|compositor| compositor.vrr_supported(handle))
            .unwrap_or(VrrSupport::NotSupported);
        // Turning it on and off without a modeset is what following the
        // fullscreen window needs; with a modeset, only "always" does.
        let is_supported = match support {
            VrrSupport::Supported => true,
            VrrSupport::RequiresModeset => vrr == Vrr::On,
            VrrSupport::NotSupported => false,
        };
        let wanted = plan::wants_vrr(vrr, is_supported, top);
        let is_enabled = self
            .drm_output
            .with_compositor(|compositor| compositor.vrr_enabled());
        if wanted != is_enabled
            && let Err(error) = self
                .drm_output
                .with_compositor(|compositor| compositor.use_vrr(wanted))
        {
            tracing::warn!(
                ?error,
                output = self.output.name(),
                "could not change variable refresh"
            );
        }
    }

    /// Draws `scene` and sends the frame to the monitor if it changed
    /// anything; returns whether it did.
    fn render(
        &mut self,
        renderer: &mut DrmRenderer<'_>,
        scene: &Scene<'_>,
    ) -> Result<bool, RenderError> {
        let started = Instant::now();
        let elements = scene.output_elements(
            renderer,
            &self.output,
            ImageDescription::of_output(&self.output),
        );
        let gathered = started.elapsed();
        let result = self
            .drm_output
            .render_frame(
                renderer,
                &elements,
                scene::BACKGROUND_COLOR,
                FrameFlags::DEFAULT,
            )
            .map_err(|error| RenderError::Hardware(error.to_string()))?;
        let is_changed = !result.is_empty;
        let drawn = started.elapsed();
        if let Some(dmabuf_feedback) = &self.dmabuf_feedback {
            scene.send_dmabuf_feedback(&self.output, dmabuf_feedback, &result.states);
        }
        if is_changed {
            let feedback = scene.presentation_feedback(&self.output, &result.states);
            self.drm_output
                .queue_frame(feedback)
                .map_err(|error| RenderError::Hardware(error.to_string()))?;
            self.queued_at = Some(Instant::now());
        }
        tracing::trace!(
            output = self.output.name(),
            elements = elements.len(),
            ?gathered,
            ?drawn,
            total = ?started.elapsed(),
            is_changed,
            "frame composed"
        );
        self.frames.drawn(is_changed);
        Ok(is_changed)
    }
}
