//! Idle tests: the session goes idle when no one touches it, unless a
//! window that shows asks that it not (a video player).

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use vaht_test_support::{HeadlessVaht, TestClient};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// How long without input counts as idle, in the tests.
const IDLE_AFTER: Duration = Duration::from_millis(100);

/// How long a test waits to see the session not go idle: many times
/// [`IDLE_AFTER`].
const STAYS_AWAKE_FOR: Duration = Duration::from_millis(800);

/// How long a test waits for the session to go idle at most.
const PATIENCE: Duration = Duration::from_secs(10);

/// Starts vaht and a client with one window.
fn desktop() -> (HeadlessVaht, TestClient) {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    (vaht, client)
}

#[test]
fn the_session_goes_idle_when_no_one_touches_it() {
    let (_vaht, mut client) = desktop();
    client.watch_idle(IDLE_AFTER);
    assert!(client.wait_for_idle(PATIENCE));
}

#[test]
fn a_window_that_shows_keeps_the_session_awake() {
    let (_vaht, mut client) = desktop();
    client.inhibit_idle();
    client.watch_idle(IDLE_AFTER);
    assert!(!client.wait_for_idle(STAYS_AWAKE_FOR));
}

#[test]
fn a_minimized_window_does_not_keep_the_session_awake() {
    let (vaht, mut client) = desktop();
    client.inhibit_idle();
    let output = Command::new(EXECUTABLE)
        .args(["run", "minimize"])
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht runs");
    assert!(output.status.success(), "{output:?}");
    client.watch_idle(IDLE_AFTER);
    assert!(client.wait_for_idle(PATIENCE));
}
