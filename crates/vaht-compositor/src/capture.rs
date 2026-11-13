//! Screenshots and screen recording: `ext-image-copy-capture-v1`.
//!
//! A client (grim, the screen-sharing portal, OBS, vaht's own tests) asks
//! for an output's image and hands vaht a shared-memory buffer. vaht
//! composes the current scene with a software renderer of its own into an
//! image it owns, reads the pixels back, and copies them into the client's
//! buffer ([`crate::shared_memory`]). The same path works for every backend,
//! and a capture never disturbs what the backend shows. A capture is rare
//! next to frames, so drawing it again in software costs nothing in normal
//! use.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::pixman::{PixmanError, PixmanRenderer};
use smithay::backend::renderer::{Bind as _, ExportMem as _, Renderer as _};
use smithay::output::WeakOutput;
use smithay::reexports::pixman::{FormatCode, Image};
use smithay::reexports::wayland_server::protocol::wl_shm::Format;
use smithay::utils::{Physical, Rectangle, Scale, Size, Transform};
use smithay::wayland::image_capture_source::{
    ImageCaptureSource, ImageCaptureSourceHandler, OutputCaptureSourceHandler,
    OutputCaptureSourceState,
};
use smithay::wayland::image_copy_capture::{
    BufferConstraints, CaptureFailureReason, Frame, ImageCopyCaptureHandler, ImageCopyCaptureState,
    Session, SessionRef,
};

use crate::Compositor;
use crate::resources::{ResourceKind, Tracked};
use crate::scene::{self, Scene};
use crate::shared_memory::{self, SharedMemoryError};

/// Draws captures; separate from the backend's renderer, so captures look
/// the same on every backend.
pub(crate) struct ScreenCapture {
    /// The software renderer captures are drawn with.
    renderer: PixmanRenderer,
    /// Open capture sessions. Smithay stops a session when its `Session`
    /// is dropped, so each is kept until its client destroys it.
    sessions: Vec<(Session, Tracked)>,
}

impl ScreenCapture {
    /// Creates the capture renderer.
    ///
    /// # Errors
    ///
    /// Returns an error if the software renderer cannot be created.
    pub(crate) fn new() -> Result<Self, PixmanError> {
        Ok(Self {
            renderer: PixmanRenderer::new()?,
            sessions: Vec::new(),
        })
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

    /// Returns `scene` as packed 32-bit BGRA rows (ARGB8888
    /// in memory) of `size`.
    ///
    /// # Errors
    ///
    /// Returns an error if the scene could not be drawn or read back.
    fn render(
        &mut self,
        scene: &Scene<'_>,
        size: Size<i32, Physical>,
        scale: Scale<f64>,
    ) -> Result<Vec<u8>, CaptureError> {
        let width = usize::try_from(size.w).map_err(|_negative| CaptureError::EmptyOutput)?;
        let height = usize::try_from(size.h).map_err(|_negative| CaptureError::EmptyOutput)?;
        let mut image =
            Image::new(FormatCode::A8R8G8B8, width, height, false).map_err(CaptureError::Image)?;
        let framebuffer = self
            .renderer
            .bind(&mut image)
            .map_err(CaptureError::Renderer)?;
        let mut framebuffer = framebuffer;
        let elements = scene.elements(&mut self.renderer, scale);
        OutputDamageTracker::new(size, scale, Transform::Normal)
            .render_output(
                &mut self.renderer,
                &mut framebuffer,
                0,
                &elements,
                scene::BACKGROUND_COLOR,
            )
            .map_err(CaptureError::Draw)?;
        let region = Rectangle::from_size((size.w, size.h).into());
        let mapping = self
            .renderer
            .copy_framebuffer(&framebuffer, region, Fourcc::Argb8888)
            .map_err(CaptureError::Renderer)?;
        let pixels = self
            .renderer
            .map_texture(&mapping)
            .map_err(CaptureError::Renderer)?;
        Ok(pixels.to_vec())
    }
}

impl std::fmt::Debug for ScreenCapture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScreenCapture")
            .field("sessions", &self.sessions.len())
            .finish_non_exhaustive()
    }
}

impl Compositor {
    /// Fills `frame` with the output's current image, or fails it.
    fn capture_frame(&mut self, frame: Frame) {
        self.prepare_panels();
        let output = self.backend.output();
        let Some(mode) = output.current_mode() else {
            frame.fail(CaptureFailureReason::Stopped);
            return;
        };
        let scale = output.current_scale().fractional_scale().into();
        let metrics = self.theme.metrics();
        let size = mode.size;
        let result = self
            .capture
            .render(&Scene::new(&self.windows, metrics), size, scale)
            .and_then(|pixels| {
                let (width, height) = (
                    usize::try_from(size.w).map_err(|_negative| CaptureError::EmptyOutput)?,
                    usize::try_from(size.h).map_err(|_negative| CaptureError::EmptyOutput)?,
                );
                shared_memory::write_frame(&frame.buffer(), &pixels, width, height)
                    .map_err(CaptureError::Write)
            });
        match result {
            Ok(()) => frame.success(Transform::Normal, None, self.start_time.elapsed()),
            Err(error) => {
                tracing::warn!(error = &error as &dyn std::error::Error, "capture failed");
                let reason = match error {
                    CaptureError::Write(_) => CaptureFailureReason::BufferConstraints,
                    CaptureError::EmptyOutput
                    | CaptureError::Image(_)
                    | CaptureError::Renderer(_)
                    | CaptureError::Draw(_) => CaptureFailureReason::Unknown,
                };
                frame.fail(reason);
            }
        }
    }
}

impl ImageCaptureSourceHandler for Compositor {}

impl OutputCaptureSourceHandler for Compositor {
    fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
        &mut self.protocols.output_capture_source
    }

    fn output_source_created(
        &mut self,
        source: ImageCaptureSource,
        output: &smithay::output::Output,
    ) {
        source.user_data().insert_if_missing(|| output.downgrade());
    }
}

impl ImageCopyCaptureHandler for Compositor {
    fn image_copy_capture_state(&mut self) -> &mut ImageCopyCaptureState {
        &mut self.protocols.image_copy_capture
    }

    fn capture_constraints(&mut self, source: &ImageCaptureSource) -> Option<BufferConstraints> {
        let output = source.user_data().get::<WeakOutput>()?.upgrade()?;
        let mode = output.current_mode()?;
        Some(BufferConstraints {
            size: mode.size.to_logical(1).to_buffer(1, Transform::Normal),
            shm: vec![Format::Argb8888, Format::Xrgb8888],
        })
    }

    fn new_session(&mut self, session: Session) {
        let tracked = self.ledger.track(ResourceKind::CaptureSession);
        self.capture.sessions.push((session, tracked));
    }

    fn session_destroyed(&mut self, session: SessionRef) {
        self.capture.sessions.retain(|(kept, _)| *kept != session);
        self.client_cleanup_pending = true;
    }

    fn frame(&mut self, _session: &SessionRef, frame: Frame) {
        self.capture_frame(frame);
    }
}

/// Why a capture failed.
#[derive(Debug, thiserror::Error)]
enum CaptureError {
    /// The output has no size.
    #[error("the output has no size")]
    EmptyOutput,

    /// The image to draw into could not be created.
    #[error("could not create the capture image")]
    Image(#[source] smithay::reexports::pixman::CreateFailed),

    /// The software renderer failed to bind or read back.
    #[error("the capture renderer failed")]
    Renderer(#[source] PixmanError),

    /// Drawing the scene failed.
    #[error("could not draw the capture")]
    Draw(#[source] smithay::backend::renderer::damage::Error<PixmanError>),

    /// The client's buffer could not be written.
    #[error("could not write the client's buffer")]
    Write(#[source] SharedMemoryError),
}
