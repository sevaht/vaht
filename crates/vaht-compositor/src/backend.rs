//! Where vaht's frames go and its input comes from.
//!
//! [`Backend`] is an enum of the ways vaht can run (rung 2 of R-M15): nested
//! in a window of another compositor ([`WinitBackend`]), or without any
//! screen for tests ([`HeadlessBackend`]); directly on the hardware (DRM,
//! libinput) comes next. The rest of the compositor never knows which one it
//! has.

mod headless;
mod winit;

use smithay::output::Output;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::{Logical, Rectangle};

pub(crate) use self::headless::HeadlessBackend;
pub(crate) use self::winit::WinitBackend;
use crate::Compositor;
use crate::resources::ResourceLedger;
use crate::windows::Windows;

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

    /// Returns the output frames are shown on.
    pub(crate) const fn output(&self) -> &Output {
        match self {
            Self::Winit(backend) => backend.output(),
            Self::Headless(backend) => backend.output(),
        }
    }

    /// Returns the output's area in global coordinates.
    pub(crate) fn output_geometry(&self) -> Rectangle<i32, Logical> {
        match self {
            Self::Winit(backend) => backend.output_geometry(),
            Self::Headless(backend) => backend.output_geometry(),
        }
    }

    /// Asks for a frame to be drawn soon.
    pub(crate) fn schedule_render(&mut self) {
        match self {
            Self::Winit(backend) => backend.schedule_render(),
            Self::Headless(backend) => backend.schedule_render(),
        }
    }

    /// Draws a frame of `windows`.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame could not be drawn or shown.
    pub(crate) fn render(&mut self, windows: &Windows) -> Result<(), RenderError> {
        match self {
            Self::Winit(backend) => backend.render(windows),
            Self::Headless(backend) => backend.render(windows),
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
