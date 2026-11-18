//! Settings tests: `settings.toml` changing how vaht places, frames, focuses
//! and starts, checked by screenshots and by the files programs write.

use std::path::Path;

use vaht_test_support::{
    BUILT_IN_METRICS, BUTTON_LEFT, Bounds, HeadlessVaht, Screenshot, TestClient, WINDOW_COLOR,
    WINDOW_SIZE, wait_for_file,
};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

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
    (
        x,
        content.top - BUILT_IN_METRICS.border_width - BUILT_IN_METRICS.titlebar_height / 2,
    )
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
        (
            BUILT_IN_METRICS.border_width,
            2 * BUILT_IN_METRICS.border_width + BUILT_IN_METRICS.titlebar_height
        ),
        "the first window's frame is in the corner"
    );
    let frame_width = WINDOW_SIZE.0 + 2 * BUILT_IN_METRICS.border_width;
    assert_eq!(
        windows_along(&screenshot, content.top),
        [
            (BUILT_IN_METRICS.border_width, WINDOW_SIZE.0),
            (
                frame_width + BUILT_IN_METRICS.border_width,
                frame_width + WINDOW_SIZE.0
            ),
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
    let (x, y) = titlebar_at(content, content.left + BUILT_IN_METRICS.titlebar_height / 2);
    client.click(x, y, BUTTON_LEFT);
    let maximized = client.screenshot();
    assert_eq!(
        maximized.bounds_of(WINDOW_COLOR),
        Some(Bounds {
            left: BUILT_IN_METRICS.border_width,
            top: 2 * BUILT_IN_METRICS.border_width + BUILT_IN_METRICS.titlebar_height,
            right: maximized.width() - 1 - BUILT_IN_METRICS.border_width,
            bottom: maximized.height()
                - 1
                - BUILT_IN_METRICS.taskbar_height
                - BUILT_IN_METRICS.below_content(),
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
    wait_for_file(&marker, |text| text.trim() == "started");
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
    let frame_left = content.left - BUILT_IN_METRICS.border_width;
    client.drag(grip, (grip.0 - frame_left + 6, grip.1), BUTTON_LEFT);
    let moved = client.screenshot().bounds_of(WINDOW_COLOR).expect("shown");
    assert_eq!(
        moved.left, BUILT_IN_METRICS.border_width,
        "the frame's left edge is the output's"
    );
}

#[test]
fn with_strict_sloppy_focus_a_window_moved_under_the_pointer_does_not_take_focus() {
    let focused_after_moving_within = |model: &str| -> u64 {
        let (vaht, mut client) = start(&format!("[focus]\nmodel = \"{model}\"\n"));
        client.open_windows(2);
        let screenshot = client.screenshot();
        let content = screenshot.bounds_of(WINDOW_COLOR).expect("shown");
        let [first, _] = windows_along(&screenshot, content.top)[..] else {
            panic!("two windows side by side");
        };
        client.move_pointer_to(first.0 + 20, content.top + 20);
        let windows = |vaht: &HeadlessVaht| -> Vec<vaht_ipc::WindowInfo> {
            let output = std::process::Command::new(EXECUTABLE)
                .args(["msg", "--json", "windows"])
                .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
                .output()
                .expect("vaht msg runs");
            match serde_json::from_slice(&output.stdout).expect("a reply") {
                vaht_ipc::Reply::Windows { windows } => windows,
                other @ (vaht_ipc::Reply::Version { .. }
                | vaht_ipc::Reply::Done { .. }
                | vaht_ipc::Reply::Workspaces { .. }
                | vaht_ipc::Reply::Outputs { .. }
                | vaht_ipc::Reply::LogFilter { .. }
                | vaht_ipc::Reply::Picked { .. }
                | vaht_ipc::Reply::Error { .. }) => panic!("not windows: {other:?}"),
            }
        };
        let listed = windows(&vaht);
        let entered = listed
            .iter()
            .find(|window| window.is_focused)
            .expect("one")
            .id;
        let other = listed
            .iter()
            .find(|window| window.id != entered)
            .expect("two")
            .id;
        // The other window comes over the pointer, without it moving.
        for command in ["tile 0 0 500 1000", "raise"] {
            let output = std::process::Command::new(EXECUTABLE)
                .args(["msg", "run", "--window", &other.to_string(), command])
                .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
                .output()
                .expect("vaht msg runs");
            assert!(output.status.success(), "{output:?}");
        }
        client.move_pointer_to(first.0 + 22, content.top + 22);
        let focused = windows(&vaht)
            .iter()
            .find(|window| window.is_focused)
            .expect("one")
            .id;
        assert_ne!(focused, 0);
        if focused == entered { 1 } else { 2 }
    };
    assert_eq!(
        focused_after_moving_within("sloppy"),
        2,
        "sloppy: the one under it"
    );
    assert_eq!(
        focused_after_moving_within("strict-sloppy"),
        1,
        "strict: the one the pointer entered"
    );
}
