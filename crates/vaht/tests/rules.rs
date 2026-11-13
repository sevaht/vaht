//! Rules tests: `rules.toml` placing particular windows when they open,
//! checked by screenshots. The test client's windows have the app id
//! `vaht.test` and the titles `test window 1`, `test window 2`, and so on,
//! in the order they open.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, TestClient, WINDOW_COLOR, WINDOW_SIZE, keys};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The built-in theme's titlebar height, in pixels.
const TITLEBAR_HEIGHT: u32 = 20;

/// The built-in theme's border width, in pixels.
const BORDER_WIDTH: u32 = 1;

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
    let frame_width = WINDOW_SIZE.0 + 2 * BORDER_WIDTH;
    let frame_height = WINDOW_SIZE.1 + TITLEBAR_HEIGHT + 3 * BORDER_WIDTH;
    let frame_left = content.left - BORDER_WIDTH;
    let frame_top = content.top - 2 * BORDER_WIDTH - TITLEBAR_HEIGHT;
    assert_eq!(
        (frame_left, frame_top),
        (
            (screenshot.width() - frame_width) / 2,
            (screenshot.height() - frame_height) / 2
        )
    );
}

#[test]
fn a_size_rule_sizes_the_frame() {
    let (_vaht, mut client) =
        start("[[rule]]\nmatch = { title = \"test window 1\" }\nsize = [402, 323]\n");
    client.open_windows(1);
    let content = client.screenshot().bounds_of(WINDOW_COLOR).expect("shown");
    assert_eq!(
        (
            content.right - content.left + 1,
            content.bottom - content.top + 1
        ),
        (400, 300),
        "the content is the frame less its borders and titlebar"
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
