//! A program sharing one window (`ext_foreign_toplevel_image_capture_
//! source_manager_v1`), as tests drive it: OBS and the screen-sharing
//! portal capture a single window this way.
//!
//! [`WindowCapturer`] connects on its own, hears of each window through
//! `ext_foreign_toplevel_list_v1`, and captures one, by title, into shared
//! memory.

use std::path::Path;
use std::sync::Arc;

use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::{wl_buffer, wl_registry};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::reexports::client::backend::ObjectData;
use smithay_client_toolkit::reexports::client::{Connection, EventQueue, Proxy, QueueHandle};
use smithay_client_toolkit::reexports::protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1};
use smithay_client_toolkit::reexports::protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1};
use smithay_client_toolkit::reexports::protocols::ext::image_capture_source::v1::client::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1;
use smithay_client_toolkit::reexports::protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
use smithay_client_toolkit::reexports::protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1};
use smithay_client_toolkit::reexports::protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options};
use smithay_client_toolkit::reexports::protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1};
use smithay_client_toolkit::shm::raw::RawPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};

use crate::screenshot::Screenshot;
use crate::wait::PATIENCE;

/// Bytes per captured pixel.
const BYTES_PER_PIXEL: u32 = 4;

/// A connection that captures single windows.
#[derive(Debug)]
pub struct WindowCapturer {
    /// The events vaht sends.
    queue: EventQueue<CapturerState>,
    /// What they said.
    state: CapturerState,
    /// Turns windows into capture sources.
    sources: ExtForeignToplevelImageCaptureSourceManagerV1,
    /// Captures them.
    capture: ExtImageCopyCaptureManagerV1,
    /// The list, held while windows are listed.
    _list: ExtForeignToplevelListV1,
}

/// What vaht's events said.
#[derive(Debug)]
struct CapturerState {
    /// Shared memory, which images are captured into.
    shm: Shm,
    /// Each window's handle and title, in the order announced.
    windows: Vec<(ExtForeignToplevelHandleV1, String)>,
    /// The buffer size the current session asked for.
    size: Option<(u32, u32)>,
    /// How the current capture ended.
    result: Option<Result<(), String>>,
}

impl WindowCapturer {
    /// Connects to vaht at `socket` and starts listing its windows.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot be reached or lacks a global needed.
    #[must_use]
    pub fn connect(socket: &Path) -> Self {
        let stream =
            std::os::unix::net::UnixStream::connect(socket).expect("vaht's socket accepts");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, mut queue) =
            registry_queue_init::<CapturerState>(&connection).expect("the registry is read");
        let handle = queue.handle();
        let list = globals
            .bind(&handle, 1..=1, ListEvents)
            .expect("vaht offers ext_foreign_toplevel_list_v1");
        let sources = globals
            .bind(&handle, 1..=1, Ignored)
            .expect("vaht offers ext_foreign_toplevel_image_capture_source_manager_v1");
        let capture = globals
            .bind(&handle, 1..=1, Ignored)
            .expect("vaht offers ext_image_copy_capture_manager_v1");
        let mut state = CapturerState {
            shm: Shm::bind(&globals, &handle).expect("vaht offers wl_shm"),
            windows: Vec::new(),
            size: None,
            result: None,
        };
        queue.roundtrip(&mut state).expect("vaht answers");
        Self {
            queue,
            state,
            sources,
            capture,
            _list: list,
        }
    }

    /// Captures the window titled `title`, its content alone.
    ///
    /// # Panics
    ///
    /// Panics if vaht lists no such window, the capture fails or the
    /// connection to vaht does.
    pub fn capture(&mut self, title: &str) -> Screenshot {
        self.dispatch_until(|state| state.windows.iter().any(|(_, known)| known == title));
        let window = self
            .state
            .windows
            .iter()
            .find(|(_, known)| known == title)
            .map_or_else(
                || panic!("vaht lists a window titled {title}"),
                |(handle, _)| handle.clone(),
            );
        let handle = self.queue.handle();
        let source: ExtImageCaptureSourceV1 = self.sources.create_source(&window, &handle, Ignored);
        let session =
            self.capture
                .create_session(&source, Options::empty(), &handle, SessionEvents);
        source.destroy();
        self.state.size = None;
        self.dispatch_until(|state| state.size.is_some());
        let (width, height) = self.state.size.expect("the session announced a size");
        let signed = |value: u32| i32::try_from(value).expect("window sizes fit in i32");
        let length = usize::try_from(width * height * BYTES_PER_PIXEL)
            .expect("a window's size fits in usize");
        let mut pool = RawPool::new(length, &self.state.shm).expect("a shared-memory pool");
        let buffer = pool.create_buffer(
            0,
            signed(width),
            signed(height),
            signed(width * BYTES_PER_PIXEL),
            smithay_client_toolkit::reexports::client::protocol::wl_shm::Format::Argb8888,
            Ignored,
            &handle,
        );
        let frame = session.create_frame(&handle, FrameEvents);
        frame.attach_buffer(&buffer);
        frame.damage_buffer(0, 0, signed(width), signed(height));
        frame.capture();
        self.state.result = None;
        self.dispatch_until(|state| state.result.is_some());
        let result = self.state.result.take().expect("the frame finished");
        assert!(result.is_ok(), "vaht failed the capture: {result:?}");
        let pixels = pool.mmap().to_vec();
        frame.destroy();
        session.destroy();
        buffer.destroy();
        Screenshot::new(width, height, pixels)
    }

    /// Reads vaht's events until `is_done` holds of what they said.
    fn dispatch_until(&mut self, is_done: impl Fn(&CapturerState) -> bool) {
        let deadline = std::time::Instant::now() + PATIENCE;
        while !is_done(&self.state) {
            assert!(std::time::Instant::now() < deadline, "vaht answers in time");
            self.queue
                .blocking_dispatch(&mut self.state)
                .expect("vaht answers");
        }
    }
}

impl ShmHandler for CapturerState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

smithay_client_toolkit::delegate_dispatch2!(CapturerState);

/// User data of objects whose events say nothing needed.
#[derive(Debug, Clone, Copy)]
struct Ignored;

/// Implements [`Dispatch2`] for interfaces whose events say nothing
/// needed.
macro_rules! ignore_events {
    ($($interface:ty),+ $(,)?) => {
        $(
            impl Dispatch2<$interface, CapturerState> for Ignored {
                fn event(
                    &self,
                    _: &mut CapturerState,
                    _: &$interface,
                    _: <$interface as Proxy>::Event,
                    _: &Connection,
                    _: &QueueHandle<CapturerState>,
                ) {
                }
            }
        )+
    };
}

ignore_events!(
    wl_buffer::WlBuffer,
    ExtForeignToplevelImageCaptureSourceManagerV1,
    ExtImageCaptureSourceV1,
    ExtImageCopyCaptureManagerV1,
);

impl Dispatch2<wl_registry::WlRegistry, CapturerState> for GlobalListContents {
    fn event(
        &self,
        _: &mut CapturerState,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &Connection,
        _: &QueueHandle<CapturerState>,
    ) {
    }
}

/// User data of the window list: records each window announced.
#[derive(Debug, Clone, Copy)]
struct ListEvents;

impl Dispatch2<ExtForeignToplevelListV1, CapturerState> for ListEvents {
    fn event(
        &self,
        state: &mut CapturerState,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &Connection,
        _: &QueueHandle<CapturerState>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state.windows.push((toplevel, String::new()));
        }
    }

    fn event_created_child(
        opcode: u16,
        handle: &QueueHandle<CapturerState>,
    ) -> Arc<dyn ObjectData> {
        assert_eq!(
            opcode,
            ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE,
            "the list creates only window handles"
        );
        handle.make_data::<ExtForeignToplevelHandleV1, _>(HandleEvents)
    }
}

/// User data of a window's handle: records its title, and its closing.
#[derive(Debug, Clone, Copy)]
struct HandleEvents;

impl Dispatch2<ExtForeignToplevelHandleV1, CapturerState> for HandleEvents {
    fn event(
        &self,
        state: &mut CapturerState,
        proxy: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &Connection,
        _: &QueueHandle<CapturerState>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "only the title names a window here"
        )]
        match event {
            ext_foreign_toplevel_handle_v1::Event::Title { title } => {
                if let Some((_, known)) =
                    state.windows.iter_mut().find(|(handle, _)| handle == proxy)
                {
                    *known = title;
                }
            }
            ext_foreign_toplevel_handle_v1::Event::Closed => {
                state.windows.retain(|(handle, _)| handle != proxy);
            }
            _ => {}
        }
    }
}

/// User data of a capture session: records the buffer size it asks for.
#[derive(Debug, Clone, Copy)]
struct SessionEvents;

impl Dispatch2<ExtImageCopyCaptureSessionV1, CapturerState> for SessionEvents {
    fn event(
        &self,
        state: &mut CapturerState,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &Connection,
        _: &QueueHandle<CapturerState>,
    ) {
        if let ext_image_copy_capture_session_v1::Event::BufferSize { width, height } = event {
            state.size = Some((width, height));
        }
    }
}

/// User data of a capture frame: records how it ended.
#[derive(Debug, Clone, Copy)]
struct FrameEvents;

impl Dispatch2<ExtImageCopyCaptureFrameV1, CapturerState> for FrameEvents {
    fn event(
        &self,
        state: &mut CapturerState,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &Connection,
        _: &QueueHandle<CapturerState>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "only the frame's outcome matters to a test"
        )]
        match event {
            ext_image_copy_capture_frame_v1::Event::Ready => state.result = Some(Ok(())),
            ext_image_copy_capture_frame_v1::Event::Failed { reason } => {
                state.result = Some(Err(format!("{reason:?}")));
            }
            _ => {}
        }
    }
}
