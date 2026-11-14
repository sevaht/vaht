//! Screenshots and screen recording: `ext-image-copy-capture-v1`.
//!
//! A client (grim, the screen-sharing portal, OBS, vaht's own tests) asks
//! for an output's image and hands vaht a shared-memory buffer. The
//! backend draws the scene again with its own renderer into an image of
//! its own ([`draw_capture`]), reads the pixels back, and vaht copies them
//! into the client's buffer ([`crate::shared_memory`]). Drawing with the
//! backend's renderer is what lets windows drawn on the GPU show; every
//! backend runs the same function, so the software one tests it. A capture
//! never disturbs what the outputs show. A turned output is drawn as its
//! monitor shows it, at its mode's size, and the frame says the transform,
//! which grim and the portal undo.

use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::{Bind, ExportMem, ImportAll, ImportMem, Offscreen, Renderer};
use smithay::output::{Output, WeakOutput};
use smithay::reexports::wayland_server::protocol::wl_shm::Format;
use smithay::utils::{Rectangle, Size, Transform};
use smithay::wayland::image_capture_source::{
    ImageCaptureSource, ImageCaptureSourceHandler, OutputCaptureSourceHandler,
    OutputCaptureSourceState,
};
use smithay::wayland::image_copy_capture::{
    BufferConstraints, CaptureFailureReason, Frame, ImageCopyCaptureHandler, ImageCopyCaptureState,
    Session, SessionRef,
};

use crate::Compositor;
use crate::layers::LayerMaps;
use crate::resources::{ResourceKind, Tracked};
use crate::scene::{self, Drawn, Scene};
use crate::shared_memory::{self, SharedMemoryError};

/// The open capture sessions.
#[derive(Default)]
pub(crate) struct ScreenCapture {
    /// Open capture sessions. Smithay stops a session when its `Session`
    /// is dropped, so each is kept until its client destroys it.
    sessions: Vec<(Session, Tracked)>,
}

/// What a capture drew: packed 32-bit rows, ARGB8888 in memory (blue,
/// green, red, alpha), of an image `width` by `height` pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedImage {
    /// The pixels.
    pub(crate) pixels: Vec<u8>,
    /// Its width, in pixels.
    pub(crate) width: usize,
    /// Its height, in pixels.
    pub(crate) height: usize,
}

/// Draws what `scene` shows on `output` with `renderer` into an image of
/// the renderer's own, and reads it back.
///
/// # Errors
///
/// Returns an error if the output has no mode, or the scene could not be
/// drawn or read back.
pub(crate) fn draw_capture<R, Target>(
    renderer: &mut R,
    scene: &Scene<'_>,
    output: &Output,
) -> Result<CapturedImage, CaptureError>
where
    R: Renderer + ImportAll + ImportMem + Offscreen<Target> + Bind<Target> + ExportMem,
    R::TextureId: Send + Clone + 'static,
    R::Error: Send + Sync + 'static,
{
    let size = output
        .current_mode()
        .map(|mode| mode.size)
        .ok_or(CaptureError::EmptyOutput)?;
    let width = usize::try_from(size.w).map_err(|_negative| CaptureError::EmptyOutput)?;
    let height = usize::try_from(size.h).map_err(|_negative| CaptureError::EmptyOutput)?;
    let scale: smithay::utils::Scale<f64> = output.current_scale().fractional_scale().into();
    let buffer_size: Size<i32, smithay::utils::Buffer> = (size.w, size.h).into();
    let mut target = renderer
        .create_buffer(Fourcc::Argb8888, buffer_size)
        .map_err(CaptureError::renderer)?;
    let elements = scene.output_elements(renderer, output);
    let mut framebuffer = renderer.bind(&mut target).map_err(CaptureError::renderer)?;
    OutputDamageTracker::new(size, scale, output.current_transform())
        .render_output(
            renderer,
            &mut framebuffer,
            0,
            &elements,
            scene::BACKGROUND_COLOR,
        )
        .map_err(|error| CaptureError::Draw(error.to_string()))?;
    let region = Rectangle::from_size((size.w, size.h).into());
    let mapping = renderer
        .copy_framebuffer(&framebuffer, region, Fourcc::Argb8888)
        .map_err(CaptureError::renderer)?;
    let pixels = renderer
        .map_texture(&mapping)
        .map_err(CaptureError::renderer)?
        .to_vec();
    Ok(CapturedImage {
        pixels,
        width,
        height,
    })
}

impl ScreenCapture {
    /// Fills `frame` with `image`, of an output turned as `transform`
    /// says, made `time` after vaht started, or fails it for why the image
    /// could not be made.
    fn fill(
        frame: Frame,
        image: Result<CapturedImage, CaptureError>,
        transform: Transform,
        time: Duration,
    ) {
        let result = image.and_then(|image| {
            shared_memory::write_frame(&frame.buffer(), &image.pixels, image.width, image.height)
                .map_err(CaptureError::Write)
        });
        match result {
            Ok(()) => frame.success(transform, None, time),
            Err(error) => {
                tracing::warn!(error = &error as &dyn std::error::Error, "capture failed");
                let reason = match error {
                    CaptureError::Write(_) => CaptureFailureReason::BufferConstraints,
                    CaptureError::EmptyOutput
                    | CaptureError::Renderer(_)
                    | CaptureError::Draw(_) => CaptureFailureReason::Unknown,
                };
                frame.fail(reason);
            }
        }
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

impl ImageCaptureSourceHandler for Compositor {}

impl OutputCaptureSourceHandler for Compositor {
    fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
        &mut self.protocols.output_capture_source
    }

    fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
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
            // Captures are copied into shared memory; GPU buffers are not
            // offered yet.
            dma: None,
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

    fn frame(&mut self, session: &SessionRef, frame: Frame) {
        let Some(output) = session
            .source()
            .user_data()
            .get::<WeakOutput>()
            .and_then(WeakOutput::upgrade)
        else {
            frame.fail(CaptureFailureReason::Stopped);
            return;
        };
        self.prepare_frames();
        let scene = Scene::new(
            &self.window_manager,
            &self.toplevels,
            Drawn {
                frames: &self.frame_textures,
                taskbars: &self.taskbars,
                menus: &self.menus,
                backdrop: &self.backdrop,
                outline: &self.outline,
                tooltip: &self.tooltip,
                snap: &self.snap_overlay,
                xwayland: self.xwayland.as_ref(),
                lock: &self.session_lock,
            },
            LayerMaps::new(&self.outputs),
        );
        let pointer = self.pointer_location();
        let scene = if session.draw_cursor() {
            scene.with_pointer(&self.cursor, pointer)
        } else {
            scene
        };
        let image = self.backend.capture(&scene, &output);
        ScreenCapture::fill(
            frame,
            image,
            output.current_transform(),
            self.start_time.elapsed(),
        );
    }
}

/// Why a capture failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CaptureError {
    /// The output has no size.
    #[error("the output has no size")]
    EmptyOutput,

    /// The renderer could not make, bind or read back the capture image.
    #[error("the renderer failed")]
    Renderer(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Drawing the scene failed.
    #[error("could not draw the capture: {0}")]
    Draw(String),

    /// The client's buffer could not be written.
    #[error("could not write the client's buffer")]
    Write(#[source] SharedMemoryError),
}

impl CaptureError {
    /// A renderer's `error`.
    fn renderer(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Renderer(Box::new(error))
    }
}
