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
use std::time::Duration;

use smithay::backend::allocator::gbm::GbmAllocator;
use smithay::backend::drm::compositor::{DrmCompositor, FrameFlags};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{DrmDeviceFd, DrmNode, VrrSupport};
use smithay::output::{Mode as OutputModeInfo, Output, PhysicalProperties};
use smithay::reexports::drm::control::{Device as _, Mode, ModeTypeFlags, connector, crtc};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay_drm_extras::display_info;
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};

use super::DrmRenderer;
use super::plan::{self, ConnectorMode, FrameState, GpuQuirks, ModeFit, TopWindow};
use crate::Compositor;
use crate::backend::{OutputEvent, RenderError};
use crate::config::{OutputMode, Vrr};
use crate::event_source::{EventSource, Timer};
use crate::scene::{self, OutputElement, Scene};

/// How a GPU's outputs are driven.
pub(super) type GpuOutputManager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    (),
    DrmDeviceFd,
>;

/// One output's driver.
type GpuOutput =
    DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

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
    /// Its vertical blank events, registered while it lives.
    pub(super) _events: EventSource<Compositor>,
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
}

/// What a GPU device needs from the rest of the backend to change its
/// outputs: where globals go, and a renderer for the device.
pub(super) struct DeviceContext<'context, 'renderer> {
    /// Where the outputs' globals are advertised.
    pub(super) display: &'context DisplayHandle,
    /// A renderer for this device.
    pub(super) renderer: &'context mut DrmRenderer<'renderer>,
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
            tracing::info!(
                connector = name,
                "not a desktop monitor (a headset); left alone"
            );
            return None;
        }
        let modes = Screen::modes_of(&connector);
        let Some((index, _)) = plan::choose_mode(&modes, None) else {
            tracing::warn!(connector = name, "the monitor offers no mode");
            return None;
        };
        let mode = connector.modes()[index];
        let info = display_info::for_connector(self.manager.device(), connector.handle());
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
            },
        );
        Some(output)
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
            if screen.global.is_none() || !screen.frames.is_due() {
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

    /// Notes that `crtc`'s frame was shown; returns whether another is due.
    pub(super) fn frame_shown(&mut self, crtc: crtc::Handle) -> bool {
        let Some(screen) = self.screens.get_mut(&crtc) else {
            return false;
        };
        if let Err(error) = screen.drm_output.frame_submitted() {
            tracing::debug!(?error, "a frame was not shown");
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
        let millihertz = self
            .output
            .current_mode()
            .map_or(60_000, |mode| mode.refresh.max(1));
        Duration::from_micros(1_000_000_000 / u64::try_from(millihertz).unwrap_or(60_000))
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
        let elements = scene.output_elements(renderer, &self.output);
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
        if is_changed {
            self.drm_output
                .queue_frame(())
                .map_err(|error| RenderError::Hardware(error.to_string()))?;
        }
        self.frames.drawn(is_changed);
        Ok(is_changed)
    }
}
