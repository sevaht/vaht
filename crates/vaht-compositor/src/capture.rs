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
use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{Bind, ExportMem, ImportAll, ImportMem, Offscreen, Renderer};
use smithay::output::{Output, WeakOutput};
use smithay::reexports::wayland_server::protocol::wl_shm::Format;
use smithay::utils::{Physical, Rectangle, Size, Transform};
use smithay::wayland::foreign_toplevel_list::ForeignToplevelHandle;
use smithay::wayland::image_capture_source::{
    ImageCaptureSource, ImageCaptureSourceHandler, OutputCaptureSourceHandler,
    OutputCaptureSourceState, ToplevelCaptureSourceHandler, ToplevelCaptureSourceState,
};
use smithay::wayland::image_copy_capture::{
    BufferConstraints, CaptureFailureReason, Frame, ImageCopyCaptureHandler, ImageCopyCaptureState,
    Session, SessionRef,
};

use crate::Compositor;
use crate::layers::LayerMaps;
use crate::resources::{ResourceKind, Tracked};
use crate::scene::{self, Drawn, OutputElement, Scene};
use crate::shared_memory::{self, SharedMemoryError};
use crate::window_manager::WindowId;

/// The open capture sessions.
#[derive(Default)]
pub(crate) struct ScreenCapture {
    /// Open capture sessions. Smithay stops a session when its `Session`
    /// is dropped, so each is kept until its client destroys it.
    sessions: Vec<(Session, Tracked)>,
}

/// What a capture source is for: an output, or one window.
#[derive(Debug, Clone)]
pub(crate) enum CaptureTarget {
    /// An output, while it lasts.
    Output(WeakOutput),
    /// A window's content.
    Window(WindowId),
}

/// What one capture draws: an output as it shows, or a window's content
/// alone, `size` pixels at `scale`.
#[derive(Debug, Clone)]
pub(crate) enum CaptureView {
    /// An output, at its mode's size, turned as it is.
    Output(Output),
    /// A window's content.
    Window {
        /// The window.
        window: WindowId,
        /// How large the image is.
        size: Size<i32, Physical>,
        /// How much larger than its logical size it is drawn.
        scale: f64,
    },
}

impl CaptureView {
    /// Returns how large the image is; none for an output with no mode.
    pub(crate) fn size(&self) -> Option<Size<i32, Physical>> {
        match self {
            Self::Output(output) => output.current_mode().map(|mode| mode.size),
            Self::Window { size, .. } => Some(*size),
        }
    }

    /// Returns how the image is turned from upright: as the output is,
    /// or not at all for a window.
    pub(crate) fn transform(&self) -> Transform {
        match self {
            Self::Output(output) => output.current_transform(),
            Self::Window { .. } => Transform::Normal,
        }
    }

    /// Returns how much larger than logical sizes it is drawn.
    fn scale(&self) -> smithay::utils::Scale<f64> {
        match self {
            Self::Output(output) => output.current_scale().fractional_scale().into(),
            Self::Window { scale, .. } => (*scale).into(),
        }
    }

    /// Returns what `scene` shows of this, drawn with `renderer`.
    fn elements<R>(&self, scene: &Scene<'_>, renderer: &mut R) -> Vec<OutputElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        match self {
            Self::Output(output) => scene.output_elements(renderer, output),
            Self::Window { window, .. } => scene.window_elements(renderer, *window, self.scale()),
        }
    }
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
    view: &CaptureView,
) -> Result<CapturedImage, CaptureError>
where
    R: Renderer + ImportAll + ImportMem + Offscreen<Target> + Bind<Target> + ExportMem,
    R::TextureId: Send + Clone + 'static,
    R::Error: Send + Sync + 'static,
{
    let size = view.size().ok_or(CaptureError::EmptyOutput)?;
    let width = usize::try_from(size.w).map_err(|_negative| CaptureError::EmptyOutput)?;
    let height = usize::try_from(size.h).map_err(|_negative| CaptureError::EmptyOutput)?;
    let buffer_size: Size<i32, smithay::utils::Buffer> = (size.w, size.h).into();
    let mut target = renderer
        .create_buffer(Fourcc::Argb8888, buffer_size)
        .map_err(CaptureError::renderer)?;
    let elements = view.elements(scene, renderer);
    let mut framebuffer = renderer.bind(&mut target).map_err(CaptureError::renderer)?;
    // Reading the frame back below waits for the GPU to finish it.
    let _drawn = render_view(renderer, &mut framebuffer, &elements, view)?;
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

/// Draws what `scene` shows on `output` with `renderer` straight into
/// `target`, a client's GPU buffer, and waits until the GPU is done, so
/// the client never reads half a frame. Nothing is copied through the
/// CPU: screen sharing and recording at full rate cost the GPU alone.
///
/// # Errors
///
/// Returns an error if the output has no mode, or the scene could not be
/// drawn into the buffer.
pub(crate) fn draw_capture_into<R, Target>(
    renderer: &mut R,
    scene: &Scene<'_>,
    view: &CaptureView,
    target: &mut Target,
) -> Result<(), CaptureError>
where
    R: Renderer + ImportAll + ImportMem + Bind<Target>,
    R::TextureId: Send + Clone + 'static,
    R::Error: Send + Sync + 'static,
{
    let elements = view.elements(scene, renderer);
    let mut framebuffer = renderer.bind(target).map_err(CaptureError::renderer)?;
    let sync = render_view(renderer, &mut framebuffer, &elements, view)?;
    sync.wait()
        .map_err(|_interrupted| CaptureError::Draw("the GPU did not finish".to_owned()))
}

/// Draws `elements` of `view` into `framebuffer`, turned as it is;
/// returns when the GPU will be done.
fn render_view<R, Element>(
    renderer: &mut R,
    framebuffer: &mut R::Framebuffer<'_>,
    elements: &[Element],
    view: &CaptureView,
) -> Result<SyncPoint, CaptureError>
where
    R: Renderer,
    Element: RenderElement<R>,
{
    let size = view.size().ok_or(CaptureError::EmptyOutput)?;
    let mut tracker = OutputDamageTracker::new(size, view.scale(), view.transform());
    let result = tracker
        .render_output(renderer, framebuffer, 0, elements, scene::BACKGROUND_COLOR)
        .map_err(|error| CaptureError::Draw(error.to_string()))?;
    Ok(result.sync)
}

impl ScreenCapture {
    /// Fills `frame`, a shared-memory buffer's, with `image`, of an output
    /// turned as `transform`, made `time` after vaht started, or fails it
    /// for why the image could not be made.
    fn fill(
        frame: Frame,
        image: Result<CapturedImage, CaptureError>,
        transform: Transform,
        time: Duration,
    ) {
        let written = image.and_then(|image| {
            shared_memory::write_frame(&frame.buffer(), &image.pixels, image.width, image.height)
                .map_err(CaptureError::Write)
        });
        Self::finish(frame, written, transform, time);
    }

    /// Tells `frame`'s client its image is ready (`drawn`), of an output
    /// turned as `transform`, made `time` after vaht started, or why not.
    fn finish(frame: Frame, drawn: Result<(), CaptureError>, transform: Transform, time: Duration) {
        match drawn {
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
        source
            .user_data()
            .insert_if_missing(|| CaptureTarget::Output(output.downgrade()));
    }
}

impl ToplevelCaptureSourceHandler for Compositor {
    fn toplevel_capture_source_state(&mut self) -> &mut ToplevelCaptureSourceState {
        &mut self.protocols.toplevel_capture_source
    }

    fn toplevel_source_created(
        &mut self,
        source: ImageCaptureSource,
        toplevel: ForeignToplevelHandle,
    ) {
        if let Some(window) = self
            .toplevel_list
            .window_where(|handle| handle.matches(&toplevel))
        {
            source
                .user_data()
                .insert_if_missing(|| CaptureTarget::Window(window));
        }
    }
}

impl ImageCopyCaptureHandler for Compositor {
    fn image_copy_capture_state(&mut self) -> &mut ImageCopyCaptureState {
        &mut self.protocols.image_copy_capture
    }

    fn capture_constraints(&mut self, source: &ImageCaptureSource) -> Option<BufferConstraints> {
        let size = self.capture_view(source)?.size()?;
        Some(BufferConstraints {
            size: (size.w, size.h).into(),
            shm: vec![Format::Argb8888, Format::Xrgb8888],
            // On the hardware, captures are drawn straight into GPU buffers
            // too, as screen sharing and OBS ask for.
            dma: self.backend.capture_dmabuf_constraints(),
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
        let Some(view) = self.capture_view(&session.source()) else {
            frame.fail(CaptureFailureReason::Stopped);
            return;
        };
        let (transform, time) = (view.transform(), self.start_time.elapsed());
        if let Ok(dmabuf) = smithay::wayland::dmabuf::get_dmabuf(&frame.buffer()) {
            let mut dmabuf = dmabuf.clone();
            let drawn = self.with_capture_scene(session.draw_cursor(), |backend, scene| {
                backend.capture_into(scene, &view, &mut dmabuf)
            });
            ScreenCapture::finish(frame, drawn, transform, time);
        } else {
            let image = self.with_capture_scene(session.draw_cursor(), |backend, scene| {
                backend.capture(scene, &view)
            });
            ScreenCapture::fill(frame, image, transform, time);
        }
    }
}

impl Compositor {
    /// Runs `draw` with the backend and the scene as it is now, the
    /// pointer drawn in it if `draws_pointer`: what a capture shows.
    pub(crate) fn with_capture_scene<T>(
        &mut self,
        draws_pointer: bool,
        draw: impl FnOnce(&mut crate::backend::Backend, &Scene<'_>) -> T,
    ) -> T {
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
        let scene = if draws_pointer {
            scene.with_pointer(&self.cursor, pointer)
        } else {
            scene
        };
        draw(&mut self.backend, &scene)
    }

    /// Returns what a capture of `source` draws now: its output, or its
    /// window's content at the scale of the output showing most of it;
    /// none once the output or window is gone, or a window is not placed.
    fn capture_view(&self, source: &ImageCaptureSource) -> Option<CaptureView> {
        match source.user_data().get::<CaptureTarget>()? {
            CaptureTarget::Output(output) => output.upgrade().map(CaptureView::Output),
            CaptureTarget::Window(id) => {
                let area = self.window_manager.window(*id)?.content_area();
                let scale = self
                    .outputs
                    .most_overlapping(area)
                    .map_or(1.0, |output| output.current_scale().fractional_scale());
                let size: Size<i32, Physical> =
                    area.size.to_f64().to_physical(scale).to_i32_round();
                (size.w > 0 && size.h > 0).then_some(CaptureView::Window {
                    window: *id,
                    size,
                    scale,
                })
            }
        }
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
