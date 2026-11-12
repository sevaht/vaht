//! Running without a screen: frames are drawn in memory by the software
//! renderer, paced like a 60 Hz display.
//!
//! Clients see an ordinary output and receive frame callbacks, so they behave
//! as on a real display. Tests use this to run vaht anywhere, with no GPU and
//! no host compositor.

use std::fmt;
use std::time::Duration;

use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::{Bind as _, Renderer as _};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::pixman::{FormatCode, Image};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::{Logical, Physical, Rectangle, Size, Transform};

use super::{BackendError, RenderError};
use crate::Compositor;
use crate::event_source::Timer;
use crate::resources::ResourceLedger;
use crate::scene;
use crate::windows::Windows;
use vaht_ui::theme::Metrics;

/// An output that exists only in memory.
pub(crate) struct HeadlessBackend {
    /// The software renderer.
    renderer: PixmanRenderer,
    /// The image frames are drawn into.
    target: Image<'static, 'static>,
    /// The output clients see.
    output: Output,
    /// Knows what changed since earlier frames, to redraw only that.
    damage_tracker: OutputDamageTracker,
    /// Registers the frame timer.
    loop_handle: LoopHandle<'static, Compositor>,
    /// Counts the frame timer.
    ledger: ResourceLedger,
    /// The next frame, when one is scheduled or was just drawn.
    frame_timer: Option<Timer<Compositor>>,
}

impl HeadlessBackend {
    /// The output's name, as clients see it.
    const OUTPUT_NAME: &str = "headless";

    /// The output's width and height: a common desktop resolution, so
    /// clients size themselves as they would on a real display.
    const WIDTH: u16 = 1920;
    /// See [`Self::WIDTH`].
    const HEIGHT: u16 = 1080;

    /// The refresh rate reported, in millihertz.
    const REFRESH_MILLIHERTZ: i32 = 60_000;

    /// The time between frames at that rate.
    const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

    /// Creates the in-memory output and advertises it.
    ///
    /// # Errors
    ///
    /// Returns an error if the software renderer or its image cannot be
    /// created.
    pub(crate) fn new(
        handle: &LoopHandle<'static, Compositor>,
        display: &DisplayHandle,
        ledger: &ResourceLedger,
    ) -> Result<Self, BackendError> {
        let renderer = PixmanRenderer::new().map_err(BackendError::SoftwareRenderer)?;
        let target = Image::new(
            FormatCode::A8R8G8B8,
            usize::from(Self::WIDTH),
            usize::from(Self::HEIGHT),
            false,
        )
        .map_err(BackendError::FrameImage)?;
        let output = Output::new(
            Self::OUTPUT_NAME.to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "vaht".to_owned(),
                model: "headless".to_owned(),
                serial_number: String::new(),
            },
        );
        // The global lives as long as the display; nothing removes it.
        let _global = output.create_global::<Compositor>(display);
        let mode = Mode {
            size: Self::size(),
            refresh: Self::REFRESH_MILLIHERTZ,
        };
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            None,
            Some((0, 0).into()),
        );
        output.set_preferred(mode);
        let damage_tracker = OutputDamageTracker::from_output(&output);
        Ok(Self {
            renderer,
            target,
            output,
            damage_tracker,
            loop_handle: handle.clone(),
            ledger: ledger.clone(),
            frame_timer: None,
        })
    }

    /// Returns the output clients see.
    pub(crate) const fn output(&self) -> &Output {
        &self.output
    }

    /// Returns the output's area in global coordinates.
    pub(crate) fn output_geometry(&self) -> Rectangle<i32, Logical> {
        let size = self
            .output
            .current_mode()
            .map_or_else(Self::size, |mode| mode.size);
        Rectangle::new((0, 0).into(), size.to_logical(1))
    }

    /// Returns the output's size in pixels.
    fn size() -> Size<i32, Physical> {
        Size::from((i32::from(Self::WIDTH), i32::from(Self::HEIGHT)))
    }

    /// Schedules the next frame one refresh interval from now, unless one is
    /// already scheduled.
    pub(crate) fn schedule_render(&mut self) {
        if self
            .frame_timer
            .as_ref()
            .is_some_and(|timer| !timer.has_fired())
        {
            return;
        }
        match Timer::once(
            &self.loop_handle,
            &self.ledger,
            Self::FRAME_INTERVAL,
            Compositor::render,
        ) {
            Ok(timer) => self.frame_timer = Some(timer),
            Err(error) => {
                tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    "could not schedule a frame"
                );
            }
        }
    }

    /// Frees textures cached for client buffers that no longer exist.
    pub(crate) fn release_unused_textures(&mut self) {
        if let Err(error) = self.renderer.cleanup_texture_cache() {
            tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not free unused textures"
            );
        }
    }

    /// Draws `windows` into the in-memory image.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame could not be drawn.
    pub(crate) fn render(
        &mut self,
        windows: &Windows,
        metrics: Metrics,
    ) -> Result<(), RenderError> {
        let mut framebuffer = self
            .renderer
            .bind(&mut self.target)
            .map_err(RenderError::BindSoftware)?;
        let scale = self.output.current_scale().fractional_scale().into();
        let elements = scene::compose(&mut self.renderer, windows, metrics, scale);
        self.damage_tracker
            .render_output(
                &mut self.renderer,
                &mut framebuffer,
                0,
                &elements,
                scene::BACKGROUND_COLOR,
            )
            .map_err(RenderError::DrawSoftware)?;
        Ok(())
    }
}

impl fmt::Debug for HeadlessBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HeadlessBackend")
            .field("output", &self.output.name())
            .finish_non_exhaustive()
    }
}
