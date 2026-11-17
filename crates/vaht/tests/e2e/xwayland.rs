//! X11 programs through Xwayland: their windows are managed like any
//! other's, know where they are, close when vaht asks, and their menus
//! show over every window.

use std::path::Path;
use std::process::Command;

use vaht_ipc::{Reply, WindowInfo};
use vaht_test_support::{
    BUTTON_LEFT, Bounds, ClipboardManager, HeadlessVaht, Selection, TestClient, WINDOW_COLOR,
    X11Client, keys,
};

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
            | Reply::LogFilter { .. }
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

    /// Waits until the screen shows `color` at `point`, waiting for vaht to
    /// draw between looks; returns whether it did.
    fn wait_for_color_at(&mut self, point: (u32, u32), color: [u8; 3]) -> bool {
        for _ in 0..50 {
            let drawn = self.vaht.logged_count("frame drawn");
            if self.wayland.screenshot().rgb(point.0, point.1) == color {
                return true;
            }
            self.vaht.wait_for_logged("frame drawn", drawn + 1);
        }
        false
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
fn an_x11_menu_shows_what_it_draws_after_it_appears() {
    const REDRAWN: [u8; 3] = [0x30, 0x60, 0xd0];
    const REDRAWN_AGAIN: [u8; 3] = [0xd0, 0x30, 0x90];
    let mut desktop = XDesktop::new();
    let menu = desktop.x11.open_menu(((50, 50), (60, 40)), MENU_COLOR);
    desktop.wait_for_color(MENU_COLOR);
    // Xwayland sends a window's next picture only once told its last one
    // showed (a frame callback): Steam's menus showed their first, black.
    desktop.x11.repaint(menu, REDRAWN);
    desktop.wait_for_color(REDRAWN);
    desktop.x11.repaint(menu, REDRAWN_AGAIN);
    desktop.wait_for_color(REDRAWN_AGAIN);
}

#[test]
fn a_transparent_x11_menu_shows_what_is_under_it() {
    let mut desktop = XDesktop::new();
    let _window = desktop.open_window();
    let shown = desktop.wait_for_color(X_COLOR);
    let corner = (
        i16::try_from(shown.left + 10).expect("small"),
        i16::try_from(shown.top + 10).expect("small"),
    );
    // Chromium's menus start out transparent: no black square.
    let _menu = desktop
        .x11
        .open_translucent_menu((corner, (60, 40)), [0xff, 0xff, 0xff, 0]);
    desktop.vaht.wait_for_logged("an X11 menu mapped", 1);
    let middle = (shown.left + 40, shown.top + 30);
    assert!(
        desktop.wait_for_color_at(middle, X_COLOR),
        "the window shows through the menu"
    );
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

/// The X window's size as rectangles: a ring 20 pixels wide around a hole.
const RING: [(i16, i16, u16, u16); 4] = [
    (0, 0, 200, 20),
    (0, 100, 200, 20),
    (0, 20, 20, 80),
    (180, 20, 20, 80),
];

#[test]
fn a_shaped_x11_window_shows_only_its_shape() {
    let mut desktop = XDesktop::new();
    let window = desktop.open_window();
    let shown = desktop.wait_for_color(X_COLOR);
    let middle = (shown.left + 100, shown.top + 60);
    let band = (shown.left + 5, shown.top + 60);
    desktop.x11.shape(window, &RING);
    assert!(
        desktop.wait_for_color_at(band, X_COLOR),
        "the band shows the window"
    );
    let screenshot = desktop.wayland.screenshot();
    assert_ne!(
        screenshot.rgb(middle.0, middle.1),
        X_COLOR,
        "the hole shows what is below"
    );
}

#[test]
fn a_shape_of_many_rectangles_is_clipped_exactly() {
    let mut desktop = XDesktop::new();
    let window = desktop.open_window();
    let shown = desktop.wait_for_color(X_COLOR);
    // A hundred one-pixel rows, every other row of the window: far more
    // rectangles than fluxland drew before giving up.
    let rows: Vec<(i16, i16, u16, u16)> = (0..100).map(|row| (0, row * 2, 200, 1)).collect();
    desktop.x11.shape(window, &rows);
    let shape_row = (shown.left + 50, shown.top + 40);
    let gap_row = (shown.left + 50, shown.top + 41);
    assert!(desktop.wait_for_color_at(shape_row, X_COLOR));
    let screenshot = desktop.wayland.screenshot();
    assert_ne!(
        screenshot.rgb(gap_row.0, gap_row.1),
        X_COLOR,
        "the rows between show through"
    );
    // Deep in the list, the 60th row shows and the gap above it does not.
    assert_eq!(screenshot.rgb(shown.left + 50, shown.top + 118), X_COLOR);
    assert_ne!(screenshot.rgb(shown.left + 50, shown.top + 117), X_COLOR);
}

#[test]
fn a_click_in_a_hole_goes_to_the_window_below() {
    let mut desktop = XDesktop::new();
    desktop.wayland.open_windows(1);
    let below = desktop.wait_for_color(WINDOW_COLOR);
    let window = desktop.open_window();
    let shown = desktop.wait_for_color(X_COLOR);
    desktop.x11.shape(window, &RING);
    assert!(desktop.wait_for_color_at((shown.left + 5, shown.top + 60), X_COLOR));
    // The hole's middle, over the window below.
    let hole = (shown.left + 100, shown.top + 60);
    assert!(
        hole.0 <= below.right && hole.1 <= below.bottom,
        "the windows overlap at the hole: {below:?} {shown:?}"
    );
    desktop.wayland.click(hole.0, hole.1, BUTTON_LEFT);
    let focused: Vec<_> = desktop
        .windows()
        .into_iter()
        .filter(|window| window.is_focused)
        .map(|window| window.app_id)
        .collect();
    assert_eq!(
        focused,
        ["vaht.test"],
        "the Wayland window below took the click"
    );
}

#[test]
fn an_x11_window_that_draws_its_own_frame_gets_none() {
    let mut desktop = XDesktop::new();
    let _window = desktop
        .x11
        .open_self_decorated_window(X_SIZE, X_COLOR, "frameless");
    desktop.vaht.wait_for_logged("placed first", 1);
    let shown = desktop.wait_for_color(X_COLOR);
    let screenshot = desktop.wayland.screenshot();
    let desktop_color = screenshot.rgb(2, 2);
    assert_eq!(
        screenshot.rgb(shown.left - 1, shown.top - 1),
        desktop_color,
        "no border at its corner"
    );
    assert_eq!(
        screenshot.rgb(shown.left + 50, shown.top - 5),
        desktop_color,
        "no titlebar above it"
    );
}

#[test]
fn an_x11_window_moving_itself_sees_the_pointer_where_it_is() {
    let mut desktop = XDesktop::new();
    let window = desktop.open_window();
    let shown = desktop.wait_for_color(X_COLOR);
    let grip = (shown.left + 20, shown.top + 20);
    let to_x = |value: u32| i16::try_from(value).expect("small");
    desktop.wayland.move_pointer_to(grip.0, grip.1);
    desktop.wayland.press(BUTTON_LEFT);
    // Its own title bar dragged: the program asks vaht to move it.
    let started = desktop.vaht.logged_count("drag started");
    desktop
        .x11
        .request_move(window, (to_x(grip.0), to_x(grip.1)));
    desktop.vaht.wait_for_logged("drag started", started + 1);
    let moved_to = (grip.0 + 150, grip.1 + 40);
    desktop.wayland.move_pointer_to(moved_to.0, moved_to.1);
    let corner = (to_x(shown.left + 150), to_x(shown.top + 40));
    assert_eq!(desktop.x11.wait_for_position(window, corner), corner);
    // Once the X server has the window where it went, it has the pointer
    // where it is too, so nothing that slid under the drag's start seems
    // hovered (fluxland's ROTK launcher issue). Without the fix it stays
    // at the drag's start; Xwayland takes vaht's motion a moment after
    // vaht sends it, so the test waits for it.
    let expected = (to_x(moved_to.0), to_x(moved_to.1));
    assert_eq!(
        desktop.x11.wait_for_pointer(expected),
        expected,
        "during the drag"
    );
    desktop.wayland.release(BUTTON_LEFT);
    assert_eq!(desktop.x11.wait_for_pointer(expected), expected, "after it");
}

#[test]
fn an_x11_window_asking_twice_to_be_moved_is_still_moved() {
    let mut desktop = XDesktop::new();
    let window = desktop.open_window();
    let shown = desktop.wait_for_color(X_COLOR);
    let grip = (shown.left + 20, shown.top + 20);
    let to_x = |value: u32| i16::try_from(value).expect("small");
    desktop.wayland.move_pointer_to(grip.0, grip.1);
    desktop.wayland.press(BUTTON_LEFT);
    // Steam asks twice for each drag of its title bar.
    let started = desktop.vaht.logged_count("drag started");
    for _ in 0..2 {
        desktop
            .x11
            .request_move(window, (to_x(grip.0), to_x(grip.1)));
    }
    desktop.vaht.wait_for_logged("drag started", started + 2);
    desktop.wayland.move_pointer_to(grip.0 + 150, grip.1 + 40);
    let corner = (to_x(shown.left + 150), to_x(shown.top + 40));
    assert_eq!(desktop.x11.wait_for_position(window, corner), corner);
    desktop.wayland.release(BUTTON_LEFT);
}

#[test]
fn an_x11_window_resized_by_vaht_keeps_the_new_size() {
    let mut desktop = XDesktop::new();
    let window = desktop.open_window();
    desktop.wait_for_color(X_COLOR);
    desktop.run(&["resize-to", "300", "200"]);
    assert_eq!(desktop.x11.wait_for_size(window, (300, 200)), (300, 200));
    // Focus moving to another window settles every window again: this
    // one keeps its size rather than flickering back to the old one.
    desktop.wayland.open_windows(1);
    desktop.vaht.wait_for_logged("placed first", 2);
    assert_eq!(desktop.x11.wait_for_size(window, (300, 200)), (300, 200));
}

#[test]
fn an_x11_window_shows_the_icon_it_sets_on_its_titlebar() {
    const ICON: [u8; 3] = [0xff, 0xff, 0x00];
    let mut desktop = XDesktop::new();
    let window = desktop.open_window();
    let content = desktop.wait_for_color(X_COLOR);
    desktop.x11.set_icon(window, 32, ICON);
    let metrics = vaht_test_support::BUILT_IN_METRICS;
    let point = (
        content.left + metrics.titlebar_height / 2,
        content.top - metrics.border_width - metrics.titlebar_height / 2,
    );
    assert!(
        desktop.wait_for_color_at(point, ICON),
        "the icon shows left of the title"
    );
}

#[test]
fn autostart_programs_find_xwayland_in_display() {
    let files = tempfile::tempdir().expect("a temporary directory");
    let seen = files.path().join("display");
    let settings = format!(
        "[session]\nautostart = [\"printf %s \\\"$DISPLAY\\\" > '{0}.part'; mv '{0}.part' '{0}'\"]\n",
        seen.display()
    );
    let vaht = HeadlessVaht::start_with_xwayland(
        Path::new(EXECUTABLE),
        &[("settings.toml", settings.as_bytes())],
    );
    let display = vaht_test_support::wait_for_file(&seen, |_| true);
    assert_eq!(
        display,
        vaht.x_display(),
        "an X program in the startup script finds X"
    );
}

#[test]
fn x_resources_named_in_the_settings_are_loaded_for_x_programs() {
    let vaht = HeadlessVaht::start_with_xwayland(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", b"[xwayland]\nresources = \"Xresources\"\n"),
            (
                "Xresources",
                b"XTerm*faceName: DejaVu Sans Mono\nXft.dpi: 96\n",
            ),
        ],
    );
    let x11 = X11Client::connect(&vaht.x_display());
    let resources = x11.resources();
    assert!(
        resources.contains("XTerm*faceName: DejaVu Sans Mono"),
        "{resources}"
    );
    assert!(resources.contains("Xft.dpi: 96"), "{resources}");
}

#[test]
fn an_x11_program_pastes_what_a_wayland_program_copied() {
    let mut desktop = XDesktop::new();
    let window = desktop.open_window();
    let _ = desktop.wait_for_color(X_COLOR);
    let mut manager = ClipboardManager::connect(desktop.vaht.socket());
    manager.copy(Selection::Clipboard, "from Wayland");
    manager.copy(Selection::Primary, "selected in Wayland");
    let x11 = &mut desktop.x11;
    let (clipboard, primary) = manager.answer_while(|| {
        (
            x11.paste(window, Selection::Clipboard),
            x11.paste(window, Selection::Primary),
        )
    });
    assert_eq!(clipboard.as_deref(), Some("from Wayland"));
    assert_eq!(primary.as_deref(), Some("selected in Wayland"));
}

#[test]
fn a_wayland_program_pastes_what_an_x11_program_copied() {
    let mut desktop = XDesktop::new();
    let window = desktop.open_window();
    let _ = desktop.wait_for_color(X_COLOR);
    let mut manager = ClipboardManager::connect(desktop.vaht.socket());
    desktop.x11.copy(window, Selection::Clipboard, "from X");
    let pasted = desktop
        .x11
        .answer_while(|| manager.wait_for_text(Selection::Clipboard, "from X"));
    assert!(pasted, "the X program's copy reached Wayland");
}

/// Starts vaht with Xwayland and input emulation `policy` (a TOML table
/// body), with an X window open and shown; returns the desktop and the
/// window.
fn emulating_desktop(policy: &str) -> (XDesktop, u32) {
    let settings =
        format!("[placement]\nstrategy = \"cascade\"\n[xwayland.input_emulation]\n{policy}\n");
    let vaht = HeadlessVaht::start_with_xwayland(
        Path::new(EXECUTABLE),
        &[("settings.toml", settings.as_bytes())],
    );
    let x11 = X11Client::connect(&vaht.x_display());
    let wayland = TestClient::connect(vaht.socket());
    let mut desktop = XDesktop { vaht, x11, wayland };
    let window = desktop.open_window();
    let _ = desktop.wait_for_color(X_COLOR);
    (desktop, window)
}

#[test]
fn an_x11_program_allowed_to_emulate_input_types_and_moves_the_pointer() {
    let (mut desktop, window) = emulating_desktop("default = \"allow\"");
    // 38 is "a".
    desktop.x11.emulate_key(38);
    assert!(
        desktop.x11.wait_for_key_press(window),
        "the key went through vaht to the focused window"
    );
    desktop.x11.emulate_motion(150, 90);
    assert_eq!(desktop.x11.wait_for_pointer((150, 90)), (150, 90));
}

#[test]
fn an_x11_program_refused_input_emulation_cannot_type() {
    let (mut desktop, window) = emulating_desktop("default = \"deny\"");
    desktop.x11.emulate_key(38);
    desktop.vaht.wait_for_logged("input emulation refused", 1);
    // A key typed for real comes after the refused one would have.
    desktop.wayland.type_text("b");
    assert!(desktop.x11.wait_for_key_press(window));
    assert_eq!(
        desktop.x11.key_codes_pressed(window),
        [56],
        "only the real \"b\", not the emulated \"a\""
    );
}

#[test]
fn allowing_input_emulation_from_now_on_is_a_setting_save_settings_keeps() {
    let (mut desktop, window) = emulating_desktop("default = \"ask\"");
    desktop.x11.emulate_key(38);
    desktop
        .vaht
        .wait_for_logged("asking whether a program may emulate input", 1);
    // Allow, Allow from now on, Deny, Deny from now on.
    desktop.wayland.type_chord(&[keys::DOWN]);
    desktop.wayland.type_chord(&[keys::DOWN]);
    desktop.wayland.type_chord(&[keys::ENTER]);
    desktop.vaht.wait_for_logged("is_allowed=true", 1);
    desktop.x11.emulate_key(38);
    assert!(desktop.x11.wait_for_key_press(window), "allowed now");

    let output = Command::new(EXECUTABLE)
        .args(["msg", "run", "save-settings"])
        .env(vaht_ipc::SOCKET_VARIABLE, desktop.vaht.ipc_socket())
        .output()
        .expect("vaht msg runs");
    assert!(output.status.success(), "{output:?}");
    let settings = std::fs::read_to_string(desktop.vaht.config_directory().join("settings.toml"))
        .expect("the settings are there");
    assert!(
        settings.contains("[xwayland.input_emulation]\ndefault = \"ask\"\nallow = ["),
        "{settings}"
    );
}
