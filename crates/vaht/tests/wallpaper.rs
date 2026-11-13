//! Wallpaper tests: the image the settings name under everything, and
//! pseudo-transparent windows showing only it through.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, TestClient, WINDOW_COLOR, solid_png};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The wallpaper's color.
const SKY: [u8; 3] = [0x30, 0xa0, 0x50];

/// The middle of the headless output, where the test windows overlap.
const MIDDLE: (u32, u32) = (960, 540);

/// Starts vaht with a solid wallpaper, tiled, and `rules`.
fn start(rules: &str) -> (HeadlessVaht, TestClient) {
    let picture = solid_png(16, SKY);
    let settings = "[wallpaper]\nimage = \"sky.png\"\nfit = \"tile\"\n";
    let vaht = HeadlessVaht::start_with_files(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", settings.as_bytes()),
            ("rules.toml", rules.as_bytes()),
            ("sky.png", &picture),
        ],
        &[],
    );
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

/// Rules that put every test window in the middle, so they overlap, and
/// `extra` keys.
fn centered(extra: &str) -> String {
    format!(
        "[[rule]]\nmatch = {{ app_id = \"vaht.test\" }}\nposition = {{ anchor = \"center\" }}\n{extra}"
    )
}

#[test]
fn the_wallpaper_covers_the_desktop() {
    let (_vaht, mut client) = start("");
    let screenshot = client.screenshot();
    assert_eq!(screenshot.rgb(5, 300), SKY);
    assert_eq!(screenshot.rgb(1900, 1000), SKY);
}

#[test]
fn a_clear_window_shows_the_window_below_it() {
    let (_vaht, mut client) = start(&centered(""));
    client.open_windows(2);
    client.clear_window(1);
    assert_eq!(client.screenshot().rgb(MIDDLE.0, MIDDLE.1), WINDOW_COLOR);
}

#[test]
fn a_pseudo_transparent_window_shows_only_the_wallpaper() {
    let (vaht, mut client) = start(&centered("pseudo_transparent = true\n"));
    client.open_windows(2);
    client.clear_window(1);
    assert_eq!(client.screenshot().rgb(MIDDLE.0, MIDDLE.1), SKY);
    client.close_windows();
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}
