//! Several outputs: each shows its own part of the layout, which runs
//! from the left in the order the backend finds them, and windows move
//! from one to the next.

use std::path::Path;

use vaht_test_support::{BUTTON_LEFT, HeadlessVaht, TestClient, WINDOW_COLOR};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Two outputs of different sizes, side by side.
const OUTPUTS: &str = "800x600,640x480";

/// The first output's width: where the second begins.
const FIRST_WIDTH: u32 = 800;

/// Starts vaht on [`OUTPUTS`], with a client.
fn two_outputs() -> (HeadlessVaht, TestClient) {
    let vaht = HeadlessVaht::start_with_outputs(Path::new(EXECUTABLE), OUTPUTS, &[]);
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

#[test]
fn each_output_is_captured_at_its_own_size() {
    let (_vaht, mut client) = two_outputs();
    let first = client.screenshot_of("HEADLESS-1");
    let second = client.screenshot_of("HEADLESS-2");
    assert_eq!((first.width(), first.height()), (800, 600));
    assert_eq!((second.width(), second.height()), (640, 480));
}

#[test]
fn a_window_moved_past_the_first_output_shows_on_the_second() {
    let (_vaht, mut client) = two_outputs();
    client.open_windows(1);
    let content = client
        .screenshot_of("HEADLESS-1")
        .bounds_of(WINDOW_COLOR)
        .expect("the window opens on the first output");
    let middle = (
        u32::midpoint(content.left, content.right),
        u32::midpoint(content.top, content.bottom),
    );
    client.hold_alt();
    client.drag(middle, (middle.0 + FIRST_WIDTH, middle.1), BUTTON_LEFT);
    client.release_modifiers();
    assert_eq!(
        client.screenshot_of("HEADLESS-1").bounds_of(WINDOW_COLOR),
        None,
        "the window left the first output"
    );
    // The second output's left edge is the layout's x = 800, so the window
    // is as far into it as it was into the first.
    assert_eq!(
        client.screenshot_of("HEADLESS-2").bounds_of(WINDOW_COLOR),
        Some(content)
    );
}
