//! vaht running headless, driven by real Wayland clients.
//!
//! Each test starts the `vaht` executable with `--backend headless` and its
//! own runtime and log directories, connects test clients that open and close
//! windows, then stops vaht with SIGTERM and reads its exit report. The
//! report must show every window and client gone, and as many open
//! descriptors as at startup: anything else is a leak.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, FrameCallbackData};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, EventQueue, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::WaylandSurface as _;
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::xdg::window::{
    Window, WindowConfigure, WindowDecorations, WindowHandler,
};
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_registry, registry_handlers};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// How long to wait for vaht to reach a state before failing the test.
const PATIENCE: Duration = Duration::from_secs(10);

/// How often to look at the log while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The size of every test window's buffer, in pixels.
const WINDOW_SIZE: u32 = 64;

/// Bytes per pixel of an ARGB8888 buffer.
const BYTES_PER_PIXEL: u32 = 4;

/// A `vaht --backend headless` process with private directories.
struct HeadlessVaht {
    /// The process.
    process: Child,
    /// Its `XDG_RUNTIME_DIR`, where its socket is; removed with the test.
    runtime: tempfile::TempDir,
    /// Where it logs; removed with the test.
    logs: tempfile::TempDir,
    /// The socket clients connect to.
    socket: PathBuf,
}

impl HeadlessVaht {
    /// Starts vaht and waits until it accepts clients.
    fn start() -> Self {
        // Socket paths are limited to about 108 bytes, so the runtime
        // directory must be short: the system's temporary directory is.
        let runtime = tempfile::Builder::new()
            .prefix("vaht-")
            .tempdir()
            .expect("a temporary runtime directory");
        let logs = tempfile::tempdir().expect("a temporary log directory");
        let process = Command::new(EXECUTABLE)
            .args(["--backend", "headless", "--journald", "off", "--verbose"])
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env("VAHT_LOG_DIRECTORY", logs.path())
            .env_remove("VAHT_LOG")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the vaht executable starts");
        let mut vaht = Self {
            process,
            socket: PathBuf::new(),
            runtime,
            logs,
        };
        let line = vaht.wait_for_log(|log| {
            log.lines()
                .find(|line| line.contains("accepting clients"))
                .map(ToOwned::to_owned)
        });
        let name = line
            .split_whitespace()
            .find_map(|field| field.strip_prefix("socket="))
            .expect("the accepting line names the socket");
        vaht.socket = vaht.runtime.path().join(name);
        vaht
    }

    /// Returns the socket clients connect to.
    fn socket(&self) -> &Path {
        &self.socket
    }

    /// Waits until vaht has logged `count` client disconnections.
    fn wait_for_disconnections(&self, count: usize) {
        self.wait_for_log(|log| {
            (log.matches("client disconnected").count() >= count).then_some(())
        });
    }

    /// Stops vaht with SIGTERM and returns its log.
    fn stop(mut self) -> ExitReport {
        let pid = i32::try_from(self.process.id()).expect("process ids fit in an i32");
        kill(Pid::from_raw(pid), Signal::SIGTERM).expect("vaht can be signalled");
        let status = self.process.wait().expect("vaht can be waited for");
        assert!(status.success(), "vaht did not stop cleanly: {status}");
        ExitReport {
            log: self.read_log(),
        }
    }

    /// Returns the log so far.
    fn read_log(&self) -> String {
        std::fs::read_to_string(self.logs.path().join("vaht.log")).unwrap_or_default()
    }

    /// Polls the log until `found` finds what it looks for.
    ///
    /// # Panics
    ///
    /// Panics with the log if [`PATIENCE`] runs out first.
    fn wait_for_log<T>(&self, found: impl Fn(&str) -> Option<T>) -> T {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let log = self.read_log();
            if let Some(value) = found(&log) {
                return value;
            }
            assert!(
                Instant::now() < deadline,
                "vaht never got there; log:\n{log}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

impl Drop for HeadlessVaht {
    /// Never leaves a vaht running when a test fails.
    fn drop(&mut self) {
        // Errors mean it has already exited, which is what drop wants.
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// What vaht reported as it stopped.
struct ExitReport {
    /// The whole log.
    log: String,
}

impl ExitReport {
    /// Returns the live count of `kind` in the exit resource report.
    fn live_count(&self, kind: &str) -> usize {
        let report = self
            .log
            .split("resources at exit")
            .nth(1)
            .expect("vaht logged its resources at exit");
        report
            .lines()
            .find_map(|line| {
                let line = line.trim_start();
                let rest = line.strip_prefix(kind)?.strip_prefix(": ")?;
                rest.split_whitespace().next()?.parse().ok()
            })
            .unwrap_or_else(|| panic!("the exit report has no {kind} count:\n{}", self.log))
    }

    /// Returns the open descriptor count vaht logged at `moment`.
    fn descriptors(&self, moment: &str) -> usize {
        let marker = format!("process usage {moment}");
        self.log
            .lines()
            .find(|line| line.contains(&marker))
            .and_then(|line| {
                line.split_whitespace()
                    .find_map(|field| field.strip_prefix("descriptors="))
            })
            .and_then(|count| count.parse().ok())
            .unwrap_or_else(|| panic!("vaht logged no descriptor count {moment}:\n{}", self.log))
    }

    /// Asserts that nothing vaht acquired for clients outlived them.
    fn assert_nothing_left_behind(&self) {
        assert_eq!(
            self.live_count("window"),
            0,
            "windows left behind:\n{}",
            self.log
        );
        assert_eq!(
            self.live_count("client"),
            0,
            "clients left behind:\n{}",
            self.log
        );
        assert_eq!(
            self.descriptors("at exit"),
            self.descriptors("at start"),
            "descriptors leaked:\n{}",
            self.log
        );
    }
}

/// A Wayland client that opens plain windows and waits for them to be shown.
struct TestClient {
    /// The client's event queue.
    queue: EventQueue<ClientState>,
    /// What the client knows and has.
    state: ClientState,
}

impl TestClient {
    /// Connects to vaht's socket.
    fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).expect("vaht accepts connections");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, queue) = registry_queue_init(&connection).expect("vaht lists its globals");
        let handle = queue.handle();
        let shm = Shm::bind(&globals, &handle).expect("vaht offers wl_shm");
        let pool_size = usize::try_from(WINDOW_SIZE * WINDOW_SIZE * BYTES_PER_PIXEL)
            .expect("a small buffer size fits in usize");
        let state = ClientState {
            registry: RegistryState::new(&globals),
            outputs: OutputState::new(&globals, &handle),
            compositor: CompositorState::bind(&globals, &handle)
                .expect("vaht offers wl_compositor"),
            xdg_shell: XdgShell::bind(&globals, &handle).expect("vaht offers xdg_wm_base"),
            pool: SlotPool::new(pool_size, &shm).expect("a shared-memory pool"),
            shm,
            windows: Vec::new(),
            frames_shown: 0,
        };
        Self { queue, state }
    }

    /// Opens `count` windows and waits until each has been shown once.
    fn open_windows(&mut self, count: usize) {
        let handle = self.queue.handle();
        for number in 0..count {
            let surface = self.state.compositor.create_surface(&handle);
            let window = self.state.xdg_shell.create_window(
                surface,
                WindowDecorations::RequestServer,
                &handle,
            );
            window.set_title(format!("test window {number}"));
            window.set_app_id("vaht.test");
            window.commit();
            self.state.windows.push(window);
        }
        let expected = self.state.frames_shown + count;
        while self.state.frames_shown < expected {
            self.queue
                .blocking_dispatch(&mut self.state)
                .expect("the connection to vaht stays up");
        }
    }

    /// Closes every window and waits until vaht has processed it.
    fn close_windows(&mut self) {
        self.state.windows.clear();
        self.queue
            .roundtrip(&mut self.state)
            .expect("the connection to vaht stays up");
    }
}

/// The client's state, which the toolkit's handlers update.
struct ClientState {
    /// The globals vaht offers.
    registry: RegistryState,
    /// vaht's outputs.
    outputs: OutputState,
    /// Creates surfaces.
    compositor: CompositorState,
    /// Creates windows.
    xdg_shell: XdgShell,
    /// Shared-memory buffers.
    shm: Shm,
    /// Where buffers are allocated.
    pool: SlotPool,
    /// The open windows.
    windows: Vec<Window>,
    /// How many frame callbacks arrived: each one is a window shown.
    frames_shown: usize,
}

impl ClientState {
    /// Draws `window` in a solid color and asks to hear when it is shown.
    fn draw(&mut self, window: &Window, handle: &QueueHandle<Self>) {
        let side = i32::try_from(WINDOW_SIZE).expect("a small size fits in i32");
        let stride = side * i32::try_from(BYTES_PER_PIXEL).expect("4 fits in i32");
        let (buffer, canvas) = self
            .pool
            .create_buffer(side, side, stride, wl_shm::Format::Argb8888)
            .expect("the pool has room");
        canvas.fill(0xff);
        let surface = window.wl_surface();
        surface.frame(handle, FrameCallbackData(surface.clone()));
        buffer.attach_to(surface).expect("a fresh buffer attaches");
        surface.damage_buffer(0, 0, side, side);
        window.commit();
    }
}

impl CompositorHandler for ClientState {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frames_shown += 1;
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for ClientState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for ClientState {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {}

    fn configure(
        &mut self,
        _: &Connection,
        handle: &QueueHandle<Self>,
        window: &Window,
        _: WindowConfigure,
        _: u32,
    ) {
        self.draw(window, handle);
    }
}

impl ShmHandler for ClientState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for ClientState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState];
}

delegate_registry!(ClientState);
smithay_client_toolkit::delegate_dispatch2!(ClientState);

#[test]
fn windows_are_shown_and_leave_nothing_behind_when_closed() {
    let vaht = HeadlessVaht::start();
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(3);
    client.close_windows();
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn many_clients_coming_and_going_leave_nothing_behind() {
    const ROUNDS: usize = 20;
    const WINDOWS_PER_CLIENT: usize = 5;
    let vaht = HeadlessVaht::start();
    for _ in 0..ROUNDS {
        let mut client = TestClient::connect(vaht.socket());
        client.open_windows(WINDOWS_PER_CLIENT);
        client.close_windows();
    }
    vaht.wait_for_disconnections(ROUNDS);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn a_client_that_disconnects_with_windows_open_leaves_nothing_behind() {
    let vaht = HeadlessVaht::start();
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(4);
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}
