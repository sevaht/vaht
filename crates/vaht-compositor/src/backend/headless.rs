//! Running without a screen: frames are drawn in memory by the software
//! renderer, paced like a 60 Hz display, on one output or as many as
//! [`HeadlessBackend::OUTPUTS_VARIABLE`] asks for.
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
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::{Physical, Size, Transform};

use super::{BackendError, RenderError};
use crate::Compositor;
use crate::config::OutputMode;
use crate::event_source::Timer;
use crate::outputs::{OutputControl, Outputs};
use crate::resources::ResourceLedger;
use crate::scene::{self, Scene};

/// Outputs that exist only in memory.
pub(crate) struct HeadlessBackend {
    /// The software renderer.
    renderer: PixmanRenderer,
    /// The first output, which is primary.
    primary: Screen,
    /// The rest.
    others: Vec<Screen>,
    /// Registers the frame timer.
    loop_handle: LoopHandle<'static, Compositor>,
    /// Counts the frame timer.
    ledger: ResourceLedger,
    /// The next frame, when one is scheduled or was just drawn.
    frame_timer: Option<Timer<Compositor>>,
    /// Where the outputs' globals are advertised.
    display: DisplayHandle,
}

/// One in-memory output.
struct Screen {
    /// The output clients see.
    output: Output,
    /// Its size unless `outputs.toml` sets a mode.
    preferred: Size<i32, Physical>,
    /// The image its frames are drawn into.
    target: Image<'static, 'static>,
    /// Knows what changed since its earlier frames, to redraw only that.
    damage_tracker: OutputDamageTracker,
    /// Its global while it is on: clients see it, and it is drawn.
    global: Option<GlobalId>,
}

impl HeadlessBackend {
    /// The environment variable that sets the outputs' sizes, such as
    /// `1920x1080,1280x1024` for two; one output of [`Self::DEFAULT_SIZE`]
    /// without it.
    pub(crate) const OUTPUTS_VARIABLE: &str = "VAHT_HEADLESS_OUTPUTS";

    /// An output's width and height unless asked otherwise: a common
    /// desktop resolution, so clients size themselves as they would on a
    /// real display.
    const DEFAULT_SIZE: (i32, i32) = (1920, 1080);

    /// The refresh rate reported, in millihertz.
    const REFRESH_MILLIHERTZ: i32 = 60_000;

    /// The time between frames at that rate.
    const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

    /// Creates the in-memory outputs and advertises them.
    ///
    /// # Errors
    ///
    /// Returns an error if the software renderer or an image cannot be
    /// created.
    pub(crate) fn new(
        handle: &LoopHandle<'static, Compositor>,
        display: &DisplayHandle,
        ledger: &ResourceLedger,
    ) -> Result<Self, BackendError> {
        let renderer = PixmanRenderer::new().map_err(BackendError::SoftwareRenderer)?;
        let asked = std::env::var(Self::OUTPUTS_VARIABLE).ok();
        let sizes = asked.as_deref().map_or_else(Vec::new, |text| {
            Self::sizes(text).unwrap_or_else(|| {
                tracing::warn!(
                    variable = Self::OUTPUTS_VARIABLE,
                    value = text,
                    "not a list of sizes such as 1920x1080,1280x1024; one output it is"
                );
                Vec::new()
            })
        });
        let (first, rest) = sizes
            .split_first()
            .map_or((Self::DEFAULT_SIZE.into(), &[][..]), |(first, rest)| {
                (*first, rest)
            });
        let primary = Screen::new(1, first, display)?;
        let others = rest
            .iter()
            .zip(2..)
            .map(|(size, number)| Screen::new(number, *size, display))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            renderer,
            primary,
            others,
            loop_handle: handle.clone(),
            ledger: ledger.clone(),
            frame_timer: None,
            display: display.clone(),
        })
    }

    /// Reads `WIDTHxHEIGHT` sizes separated by commas; none if one is not
    /// a size.
    fn sizes(text: &str) -> Option<Vec<Size<i32, Physical>>> {
        text.split(',')
            .map(|size| {
                let (width, height) = size.trim().split_once('x')?;
                let (width, height) = (width.parse().ok()?, height.parse().ok()?);
                (width > 0 && height > 0).then(|| Size::from((width, height)))
            })
            .collect()
    }

    /// Returns the outputs, laid out side by side.
    pub(crate) fn outputs(&self) -> Outputs {
        Outputs::in_a_row(
            self.primary.output.clone(),
            self.others
                .iter()
                .map(|screen| screen.output.clone())
                .collect(),
        )
    }

    /// Returns the renderer.
    pub(crate) const fn renderer(&mut self) -> &mut PixmanRenderer {
        &mut self.renderer
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

    /// Draws `scene` into each output's in-memory image.
    ///
    /// # Errors
    ///
    /// Returns an error if a frame could not be drawn; the outputs after it
    /// are still drawn.
    pub(crate) fn render(&mut self, scene: &Scene<'_>) -> Result<(), RenderError> {
        let mut result = Ok(());
        for screen in std::iter::once(&mut self.primary).chain(&mut self.others) {
            if screen.global.is_some() {
                let drawn = screen.render(&mut self.renderer, scene);
                result = result.and(drawn);
            }
        }
        result
    }

    /// Returns every output.
    fn screens_mut(&mut self) -> impl Iterator<Item = &mut Screen> {
        std::iter::once(&mut self.primary).chain(&mut self.others)
    }

    /// Returns the output `output` is.
    fn screen_mut(&mut self, output: &Output) -> Option<&mut Screen> {
        self.screens_mut().find(|screen| screen.output == *output)
    }
}

impl OutputControl for HeadlessBackend {
    fn set_enabled(&mut self, output: &Output, is_enabled: bool) {
        let display = self.display.clone();
        if let Some(screen) = self.screen_mut(output) {
            match (is_enabled, screen.global.take()) {
                (true, None) => {
                    screen.global = Some(output.create_global::<Compositor>(&display));
                }
                (false, Some(global)) => display.remove_global::<Compositor>(global),
                (true, Some(global)) => screen.global = Some(global),
                (false, None) => {}
            }
        }
    }

    fn set_transform(&mut self, output: &Output, transform: Transform) {
        output.change_current_state(None, Some(transform), None, None);
    }

    fn set_mode(&mut self, output: &Output, mode: Option<OutputMode>) {
        if let Some(screen) = self.screen_mut(output) {
            let size = mode.map_or(screen.preferred, |mode| {
                Size::from((mode.width, mode.height))
            });
            let refresh = mode
                .and_then(|mode| mode.refresh_millihertz)
                .unwrap_or(Self::REFRESH_MILLIHERTZ);
            if let Err(error) = screen.resize(size, refresh) {
                tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    output = output.name(),
                    "the output keeps its mode"
                );
            }
        }
    }
}

impl Screen {
    /// Creates output `number` (counting from 1) of `size` and advertises
    /// it on `display`.
    ///
    /// # Errors
    ///
    /// Returns an error if its image cannot be created.
    fn new(
        number: usize,
        size: Size<i32, Physical>,
        display: &DisplayHandle,
    ) -> Result<Self, BackendError> {
        let target = Self::image(size)?;
        let output = Output::new(
            format!("HEADLESS-{number}"),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "vaht".to_owned(),
                model: "headless".to_owned(),
                serial_number: String::new(),
            },
        );
        let global = output.create_global::<Compositor>(display);
        let mode = Mode {
            size,
            refresh: HeadlessBackend::REFRESH_MILLIHERTZ,
        };
        output.change_current_state(Some(mode), Some(Transform::Normal), None, None);
        output.set_preferred(mode);
        let damage_tracker = OutputDamageTracker::from_output(&output);
        Ok(Self {
            output,
            preferred: size,
            target,
            damage_tracker,
            global: Some(global),
        })
    }

    /// Returns an image of `size` to draw into.
    ///
    /// # Errors
    ///
    /// Returns an error if it cannot be created.
    fn image(size: Size<i32, Physical>) -> Result<Image<'static, 'static>, BackendError> {
        let dimension = |value: i32| usize::try_from(value).unwrap_or_default();
        Image::new(
            FormatCode::A8R8G8B8,
            dimension(size.w),
            dimension(size.h),
            false,
        )
        .map_err(BackendError::FrameImage)
    }

    /// Makes the output `size` pixels at `refresh` millihertz, with an
    /// image to match, unless it is already.
    ///
    /// # Errors
    ///
    /// Returns an error if the new image cannot be created.
    fn resize(&mut self, size: Size<i32, Physical>, refresh: i32) -> Result<(), BackendError> {
        let mode = Mode { size, refresh };
        if self.output.current_mode() == Some(mode) {
            return Ok(());
        }
        self.target = Self::image(size)?;
        self.output.add_mode(mode);
        self.output
            .change_current_state(Some(mode), None, None, None);
        Ok(())
    }

    /// Draws what `scene` shows on this output into its image.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame could not be drawn.
    fn render(
        &mut self,
        renderer: &mut PixmanRenderer,
        scene: &Scene<'_>,
    ) -> Result<(), RenderError> {
        let mut framebuffer = renderer
            .bind(&mut self.target)
            .map_err(RenderError::BindSoftware)?;
        let elements = scene.output_elements(renderer, &self.output);
        self.damage_tracker
            .render_output(
                renderer,
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
            .field("primary", &self.primary.output.name())
            .field("others", &self.others.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_read_from_a_list() {
        assert_eq!(
            HeadlessBackend::sizes("1920x1080, 1280x1024"),
            Some(vec![Size::from((1920, 1080)), Size::from((1280, 1024))])
        );
        assert_eq!(HeadlessBackend::sizes("1920x0"), None);
        assert_eq!(HeadlessBackend::sizes("wide"), None);
    }
}
