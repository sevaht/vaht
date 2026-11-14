//! Session lock tests: a locker (swaylock) hides the session until it
//! unlocks, and has the keys meanwhile; if it dies, the session stays
//! locked.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, TestClient, WINDOW_COLOR, keys};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The locker's color.
const LOCK_COLOR: [u8; 3] = [0x30, 0x30, 0x90];

/// Starts vaht and a client with one window; returns the window's
/// content.
fn desktop() -> (HeadlessVaht, TestClient, vaht_test_support::Bounds) {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let content = client
        .screenshot()
        .bounds_of(WINDOW_COLOR)
        .expect("the window shows");
    (vaht, client, content)
}

#[test]
fn a_locked_session_shows_only_the_locker() {
    let (_vaht, mut client, content) = desktop();
    assert!(client.lock_session(LOCK_COLOR), "vaht says it is locked");
    let shown = client.screenshot();
    assert_eq!(shown.bounds_of(WINDOW_COLOR), None, "the window is hidden");
    assert_eq!(shown.rgb(content.left, content.top), LOCK_COLOR);
    assert_eq!(shown.rgb(5, 5), LOCK_COLOR, "the locker covers the output");
}

#[test]
fn keys_go_to_the_locker_and_bindings_rest() {
    let (_vaht, mut client, content) = desktop();
    assert!(client.lock_session(LOCK_COLOR));
    let before = client.received_keys().len();
    client.type_chord(&[keys::LEFT_SUPER, keys::UP]);
    assert_eq!(
        client.received_keys()[before..],
        [keys::LEFT_SUPER, keys::UP],
        "the locker got the keys"
    );
    client.unlock_session();
    assert_eq!(
        client.screenshot().bounds_of(WINDOW_COLOR),
        Some(content),
        "not maximized: the binding rested, and the window shows again"
    );
}

#[test]
fn a_locker_that_dies_leaves_the_session_locked() {
    let (vaht, mut client, _) = desktop();
    let mut locker = TestClient::connect(vaht.socket());
    assert!(locker.lock_session(LOCK_COLOR));
    drop(locker);
    vaht.wait_for_disconnections(1);
    let shown = client.screenshot();
    assert_eq!(shown.bounds_of(WINDOW_COLOR), None, "still hidden");
}
