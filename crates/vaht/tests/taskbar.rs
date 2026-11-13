//! Taskbar tests: its window list, its buttons, and the room it keeps,
//! checked by screenshots and clicks. A theme of solid colors tells its
//! parts apart.

use std::path::Path;

use vaht_test_support::{BUTTON_LEFT, Bounds, HeadlessVaht, Screenshot, TestClient, WINDOW_COLOR};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The taskbar's height in the test theme, in pixels.
const HEIGHT: u32 = 20;

/// The most an entry is wide, by default, in pixels.
const ENTRY_WIDTH: u32 = 128;

/// The focused window's entry.
const FOCUSED: [u8; 3] = [0xff, 0x00, 0x00];

/// Another window's entry.
const UNFOCUSED: [u8; 3] = [0x00, 0x00, 0xff];

/// The taskbar where nothing else is.
const BACKGROUND: [u8; 3] = [0x00, 0xff, 0x00];

/// A theme whose taskbar parts are solid colors, text in the color it is
/// on, so it never shows.
const THEME: &str = "\
[default]
text_effect = \"none\"
[taskbar]
height = 20
texture = { solid = \"#00ff00\" }
[taskbar.window-list.empty]
texture = { solid = \"#00ff00\" }
[taskbar.window-list.entry.focused]
texture = { solid = \"#ff0000\" }
text_color = \"#ff0000\"
[taskbar.window-list.entry.unfocused]
texture = { solid = \"#0000ff\" }
text_color = \"#0000ff\"
";

/// Starts vaht with the test theme and a taskbar of `widgets`, and a
/// client.
fn start(widgets: &str) -> (HeadlessVaht, TestClient) {
    let settings = format!("theme = \"test\"\n[taskbar]\nwidgets = {widgets}\n");
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", &settings), ("themes/test.toml", THEME)],
    );
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

/// Returns the runs of color along the taskbar's middle row, left to right:
/// each color and how many pixels it covers.
fn taskbar_runs(screenshot: &Screenshot) -> Vec<([u8; 3], u32)> {
    let y = screenshot.height() - HEIGHT / 2;
    let mut runs: Vec<([u8; 3], u32)> = Vec::new();
    for x in 0..screenshot.width() {
        let color = screenshot.rgb(x, y);
        match runs.last_mut() {
            Some((last, length)) if *last == color => *length += 1,
            _ => runs.push((color, 1)),
        }
    }
    runs
}

#[test]
fn each_window_has_an_entry_and_the_focused_ones_stands_out() {
    let (_vaht, mut client) = start("[\"window-list\"]");
    client.open_windows(2);
    let runs = taskbar_runs(&client.screenshot());
    assert_eq!(
        runs.iter().take(2).copied().collect::<Vec<_>>(),
        [(UNFOCUSED, ENTRY_WIDTH), (FOCUSED, ENTRY_WIDTH)],
        "in the order they opened, the second focused"
    );
    assert_eq!(runs.get(2).map(|(color, _)| *color), Some(BACKGROUND));
}

#[test]
fn clicking_an_entry_minimizes_its_focused_window_and_again_restores_it() {
    let (_vaht, mut client) = start("[\"window-list\"]");
    client.open_windows(1);
    let screenshot = client.screenshot();
    let y = screenshot.height() - HEIGHT / 2;
    client.click(ENTRY_WIDTH / 2, y, BUTTON_LEFT);
    let minimized = client.screenshot();
    assert_eq!(minimized.bounds_of(WINDOW_COLOR), None, "minimized");
    assert_eq!(
        taskbar_runs(&minimized).first().map(|(color, _)| *color),
        Some(UNFOCUSED),
        "its entry stays, unfocused"
    );
    client.click(ENTRY_WIDTH / 2, y, BUTTON_LEFT);
    let restored = client.screenshot();
    assert!(restored.bounds_of(WINDOW_COLOR).is_some(), "shown again");
    assert_eq!(
        taskbar_runs(&restored).first().map(|(color, _)| *color),
        Some(FOCUSED)
    );
}

#[test]
fn the_next_workspace_button_shows_the_next_workspace() {
    let (_vaht, mut client) = start("[\"workspace-next\", \"window-list\"]");
    client.open_windows(1);
    let screenshot = client.screenshot();
    client.click(HEIGHT / 2, screenshot.height() - HEIGHT / 2, BUTTON_LEFT);
    let next = client.screenshot();
    assert_eq!(
        next.bounds_of(WINDOW_COLOR),
        None,
        "the window stays on workspace 1"
    );
    assert!(
        !taskbar_runs(&next)
            .iter()
            .any(|(color, _)| *color == FOCUSED || *color == UNFOCUSED),
        "and the list shows workspace 2's windows: none"
    );
}

#[test]
fn without_a_taskbar_windows_maximize_over_the_whole_output() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", "[taskbar]\nenabled = false\n")],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    client.type_chord(&[
        vaht_test_support::keys::LEFT_SUPER,
        vaht_test_support::keys::UP,
    ]);
    let maximized = client.screenshot();
    let content = maximized.bounds_of(WINDOW_COLOR).expect("shown");
    assert_eq!(
        content.bottom,
        maximized.height() - 2,
        "only the border below it"
    );
}

#[test]
fn a_clock_showing_seconds_ticks_each_second() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[(
            "settings.toml",
            "[taskbar]\nwidgets = [\"clock\"]\n[taskbar.clock]\nformat = \"%H:%M:%S\"\n",
        )],
    );
    let mut client = TestClient::connect(vaht.socket());
    let before = client.screenshot();
    let taskbar = Bounds {
        left: 0,
        top: before.height() - HEIGHT,
        right: before.width() - 1,
        bottom: before.height() - 1,
    };
    let mut ticks = 0;
    let mut last = before;
    // Three ticks: each re-arms the clock from the tick before it.
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        let now = client.screenshot();
        if now.differs_within(&last, taskbar) {
            ticks += 1;
        }
        last = now;
    }
    assert_eq!(ticks, 3, "the clock's text changed every second");
}
