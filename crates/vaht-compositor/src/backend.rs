//! Where vaht's frames go and its input comes from.
//!
//! [`Backend`] is an enum of the ways vaht can run (rung 2 of R-M15):
//! directly on the hardware from a virtual terminal ([`DrmBackend`]),
//! nested in a window of another compositor ([`WinitBackend`]), or
//! without any screen for tests ([`HeadlessBackend`]). The rest of the
//! compositor never knows which one it has.

mod drm;
mod headless;
mod winit;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Format, Fourcc};
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::{ExportMem, ImportMemWl, Texture as _};
use smithay::output::Output;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::pixman::Image;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::utils::{Rectangle, Transform};
use smithay::wayland::image_copy_capture::DmabufConstraints;

pub(crate) use self::drm::DrmBackend;
pub(crate) use self::drm::plan::{TopWindow, virtual_terminal};
pub(crate) use self::headless::HeadlessBackend;
pub(crate) use self::winit::WinitBackend;
use crate::Compositor;
use crate::capture::{self, CaptureError, CapturedImage};
use crate::config::OutputMode;
use crate::gamma::GammaRamps;
use crate::outputs::{OutputControl, Outputs};
use crate::resources::ResourceLedger;
use crate::scene::Scene;

/// The environment variables whose presence means a host compositor is
/// running, so vaht can open a window in it.
const HOST_DISPLAY_VARIABLES: [&str; 2] = ["WAYLAND_DISPLAY", "DISPLAY"];

/// Which backend vaht should run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendChoice {
    /// Whichever the environment allows: nested when a host compositor runs.
    #[default]
    Automatic,
    /// No screen at all; frames are drawn in memory. For tests.
    Headless,
    /// Directly on the hardware, from a virtual terminal.
    Drm,
}

/// A change in the outputs a backend has.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum OutputEvent {
    /// An output appeared: a monitor plugged in or turned on.
    Connected(Output),
    /// An output went away: a monitor unplugged or turned off.
    Disconnected(Output),
    /// An output's preferred mode changed while it stayed connected (its
    /// monitor told more about itself, or was swapped for another).
    Changed(Output),
}

/// How vaht shows frames and receives input.
///
/// The variants are boxed: backends are large and differ in size by a factor
/// of ten, and there is only ever one, so the indirection costs nothing.
#[derive(Debug)]
pub(crate) enum Backend {
    /// In a window of a host compositor.
    Winit(Box<WinitBackend>),
    /// In memory only.
    Headless(Box<HeadlessBackend>),
    /// On the hardware.
    Drm(Box<DrmBackend>),
}

impl Backend {
    /// Starts the backend `choice` asks for.
    ///
    /// # Errors
    ///
    /// Returns an error if no backend can run here, or the chosen one fails
    /// to start.
    pub(crate) fn start(
        choice: BackendChoice,
        handle: &LoopHandle<'static, Compositor>,
        display: &DisplayHandle,
        ledger: &ResourceLedger,
    ) -> Result<Self, BackendError> {
        match choice {
            BackendChoice::Headless => Ok(Self::Headless(Box::new(HeadlessBackend::new(
                handle, display, ledger,
            )?))),
            BackendChoice::Automatic
                if HOST_DISPLAY_VARIABLES
                    .iter()
                    .any(|variable| std::env::var_os(variable).is_some()) =>
            {
                Ok(Self::Winit(Box::new(WinitBackend::new(
                    handle, display, ledger,
                )?)))
            }
            BackendChoice::Drm | BackendChoice::Automatic => Ok(Self::Drm(Box::new(
                DrmBackend::new(handle, display, ledger)?,
            ))),
        }
    }

    /// Returns the outputs frames are shown on, laid out side by side.
    pub(crate) fn outputs(&self) -> Outputs {
        match self {
            Self::Winit(backend) => backend.outputs(),
            Self::Headless(backend) => backend.outputs(),
            Self::Drm(backend) => backend.outputs(),
        }
    }

    /// Returns whether vaht draws the pointer on this backend's outputs;
    /// nested, the host draws it, and headless, no one sees it.
    pub(crate) const fn draws_pointer(&self) -> bool {
        match self {
            Self::Winit(_) | Self::Headless(_) => false,
            Self::Drm(_) => true,
        }
    }

    /// Returns how many values each of `output`'s gamma ramps has; none
    /// where its colors cannot change (nested, the host's are used).
    pub(crate) fn gamma_size(&self, output: &Output) -> Option<u32> {
        match self {
            Self::Winit(_) => None,
            Self::Headless(_) => Some(HeadlessBackend::GAMMA_SIZE),
            Self::Drm(backend) => backend.gamma_size(output),
        }
    }

    /// Changes `output`'s colors by `ramps`, or back to normal without.
    pub(crate) fn set_gamma(&mut self, output: &Output, ramps: Option<&GammaRamps>) {
        match self {
            Self::Winit(_) => {}
            Self::Headless(_) => {
                tracing::info!(
                    output = output.name(),
                    is_normal = ramps.is_none(),
                    "gamma set"
                );
            }
            Self::Drm(backend) => backend.set_gamma(output, ramps),
        }
    }

    /// Asks for a frame to be drawn soon.
    pub(crate) fn schedule_render(&mut self) {
        match self {
            Self::Winit(backend) => backend.schedule_render(),
            Self::Headless(backend) => backend.schedule_render(),
            Self::Drm(backend) => backend.schedule_render(),
        }
    }

    /// Frees textures cached for client buffers that no longer exist.
    pub(crate) fn release_unused_textures(&mut self) {
        match self {
            Self::Winit(backend) => backend.release_unused_textures(),
            Self::Headless(backend) => backend.release_unused_textures(),
            Self::Drm(backend) => backend.release_unused_textures(),
        }
    }

    /// Returns the GPU buffer formats the renderer can import, if it can
    /// import any.
    pub(crate) fn dmabuf_formats(&mut self) -> Option<Vec<Format>> {
        match self {
            Self::Winit(backend) => Some(backend.dmabuf_formats()),
            Self::Drm(backend) => Some(backend.dmabuf_formats()),
            Self::Headless(_) => None,
        }
    }

    /// Imports a client's GPU buffer; returns whether the renderer can use
    /// it.
    pub(crate) fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        match self {
            Self::Winit(backend) => backend.import_dmabuf(dmabuf),
            Self::Drm(backend) => backend.import_dmabuf(dmabuf),
            Self::Headless(_) => false,
        }
    }

    /// Returns the picture in a client's shared-memory `buffer` as an icon
    /// (a window's, set over xdg-toplevel-icon), if it can be read.
    ///
    /// The renderer reads it: it takes the buffer in as a texture and
    /// hands its pixels back, which needs no `unsafe` in vaht (R-X3), as
    /// reading the client's memory directly would.
    pub(crate) fn read_icon(&mut self, buffer: &WlBuffer) -> Option<vaht_ui::Icon> {
        match self {
            Self::Winit(backend) => Self::read_icon_with(backend.renderer(), buffer),
            Self::Headless(backend) => Self::read_icon_with(backend.renderer(), buffer),
            Self::Drm(backend) => backend
                .renderer()
                .ok()
                .and_then(|mut renderer| Self::read_icon_with(&mut renderer, buffer)),
        }
    }

    /// Draws a frame of `scene`.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame could not be drawn or shown.
    pub(crate) fn render(&mut self, scene: &Scene<'_>) -> Result<(), RenderError> {
        match self {
            Self::Winit(backend) => backend.render(scene),
            Self::Headless(backend) => backend.render(scene),
            Self::Drm(backend) => backend.render(scene),
        }
    }

    /// Draws what `scene` shows on `output` with this backend's renderer,
    /// for a screenshot or a screen recording.
    ///
    /// # Errors
    ///
    /// Returns why the image could not be made.
    pub(crate) fn capture(
        &mut self,
        scene: &Scene<'_>,
        output: &Output,
    ) -> Result<CapturedImage, CaptureError> {
        match self {
            Self::Winit(backend) => {
                capture::draw_capture::<_, GlesTexture>(backend.renderer(), scene, output)
            }
            Self::Headless(backend) => capture::draw_capture::<_, Image<'static, 'static>>(
                backend.renderer(),
                scene,
                output,
            ),
            Self::Drm(backend) => backend.capture(scene, output),
        }
    }

    /// Returns what GPU buffers captures may be drawn into: on the DRM
    /// backend, the primary GPU's; none elsewhere, where captures go
    /// through shared memory.
    pub(crate) fn capture_dmabuf_constraints(&mut self) -> Option<DmabufConstraints> {
        match self {
            Self::Drm(backend) => backend.capture_dmabuf_constraints(),
            Self::Winit(_) | Self::Headless(_) => None,
        }
    }

    /// Draws what `scene` shows on `output` straight into `dmabuf`, a
    /// client's GPU buffer.
    ///
    /// # Errors
    ///
    /// Returns why it could not be drawn: only the DRM backend offers GPU
    /// buffers.
    pub(crate) fn capture_into(
        &mut self,
        scene: &Scene<'_>,
        output: &Output,
        dmabuf: &mut Dmabuf,
    ) -> Result<(), CaptureError> {
        match self {
            Self::Drm(backend) => backend.capture_into(scene, output, dmabuf),
            Self::Winit(_) | Self::Headless(_) => Err(CaptureError::Draw(
                "GPU buffers are offered on the DRM backend alone".to_owned(),
            )),
        }
    }

    /// Reads `buffer` as an icon with `renderer`.
    fn read_icon_with<R>(renderer: &mut R, buffer: &WlBuffer) -> Option<vaht_ui::Icon>
    where
        R: ImportMemWl + ExportMem,
    {
        let texture = renderer
            .import_shm_buffer(buffer, None, &[])
            .inspect_err(|error| tracing::debug!(%error, "a window's icon could not be taken in"))
            .ok()?;
        let size = texture.size();
        let mapping = renderer
            .copy_texture(&texture, Rectangle::from_size(size), Fourcc::Argb8888)
            .inspect_err(|error| tracing::debug!(%error, "a window's icon could not be read"))
            .ok()?;
        let bytes = renderer.map_texture(&mapping).ok()?;
        let (width, height) = (u32::try_from(size.w).ok()?, u32::try_from(size.h).ok()?);
        vaht_ui::Icon::from_argb8888(width, height, bytes)
    }
}

impl OutputControl for Backend {
    fn set_enabled(&mut self, output: &Output, is_enabled: bool) {
        match self {
            Self::Winit(backend) => backend.set_enabled(output, is_enabled),
            Self::Headless(backend) => backend.set_enabled(output, is_enabled),
            Self::Drm(backend) => backend.set_enabled(output, is_enabled),
        }
    }

    fn set_transform(&mut self, output: &Output, transform: Transform) {
        match self {
            Self::Winit(backend) => backend.set_transform(output, transform),
            Self::Headless(backend) => backend.set_transform(output, transform),
            Self::Drm(backend) => backend.set_transform(output, transform),
        }
    }

    fn set_mode(&mut self, output: &Output, mode: Option<OutputMode>) {
        match self {
            Self::Winit(backend) => backend.set_mode(output, mode),
            Self::Headless(backend) => backend.set_mode(output, mode),
            Self::Drm(backend) => backend.set_mode(output, mode),
        }
    }
}

/// Why no backend could start.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// vaht cannot have the seat: it was not started from a virtual
    /// terminal, or neither logind nor seatd runs.
    #[error(
        "could not take the seat ({0}); run vaht from a virtual terminal, with \
         logind or seatd running, or nested inside another compositor"
    )]
    Seat(String),

    /// No GPU could be used.
    #[error("no GPU to draw with: {0}")]
    Gpu(String),

    /// The window in the host compositor could not be opened.
    #[error("could not open a window in the host compositor: {reason}")]
    OpenWindow {
        /// Smithay's error and its causes, joined. Kept as text because one
        /// of Smithay's causes cannot be sent between threads.
        reason: String,
    },

    /// The software renderer could not be created.
    #[error("could not create the software renderer")]
    SoftwareRenderer(#[source] smithay::backend::renderer::pixman::PixmanError),

    /// The in-memory image frames are drawn into could not be created.
    #[error("could not create the in-memory frame image")]
    FrameImage(#[source] smithay::reexports::pixman::CreateFailed),

    /// The window's events could not be registered with the event loop.
    #[error("could not receive the window's events")]
    RegisterEvents(#[source] calloop::Error),
}

impl BackendError {
    /// Describes Smithay's failure to open the host window, causes included.
    pub(crate) fn open_window(error: &smithay::backend::winit::Error) -> Self {
        let causes: Vec<String> =
            std::iter::successors(Some(error as &dyn std::error::Error), |cause| {
                cause.source()
            })
            .map(ToString::to_string)
            .collect();
        Self::OpenWindow {
            reason: causes.join(": "),
        }
    }
}

/// Why a frame could not be drawn.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The buffer to draw into could not be obtained.
    #[error("could not obtain a buffer to draw into")]
    Bind(#[source] smithay::backend::SwapBuffersError),

    /// Drawing failed.
    #[error("could not draw the frame")]
    Draw(
        #[source]
        smithay::backend::renderer::damage::Error<smithay::backend::renderer::gles::GlesError>,
    ),

    /// The in-memory image could not be drawn into.
    #[error("could not draw into the in-memory frame image")]
    BindSoftware(#[source] smithay::backend::renderer::pixman::PixmanError),

    /// Drawing in software failed.
    #[error("could not draw the frame in software")]
    DrawSoftware(
        #[source]
        smithay::backend::renderer::damage::Error<smithay::backend::renderer::pixman::PixmanError>,
    ),

    /// The frame could not be shown.
    #[error("could not show the frame")]
    Submit(#[source] smithay::backend::SwapBuffersError),

    /// Drawing on a GPU for a monitor failed.
    #[error("could not draw on the monitor: {0}")]
    Hardware(String),
}
