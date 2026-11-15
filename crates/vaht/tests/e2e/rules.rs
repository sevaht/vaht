//! Rules tests: `rules.toml` placing particular windows when they open,
//! checked by screenshots. The test client's windows have the app id
//! `vaht.test` and the titles `test window 1`, `test window 2`, and so on,
//! in the order they open.

use std::path::Path;

use vaht_test_support::{
    BUILT_IN_METRICS, HeadlessVaht, TestClient, WINDOW_COLOR, WINDOW_SIZE, keys,
};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Starts vaht with `rules` as its `rules.toml`, and a client.
fn start(rules: &str) -> (HeadlessVaht, TestClient) {
    let vaht = HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("rules.toml", rules)]);
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

#[test]
fn a_position_rule_centers_the_frame() {
    let (_vaht, mut client) =
        start("[[rule]]\nmatch = { app_id = \"vaht.test\" }\nposition = { anchor = \"center\" }\n");
    client.open_windows(1);
    let screenshot = client.screenshot();
    let content = screenshot.bounds_of(WINDOW_COLOR).expect("shown");
    let frame_width = WINDOW_SIZE.0 + 2 * BUILT_IN_METRICS.border_width;
    let frame_height =
        WINDOW_SIZE.1 + BUILT_IN_METRICS.above_content() + BUILT_IN_METRICS.below_content();
    let frame_left = content.left - BUILT_IN_METRICS.border_width;
    let frame_top =
        content.top - 2 * BUILT_IN_METRICS.border_width - BUILT_IN_METRICS.titlebar_height;
    assert_eq!(
        (frame_left, frame_top),
        (
            (screenshot.width() - frame_width) / 2,
            (screenshot.height() - BUILT_IN_METRICS.taskbar_height - frame_height) / 2
        ),
        "centered in what the taskbar leaves"
    );
}

#[test]
fn a_size_rule_sizes_the_frame() {
    let frame_width = 400 + 2 * BUILT_IN_METRICS.border_width;
    let frame_height = 300 + BUILT_IN_METRICS.above_content() + BUILT_IN_METRICS.below_content();
    let (_vaht, mut client) = start(&format!(
        "[[rule]]\nmatch = {{ title = \"test window 1\" }}\nsize = [{frame_width}, {frame_height}]\n"
    ));
    client.open_windows(1);
    let content = client.screenshot().bounds_of(WINDOW_COLOR).expect("shown");
    assert_eq!(
        (
            content.right - content.left + 1,
            content.bottom - content.top + 1
        ),
        (400, 300),
        "the content is the frame less its lines, titlebar and handle"
    );
}

#[test]
fn a_workspace_rule_opens_the_window_there() {
    let (_vaht, mut client) =
        start("[[rule]]\nmatch = { title = { regex = \"test window [0-9]+\" } }\nworkspace = 2\n");
    client.open_windows(1);
    assert_eq!(
        client.screenshot().bounds_of(WINDOW_COLOR),
        None,
        "not on workspace 1"
    );
    client.type_chord(&[keys::LEFT_CONTROL, keys::F2]);
    assert!(
        client.screenshot().bounds_of(WINDOW_COLOR).is_some(),
        "on workspace 2"
    );
}
