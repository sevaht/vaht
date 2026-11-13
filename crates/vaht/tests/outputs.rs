//! Several outputs: each shows its own part of the layout, which runs
//! from the left in the order the backend finds them, and windows move
//! from one to the next.

use std::path::Path;

use vaht_test_support::{
    BUILT_IN_METRICS, BUTTON_LEFT, Bounds, HeadlessVaht, TestClient, TestLayer, WINDOW_COLOR, keys,
    solid_png,
};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Two outputs of different sizes, side by side.
const OUTPUTS: &str = "800x600,640x480";

/// The first output's width: where the second begins.
const FIRST_WIDTH: u32 = 800;

/// The wallpaper's color.
const SKY: [u8; 3] = [0x30, 0xa0, 0x50];

/// A layer surface's color.
const BACKDROP: [u8; 3] = [0xc0, 0x40, 0x90];

/// Starts vaht on [`OUTPUTS`], with a client.
fn two_outputs() -> (HeadlessVaht, TestClient) {
    with_settings("")
}

/// Starts vaht on [`OUTPUTS`] with a solid wallpaper and `settings`, with
/// a client.
fn with_settings(settings: &str) -> (HeadlessVaht, TestClient) {
    let picture = solid_png(16, SKY);
    let settings = format!("[wallpaper]\nimage = \"sky.png\"\nfit = \"tile\"\n{settings}");
    let vaht = HeadlessVaht::start_with_outputs(
        Path::new(EXECUTABLE),
        OUTPUTS,
        &[
            ("settings.toml", settings.as_bytes()),
            ("sky.png", &picture),
        ],
    );
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

#[test]
fn a_window_opens_on_the_output_the_pointer_is_on() {
    let (_vaht, mut client) = two_outputs();
    client.move_pointer_to(FIRST_WIDTH + 100, 100);
    client.open_windows(1);
    assert_eq!(
        client.screenshot_of("HEADLESS-1").bounds_of(WINDOW_COLOR),
        None
    );
    assert!(
        client
            .screenshot_of("HEADLESS-2")
            .bounds_of(WINDOW_COLOR)
            .is_some(),
        "the window opened on the second output"
    );
}

#[test]
fn a_window_maximizes_on_its_own_output() {
    let (_vaht, mut client) = two_outputs();
    client.move_pointer_to(FIRST_WIDTH + 100, 100);
    client.open_windows(1);
    client.type_chord(&[keys::LEFT_SUPER, keys::UP]);
    let second = client.screenshot_of("HEADLESS-2");
    // The taskbar is on the first output, so all of the second is usable.
    let whole = Bounds {
        left: 0,
        top: 0,
        right: second.width() - 1,
        bottom: second.height() - 1,
    };
    assert_eq!(
        second.bounds_of(WINDOW_COLOR),
        Some(BUILT_IN_METRICS.content_within(whole))
    );
    assert_eq!(
        client.screenshot_of("HEADLESS-1").bounds_of(WINDOW_COLOR),
        None
    );
}

#[test]
fn the_wallpaper_covers_every_output() {
    let (_vaht, mut client) = two_outputs();
    assert_eq!(client.screenshot_of("HEADLESS-1").rgb(5, 5), SKY);
    let second = client.screenshot_of("HEADLESS-2");
    assert_eq!(second.rgb(5, 5), SKY);
    assert_eq!(second.rgb(second.width() - 1, second.height() - 1), SKY);
}

#[test]
fn the_taskbar_shows_on_the_output_the_settings_name() {
    let (_vaht, mut client) = with_settings("[taskbar]\noutput = \"HEADLESS-2\"\n");
    let first = client.screenshot_of("HEADLESS-1");
    assert_eq!(first.rgb(5, first.height() - 2), SKY, "no taskbar here");
    let second = client.screenshot_of("HEADLESS-2");
    assert_ne!(
        second.rgb(5, second.height() - 2),
        SKY,
        "the taskbar is here"
    );
}

#[test]
fn a_layer_surface_opens_on_the_output_the_pointer_is_on() {
    let (_vaht, mut client) = two_outputs();
    client.move_pointer_to(FIRST_WIDTH + 100, 100);
    client.open_backdrop(TestLayer::Background, BACKDROP);
    assert_eq!(client.screenshot_of("HEADLESS-1").rgb(5, 5), SKY);
    assert_eq!(client.screenshot_of("HEADLESS-2").rgb(5, 5), BACKDROP);
}
