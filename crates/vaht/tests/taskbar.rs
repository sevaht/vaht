//! Taskbar tests: its window list, its buttons, and the room it keeps,
//! checked by screenshots and clicks. A theme of solid colors tells its
//! parts apart.

use std::path::Path;

use vaht_test_support::{
    BUILT_IN_METRICS, BUTTON_LEFT, Bounds, HeadlessVaht, Screenshot, TestClient, WINDOW_COLOR,
    solid_png,
};

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
        maximized.height() - 1 - BUILT_IN_METRICS.below_content(),
        "only the frame's bottom below it"
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

/// The test client's icon, as its desktop entry names it.
const ICON: [u8; 3] = [0xff, 0x00, 0xff];

/// The icon a window names itself.
const NAMED_ICON: [u8; 3] = [0x00, 0xff, 0xff];

/// Starts vaht with the test theme, a window list, `settings` added, and an
/// icon for the test client's app id; opens a window.
fn start_with_icon(settings: &str) -> (HeadlessVaht, TestClient) {
    let settings = format!("theme = \"test\"\n[taskbar]\nwidgets = [\"window-list\"]\n{settings}");
    let png = solid_png(32, ICON);
    let named_png = solid_png(32, NAMED_ICON);
    let vaht = HeadlessVaht::start_with_files(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", settings.as_bytes()),
            ("themes/test.toml", THEME.as_bytes()),
        ],
        &[
            (
                "applications/vaht.test.desktop",
                b"[Desktop Entry]\nName=Test\nIcon=vaht-test-icon\n",
            ),
            ("icons/hicolor/32x32/apps/vaht-test-icon.png", &png),
            ("icons/hicolor/32x32/apps/vaht-named-icon.png", &named_png),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    (vaht, client)
}

#[test]
fn an_entry_shows_its_application_s_icon_left_of_its_title() {
    let (_vaht, mut client) = start_with_icon("");
    let screenshot = client.screenshot();
    let middle = screenshot.height() - HEIGHT / 2;
    assert_eq!(screenshot.rgb(0, middle), FOCUSED, "the entry's edge");
    assert_eq!(screenshot.rgb(1, middle), ICON, "the icon, a pixel in");
    assert_eq!(
        screenshot.rgb(HEIGHT - 2, middle),
        ICON,
        "as tall as the bar, less a pixel each side"
    );
    assert_eq!(
        screenshot.rgb(HEIGHT, middle),
        FOCUSED,
        "then the entry again"
    );
    assert_eq!(
        screenshot.rgb(9, screenshot.height() - HEIGHT),
        FOCUSED,
        "a pixel above it"
    );
}

#[test]
fn entries_can_leave_icons_out() {
    let (_vaht, mut client) = start_with_icon("[taskbar.window_list]\nicons = false\n");
    let screenshot = client.screenshot();
    assert_eq!(screenshot.rgb(9, screenshot.height() - HEIGHT / 2), FOCUSED);
}

#[test]
fn the_icon_a_window_names_comes_before_its_application_s() {
    let (_vaht, mut client) = start_with_icon("");
    client.name_window_icons("vaht-named-icon");
    let screenshot = client.screenshot();
    assert_eq!(
        screenshot.rgb(9, screenshot.height() - HEIGHT / 2),
        NAMED_ICON
    );
}

#[test]
fn an_auto_hiding_taskbar_shows_at_its_edge_and_hides_when_left() {
    let (_vaht, mut client) = start("[\"window-list\"]\nauto_hide = true\nauto_hide_delay_ms = 50");
    let bottom_middle = |screenshot: &Screenshot| {
        screenshot.rgb(screenshot.width() / 2, screenshot.height() - HEIGHT / 2)
    };
    let hidden = client.screenshot();
    assert_ne!(bottom_middle(&hidden), BACKGROUND, "hidden at first");
    let (width, height) = (hidden.width(), hidden.height());
    client.move_pointer_to(width / 2, height - 1);
    assert_eq!(
        bottom_middle(&client.screenshot()),
        BACKGROUND,
        "shown at the edge"
    );
    client.move_pointer_to(width / 2, height / 2);
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_ne!(
        bottom_middle(&client.screenshot()),
        BACKGROUND,
        "hidden again once the pointer left"
    );
}

#[test]
fn a_narrower_taskbar_sits_where_it_is_aligned() {
    let (_vaht, mut client) = start("[\"window-list\"]\nwidth_percent = 50\nalign = \"right\"");
    let screenshot = client.screenshot();
    let row = screenshot.height() - HEIGHT / 2;
    let half = screenshot.width() / 2;
    assert_ne!(
        screenshot.rgb(half - 1, row),
        BACKGROUND,
        "the left half is free"
    );
    assert_eq!(
        screenshot.rgb(half, row),
        BACKGROUND,
        "the bar starts halfway"
    );
    assert_eq!(screenshot.rgb(screenshot.width() - 1, row), BACKGROUND);
}
