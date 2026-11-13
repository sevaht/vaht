//! Settings tests: `settings.toml` changing how vaht places, frames, focuses
//! and starts, checked by screenshots and by the files programs write.

use std::path::Path;
use std::time::{Duration, Instant};

use vaht_test_support::{
    BUTTON_LEFT, Bounds, HeadlessVaht, Screenshot, TestClient, WINDOW_COLOR, WINDOW_SIZE,
};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The built-in theme's titlebar height, in pixels.
const TITLEBAR_HEIGHT: u32 = 20;

/// The built-in theme's border width, in pixels.
const BORDER_WIDTH: u32 = 1;

/// The built-in taskbar's height, in pixels; it runs along the bottom.
const TASKBAR_HEIGHT: u32 = 20;

/// How long to wait for a program vaht started to leave its mark.
const PATIENCE: Duration = Duration::from_secs(10);

/// Starts vaht with `settings` as its `settings.toml`, and a client.
fn start(settings: &str) -> (HeadlessVaht, TestClient) {
    let vaht =
        HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("settings.toml", settings)]);
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

/// Returns the content areas of the windows in `screenshot`, left to
/// right, found as runs of the window color along the row `y`.
fn windows_along(screenshot: &Screenshot, y: u32) -> Vec<(u32, u32)> {
    let mut runs = Vec::new();
    let mut start = None;
    for x in 0..screenshot.width() {
        match (screenshot.rgb(x, y) == WINDOW_COLOR, start) {
            (true, None) => start = Some(x),
            (false, Some(first)) => {
                runs.push((first, x - 1));
                start = None;
            }
            (true, Some(_)) | (false, None) => {}
        }
    }
    if let Some(first) = start {
        runs.push((first, screenshot.width() - 1));
    }
    runs
}

/// Returns the middle of the titlebar above `content`, at `x`.
const fn titlebar_at(content: Bounds, x: u32) -> (u32, u32) {
    (x, content.top - BORDER_WIDTH - TITLEBAR_HEIGHT / 2)
}

#[test]
fn row_placement_puts_windows_side_by_side_from_the_corner() {
    let (_vaht, mut client) = start("");
    client.open_windows(2);
    let screenshot = client.screenshot();
    let content = screenshot
        .bounds_of(WINDOW_COLOR)
        .expect("windows are shown");
    assert_eq!(
        (content.left, content.top),
        (BORDER_WIDTH, 2 * BORDER_WIDTH + TITLEBAR_HEIGHT),
        "the first window's frame is in the corner"
    );
    let frame_width = WINDOW_SIZE.0 + 2 * BORDER_WIDTH;
    assert_eq!(
        windows_along(&screenshot, content.top),
        [
            (BORDER_WIDTH, WINDOW_SIZE.0),
            (frame_width + BORDER_WIDTH, frame_width + WINDOW_SIZE.0),
        ],
        "the second window's frame starts where the first's ends"
    );
}

#[test]
fn the_titlebar_has_the_buttons_settings_name() {
    let (_vaht, mut client) = start(
        "[placement]\nstrategy = \"center\"\n\
         [windows.frame]\ntitlebar_left = [\"maximize\"]\ntitlebar_right = []\n",
    );
    client.open_windows(1);
    let content = client.screenshot().bounds_of(WINDOW_COLOR).expect("shown");
    let (x, y) = titlebar_at(content, content.left + TITLEBAR_HEIGHT / 2);
    client.click(x, y, BUTTON_LEFT);
    let maximized = client.screenshot();
    assert_eq!(
        maximized.bounds_of(WINDOW_COLOR),
        Some(Bounds {
            left: BORDER_WIDTH,
            top: 2 * BORDER_WIDTH + TITLEBAR_HEIGHT,
            right: maximized.width() - 1 - BORDER_WIDTH,
            bottom: maximized.height() - 1 - TASKBAR_HEIGHT - BORDER_WIDTH,
        }),
        "the left square is the maximize button"
    );
}

#[test]
fn autostart_runs_its_commands_once_vaht_starts() {
    let files = tempfile::tempdir().expect("a temporary directory");
    let marker = files.path().join("started");
    let (vaht, _client) = start(&format!(
        "[session]\nautostart = [\"echo started > '{}'\"]\n",
        marker.display()
    ));
    let deadline = Instant::now() + PATIENCE;
    while !std::fs::read_to_string(&marker).is_ok_and(|text| text.trim() == "started") {
        assert!(Instant::now() < deadline, "the autostart command never ran");
        std::thread::sleep(Duration::from_millis(20));
    }
    vaht.wait_for_logged("program exited", 1);
}

#[test]
fn with_sloppy_focus_the_window_under_the_pointer_takes_focus() {
    let (_vaht, mut client) = start("[focus]\nmodel = \"sloppy\"\n");
    client.open_windows(2);
    let screenshot = client.screenshot();
    let content = screenshot.bounds_of(WINDOW_COLOR).expect("shown");
    let [first, second] = windows_along(&screenshot, content.top)[..] else {
        panic!("two windows side by side");
    };
    let first_title = titlebar_at(content, first.0 + 10);
    let second_title = titlebar_at(content, second.0 + 10);
    let title_color = |screenshot: &Screenshot, (x, y): (u32, u32)| screenshot.rgb(x, y);
    // The newer window has focus: its titlebar is drawn as focused.
    let focused = title_color(&screenshot, second_title);
    assert_ne!(title_color(&screenshot, first_title), focused);

    client.move_pointer_to(first.0 + 20, content.top + 20);
    let after = client.screenshot();
    assert_eq!(
        title_color(&after, first_title),
        focused,
        "the first has focus now"
    );
    assert_ne!(title_color(&after, second_title), focused);
}

#[test]
fn a_window_moved_near_the_output_edge_snaps_to_it() {
    let (_vaht, mut client) = start("[placement]\nstrategy = \"center\"\n");
    client.open_windows(1);
    let content = client.screenshot().bounds_of(WINDOW_COLOR).expect("shown");
    let grip = titlebar_at(content, content.left + 30);
    // Drag the frame's left edge to 6 pixels from the output's: within the
    // default 10 pixels, so it lands on the edge.
    let frame_left = content.left - BORDER_WIDTH;
    client.drag(grip, (grip.0 - frame_left + 6, grip.1), BUTTON_LEFT);
    let moved = client.screenshot().bounds_of(WINDOW_COLOR).expect("shown");
    assert_eq!(
        moved.left, BORDER_WIDTH,
        "the frame's left edge is the output's"
    );
}
