//! Layer shell tests: wallpapers, panels and overlays placed and stacked
//! around the windows, checked by screenshots.

use std::path::Path;
use std::time::{Duration, Instant};

use vaht_test_support::{
    BUTTON_LEFT, HeadlessVaht, KeyboardFocus, TestClient, TestLayer, WINDOW_COLOR, keys,
};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The built-in theme's border width, in pixels.
const BORDER_WIDTH: u32 = 1;

/// A panel's color.
const PANEL_COLOR: [u8; 3] = [0xc0, 0x30, 0x30];

/// A wallpaper's color.
const WALLPAPER_COLOR: [u8; 3] = [0x30, 0xa0, 0x40];

/// A panel's height, in pixels.
const PANEL_HEIGHT: u32 = 30;

/// How long to wait for a program vaht started to leave its mark.
const PATIENCE: Duration = Duration::from_secs(10);

/// Starts vaht with `files` as its configuration, and a client.
fn start(files: &[(&str, &str)]) -> (HeadlessVaht, TestClient) {
    let vaht = HeadlessVaht::start_with_config(Path::new(EXECUTABLE), files);
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

#[test]
fn a_maximized_window_leaves_an_exclusive_panel_visible() {
    let (_vaht, mut client) = start(&[]);
    client.open_bar(TestLayer::Top, PANEL_HEIGHT, true, PANEL_COLOR);
    client.open_windows(1);
    client.type_chord(&[keys::LEFT_SUPER, keys::UP]);
    let screenshot = client.screenshot();
    let panel = screenshot
        .bounds_of(PANEL_COLOR)
        .expect("the panel is shown");
    assert_eq!(
        (panel.top, panel.bottom),
        (screenshot.height() - PANEL_HEIGHT, screenshot.height() - 1),
        "across the bottom"
    );
    let content = screenshot
        .bounds_of(WINDOW_COLOR)
        .expect("the window is shown");
    assert_eq!(
        content.bottom,
        panel.top - 1 - BORDER_WIDTH,
        "the window's frame ends where the panel begins"
    );
}

#[test]
fn a_wallpaper_is_below_windows_and_an_overlay_above_them() {
    let (_vaht, mut client) = start(&[]);
    client.open_backdrop(TestLayer::Background, WALLPAPER_COLOR);
    client.open_windows(1);
    client.type_chord(&[keys::LEFT_SUPER, keys::UP]);
    let maximized = client.screenshot();
    let bottom = maximized.height() - 1;
    assert_eq!(
        maximized.rgb(maximized.width() / 2, bottom - BORDER_WIDTH - 1),
        WINDOW_COLOR,
        "the window covers the wallpaper"
    );
    client.open_bar(TestLayer::Overlay, PANEL_HEIGHT, false, PANEL_COLOR);
    let covered = client.screenshot();
    assert_eq!(
        covered.rgb(covered.width() / 2, bottom - BORDER_WIDTH - 1),
        PANEL_COLOR,
        "the overlay covers the window, which it does not push aside"
    );
    client.type_chord(&[keys::LEFT_SUPER, keys::DOWN]);
    // The window floats in the top-left corner again; the right edge's
    // middle is clear of it and of the overlay.
    let restored = client.screenshot();
    assert_eq!(
        restored.rgb(restored.width() - 1, restored.height() / 2),
        WALLPAPER_COLOR,
        "the wallpaper shows where no window is"
    );
}

#[test]
fn desktop_bindings_work_over_a_wallpaper() {
    let files = tempfile::tempdir().expect("a temporary directory");
    let marker = files.path().join("clicked");
    let bindings = format!(
        "[mouse.desktop]\n\"Left\" = \"exec touch '{}'\"\n",
        marker.display()
    );
    let (_vaht, mut client) = start(&[("bindings.toml", &bindings)]);
    client.open_backdrop(TestLayer::Background, WALLPAPER_COLOR);
    client.click(5, 5, BUTTON_LEFT);
    let deadline = Instant::now() + PATIENCE;
    while !marker.exists() {
        assert!(Instant::now() < deadline, "the desktop binding never ran");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_panel_clicked_for_the_keyboard_keeps_it_when_it_redraws() {
    let (_vaht, mut client) = start(&[]);
    client.open_windows(1);
    assert_eq!(client.keyboard_focus(), KeyboardFocus::Window(0));
    client.open_keyboard_bar(TestLayer::Top, PANEL_HEIGHT, PANEL_COLOR);
    let screenshot = client.screenshot();
    let panel = screenshot
        .bounds_of(PANEL_COLOR)
        .expect("the panel is shown");
    client.click(screenshot.width() / 2, panel.top + 2, BUTTON_LEFT);
    assert_eq!(
        client.keyboard_focus(),
        KeyboardFocus::Layer(0),
        "a click gives it the keyboard"
    );
    client.commit_layers();
    assert_eq!(
        client.keyboard_focus(),
        KeyboardFocus::Layer(0),
        "and redrawing keeps it"
    );
}
