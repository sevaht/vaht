//! Blur tests: programs asking for what is behind them to be blurred
//! (`ext-background-effect-v1`). The headless renderer cannot read back
//! what it drew, so it draws no blur; these check what vaht tells
//! programs, and that asking changes nothing else.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, TestClient, WINDOW_COLOR};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Starts vaht with `settings`, and a client.
fn start(settings: &str) -> (HeadlessVaht, TestClient) {
    let vaht =
        HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("settings.toml", settings)]);
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

#[test]
fn vaht_says_it_blurs_unless_blur_is_off() {
    let (_vaht, mut client) = start("");
    assert!(client.blurs(), "on by default");
    let (_off_vaht, mut off_client) = start("[blur]\nenabled = false\n");
    assert!(!off_client.blurs());
}

#[test]
fn a_window_asking_for_blur_still_shows() {
    let (_vaht, mut client) = start("");
    client.open_windows(1);
    let before = client.screenshot().bounds_of(WINDOW_COLOR);
    client.blur_behind_window(0);
    assert_eq!(client.screenshot().bounds_of(WINDOW_COLOR), before);
}
