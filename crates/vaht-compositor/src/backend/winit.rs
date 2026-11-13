//! Running nested: frames go to a window of a host compositor, and input
//! comes from it.

use std::fmt;

use smithay::backend::allocator::Format;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{ImportDma as _, Renderer as _};
use smithay::backend::winit::{self as host_window, WinitEvent, WinitGraphicsBackend};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::{Logical, Physical, Rectangle, Size, Transform};

use super::{BackendError, RenderError};
use crate::Compositor;
use crate::event_source::EventSource;
use crate::resources::ResourceLedger;
use crate::scene::{self, Scene};

/// A window in a host compositor, shown to vaht's clients as one output.
pub(crate) struct WinitBackend {
    /// The host window and its renderer.
    graphics: WinitGraphicsBackend<GlesRenderer>,
    /// The output clients see.
    output: Output,
    /// Knows what changed since earlier frames, to redraw only that.
    damage_tracker: OutputDamageTracker,
    /// The host window's events, registered for as long as the backend lives.
    _events: EventSource<Compositor>,
}

impl WinitBackend {
    /// The output's name, as clients see it.
    const OUTPUT_NAME: &str = "nested";

    /// The refresh rate reported for the window. The host decides the real
    /// rate; clients only use this as a hint.
    const REFRESH_MILLIHERTZ: i32 = 60_000;

    /// Opens a window in the host compositor and advertises it as an output.
    ///
    /// # Errors
    ///
    /// Returns an error if the window cannot be opened or its events cannot
    /// be registered.
    pub(crate) fn new(
        handle: &LoopHandle<'static, Compositor>,
        display: &DisplayHandle,
        ledger: &ResourceLedger,
    ) -> Result<Self, BackendError> {
        let (graphics, events) = host_window::init::<GlesRenderer>()
            .map_err(|error| BackendError::open_window(&error))?;
        let output = Output::new(
            Self::OUTPUT_NAME.to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "vaht".to_owned(),
                model: "nested".to_owned(),
                serial_number: String::new(),
            },
        );
        // The global lives as long as the display; nothing removes it.
        let _global = output.create_global::<Compositor>(display);
        let mode = Self::mode_for(graphics.window_size());
        // OpenGL's framebuffer is upside down relative to the window.
        output.change_current_state(
            Some(mode),
            Some(Transform::Flipped180),
            None,
            Some((0, 0).into()),
        );
        output.set_preferred(mode);
        let damage_tracker = OutputDamageTracker::from_output(&output);
        let events = EventSource::insert(handle, ledger, events, |event, (), compositor| {
            match event {
                WinitEvent::Resized { size, .. } => {
                    if let super::Backend::Winit(backend) = &compositor.backend {
                        backend.resize(size);
                    }
                    compositor.schedule_render();
                }
                WinitEvent::Input(input) => compositor.handle_input(input),
                WinitEvent::Redraw => compositor.render(),
                WinitEvent::CloseRequested => compositor.stop(),
                // Keyboard focus moving to or from the host window changes
                // nothing inside vaht.
                WinitEvent::Focus(_) => {}
            }
        })
        .map_err(|error| BackendError::RegisterEvents(error.error))?;
        Ok(Self {
            graphics,
            output,
            damage_tracker,
            _events: events,
        })
    }

    /// Returns the output clients see.
    pub(crate) const fn output(&self) -> &Output {
        &self.output
    }

    /// Returns the output's area in global coordinates.
    pub(crate) fn output_geometry(&self) -> Rectangle<i32, Logical> {
        Rectangle::new((0, 0).into(), self.graphics.window_size().to_logical(1))
    }

    /// Asks the host for a redraw, which arrives as a [`WinitEvent::Redraw`].
    pub(crate) fn schedule_render(&self) {
        self.graphics.window().request_redraw();
    }

    /// Draws `scene` into the host window, redrawing only what changed.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame could not be drawn or shown.
    pub(crate) fn render(&mut self, scene: &Scene<'_>) -> Result<(), RenderError> {
        let age = self.graphics.buffer_age().unwrap_or(0);
        let damage = {
            let (renderer, mut framebuffer) = self.graphics.bind().map_err(RenderError::Bind)?;
            let elements = scene.elements(
                renderer,
                self.output.current_scale().fractional_scale().into(),
            );
            self.damage_tracker
                .render_output(
                    renderer,
                    &mut framebuffer,
                    age,
                    &elements,
                    scene::BACKGROUND_COLOR,
                )
                .map_err(RenderError::Draw)?
                .damage
                .cloned()
        };
        // No damage means nothing changed: keep showing the last frame.
        if let Some(damage) = damage {
            self.graphics
                .submit(Some(&damage))
                .map_err(RenderError::Submit)?;
        }
        Ok(())
    }

    /// Returns the GPU buffer formats the renderer can import.
    pub(crate) fn dmabuf_formats(&mut self) -> Vec<Format> {
        self.graphics
            .renderer()
            .dmabuf_formats()
            .into_iter()
            .collect()
    }

    /// Imports a client's GPU buffer; returns whether the renderer can use
    /// it.
    pub(crate) fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        match self.graphics.renderer().import_dmabuf(dmabuf, None) {
            Ok(_) => true,
            Err(error) => {
                tracing::debug!(%error, "a client's GPU buffer could not be imported");
                false
            }
        }
    }

    /// Returns the renderer.
    pub(crate) fn renderer(&mut self) -> &mut GlesRenderer {
        self.graphics.renderer()
    }

    /// Frees textures cached for client buffers that no longer exist.
    pub(crate) fn release_unused_textures(&mut self) {
        if let Err(error) = self.graphics.renderer().cleanup_texture_cache() {
            tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not free unused textures"
            );
        }
    }

    /// Matches the output to the host window's new size.
    fn resize(&self, size: Size<i32, Physical>) {
        self.output
            .change_current_state(Some(Self::mode_for(size)), None, None, None);
    }

    /// Returns the output mode for a window of `size`.
    fn mode_for(size: Size<i32, Physical>) -> Mode {
        Mode {
            size,
            refresh: Self::REFRESH_MILLIHERTZ,
        }
    }
}

impl fmt::Debug for WinitBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WinitBackend")
            .field("output", &self.output.name())
            .finish_non_exhaustive()
    }
}
