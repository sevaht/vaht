//! Where vaht's frames go and its input comes from.
//!
//! [`Backend`] is an enum of the ways vaht can run (rung 2 of R-M15): nested
//! in a window of another compositor ([`WinitBackend`]), or without any
//! screen for tests ([`HeadlessBackend`]); directly on the hardware (DRM,
//! libinput) comes next. The rest of the compositor never knows which one it
//! has.

mod headless;
mod winit;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Format, Fourcc};
use smithay::backend::renderer::{ExportMem, ImportMemWl, Texture as _};
use smithay::output::Output;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::utils::{Rectangle, Transform};

pub(crate) use self::headless::HeadlessBackend;
pub(crate) use self::winit::WinitBackend;
use crate::Compositor;
use crate::config::OutputMode;
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
            BackendChoice::Automatic => Err(BackendError::NoHostDisplay),
        }
    }

    /// Returns the outputs frames are shown on, laid out side by side.
    pub(crate) fn outputs(&self) -> Outputs {
        match self {
            Self::Winit(backend) => backend.outputs(),
            Self::Headless(backend) => backend.outputs(),
        }
    }

    /// Returns whether vaht draws the pointer on this backend's outputs;
    /// nested, the host draws it, and headless, no one sees it.
    pub(crate) const fn draws_pointer(&self) -> bool {
        match self {
            Self::Winit(_) | Self::Headless(_) => false,
        }
    }

    /// Asks for a frame to be drawn soon.
    pub(crate) fn schedule_render(&mut self) {
        match self {
            Self::Winit(backend) => backend.schedule_render(),
            Self::Headless(backend) => backend.schedule_render(),
        }
    }

    /// Frees textures cached for client buffers that no longer exist.
    pub(crate) fn release_unused_textures(&mut self) {
        match self {
            Self::Winit(backend) => backend.release_unused_textures(),
            Self::Headless(backend) => backend.release_unused_textures(),
        }
    }

    /// Returns the GPU buffer formats the renderer can import, if it can
    /// import any.
    pub(crate) fn dmabuf_formats(&mut self) -> Option<Vec<Format>> {
        match self {
            Self::Winit(backend) => Some(backend.dmabuf_formats()),
            Self::Headless(_) => None,
        }
    }

    /// Imports a client's GPU buffer; returns whether the renderer can use
    /// it.
    pub(crate) fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        match self {
            Self::Winit(backend) => backend.import_dmabuf(dmabuf),
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
        }
    }

    fn set_transform(&mut self, output: &Output, transform: Transform) {
        match self {
            Self::Winit(backend) => backend.set_transform(output, transform),
            Self::Headless(backend) => backend.set_transform(output, transform),
        }
    }

    fn set_mode(&mut self, output: &Output, mode: Option<OutputMode>) {
        match self {
            Self::Winit(backend) => backend.set_mode(output, mode),
            Self::Headless(backend) => backend.set_mode(output, mode),
        }
    }
}

/// Why no backend could start.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// Neither `WAYLAND_DISPLAY` nor `DISPLAY` is set.
    #[error(
        "no host compositor to run nested in (WAYLAND_DISPLAY and DISPLAY are unset); \
         running directly on a TTY needs the DRM backend, which vaht does not have yet"
    )]
    NoHostDisplay,

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
}
