//! X11 programs through Xwayland: their windows are managed like any
//! other's, know where they are, close when vaht asks, and their menus
//! show over every window.

use std::path::Path;
use std::process::Command;

use vaht_ipc::{Reply, WindowInfo};
use vaht_test_support::{Bounds, HeadlessVaht, TestClient, X11Client};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The X window's color.
const X_COLOR: [u8; 3] = [0x40, 0xb0, 0x60];

/// The X menu's color.
const MENU_COLOR: [u8; 3] = [0xd0, 0x80, 0x20];

/// The X window's size.
const X_SIZE: (u16, u16) = (200, 120);

/// A headless vaht with Xwayland, an X11 program, and a Wayland client
/// for screenshots.
struct XDesktop {
    /// The compositor.
    vaht: HeadlessVaht,
    /// The X11 program.
    x11: X11Client,
    /// Takes screenshots.
    wayland: TestClient,
}

impl XDesktop {
    /// Starts vaht with Xwayland, windows cascading from the top left.
    fn new() -> Self {
        let vaht = HeadlessVaht::start_with_xwayland(
            Path::new(EXECUTABLE),
            &[("settings.toml", b"[placement]\nstrategy = \"cascade\"\n")],
        );
        let x11 = X11Client::connect(&vaht.x_display());
        let wayland = TestClient::connect(vaht.socket());
        Self { vaht, x11, wayland }
    }

    /// Returns the windows `vaht msg windows` lists.
    fn windows(&self) -> Vec<WindowInfo> {
        let output = Command::new(EXECUTABLE)
            .args(["msg", "--json", "windows"])
            .env(vaht_ipc::SOCKET_VARIABLE, self.vaht.ipc_socket())
            .output()
            .expect("vaht msg runs");
        match serde_json::from_slice(&output.stdout).expect("one JSON reply") {
            Reply::Windows { windows } => windows,
            other @ (Reply::Version { .. }
            | Reply::Done {}
            | Reply::Workspaces { .. }
            | Reply::Outputs { .. }
            | Reply::Error { .. }) => panic!("not a window list: {other:?}"),
        }
    }

    /// Runs `command` on the focused window, as `vaht msg run` does.
    fn run(&self, command: &[&str]) {
        let output = Command::new(EXECUTABLE)
            .args(["msg", "run"])
            .args(command)
            .env(vaht_ipc::SOCKET_VARIABLE, self.vaht.ipc_socket())
            .output()
            .expect("vaht msg runs");
        assert!(output.status.success(), "{output:?}");
    }

    /// Waits until the screen shows `color`, waiting for vaht to draw
    /// between looks (Xwayland draws a moment after a window maps);
    /// returns where.
    fn wait_for_color(&mut self, color: [u8; 3]) -> Bounds {
        for _ in 0..50 {
            let drawn = self.vaht.logged_count("frame drawn");
            if let Some(bounds) = self.wayland.screenshot().bounds_of(color) {
                return bounds;
            }
            self.vaht.wait_for_logged("frame drawn", drawn + 1);
        }
        panic!("{color:?} never showed");
    }

    /// Opens the X window and waits until vaht shows it; returns it.
    fn open_window(&mut self) -> u32 {
        let window = self
            .x11
            .open_window(X_SIZE, X_COLOR, "an X program", "xprogram");
        self.vaht.wait_for_logged("placed first", 1);
        window
    }
}

#[test]
fn an_x11_window_is_listed_with_its_title_and_class() {
    let mut desktop = XDesktop::new();
    let _window = desktop.open_window();
    let windows = desktop.windows();
    assert_eq!(windows.len(), 1, "{windows:?}");
    assert_eq!(windows[0].title, "an X program");
    assert_eq!(windows[0].app_id, "xprogram");
}

#[test]
fn an_x11_window_shows_framed_where_it_believes_it_is() {
    let mut desktop = XDesktop::new();
    let window = desktop.open_window();
    let shown = desktop.wait_for_color(X_COLOR);
    // Its content is where the X server says the window is, so an X
    // program's menus open where they should.
    let expected = (
        i16::try_from(shown.left).expect("small"),
        i16::try_from(shown.top).expect("small"),
    );
    assert_eq!(desktop.x11.wait_for_position(window, expected), expected);
    assert_eq!(
        (shown.right - shown.left + 1, shown.bottom - shown.top + 1),
        (u32::from(X_SIZE.0), u32::from(X_SIZE.1))
    );
    assert!(shown.top > 0, "a titlebar is above it");
}

#[test]
fn closing_an_x11_window_asks_its_program() {
    let mut desktop = XDesktop::new();
    let window = desktop.open_window();
    desktop.run(&["close"]);
    assert!(
        desktop.x11.wait_for_close_request(window),
        "the program was asked through WM_DELETE_WINDOW"
    );
}

#[test]
fn an_x11_menu_shows_over_the_windows() {
    let mut desktop = XDesktop::new();
    let _window = desktop.open_window();
    let shown = desktop.wait_for_color(X_COLOR);
    let corner = (
        i16::try_from(shown.left + 10).expect("small"),
        i16::try_from(shown.top + 10).expect("small"),
    );
    let _menu = desktop.x11.open_menu((corner, (60, 40)), MENU_COLOR);
    let menu = desktop.wait_for_color(MENU_COLOR);
    assert_eq!((menu.left, menu.top), (shown.left + 10, shown.top + 10));
}

#[test]
fn keys_typed_reach_the_focused_x11_window() {
    let mut desktop = XDesktop::new();
    let window = desktop.open_window();
    let _ = desktop.wait_for_color(X_COLOR);
    desktop.wayland.type_text("a");
    assert!(
        desktop.x11.wait_for_key_press(window),
        "the X program has the keyboard, through X's input focus"
    );
}
