//! Leak tests: vaht running headless, driven by real Wayland clients.
//!
//! Each test opens and closes windows from test clients, stops vaht with
//! SIGTERM and reads its exit report. It must show every window, frame,
//! client and capture session gone, and as many open descriptors as at
//! startup: anything else is a leak.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, TestClient};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Starts a headless vaht for one test.
fn start_vaht() -> HeadlessVaht {
    HeadlessVaht::start(Path::new(EXECUTABLE))
}

#[test]
fn windows_are_shown_and_leave_nothing_behind_when_closed() {
    let vaht = start_vaht();
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(3);
    client.close_windows();
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn many_clients_coming_and_going_leave_nothing_behind() {
    const ROUNDS: usize = 20;
    const WINDOWS_PER_CLIENT: usize = 5;
    let vaht = start_vaht();
    for _ in 0..ROUNDS {
        let mut client = TestClient::connect(vaht.socket());
        client.open_windows(WINDOWS_PER_CLIENT);
        client.close_windows();
    }
    vaht.wait_for_disconnections(ROUNDS);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn a_client_that_disconnects_with_windows_open_leaves_nothing_behind() {
    let vaht = start_vaht();
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(4);
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn a_client_that_only_takes_screenshots_leaves_nothing_behind() {
    let vaht = start_vaht();
    let mut client = TestClient::connect(vaht.socket());
    let _ = client.screenshot();
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn screenshots_leave_nothing_behind() {
    let vaht = start_vaht();
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(2);
    for _ in 0..5 {
        let _ = client.screenshot();
    }
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn clients_find_the_protocols_everyday_programs_use() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let client = TestClient::connect(vaht.socket());
    let interfaces = client.global_interfaces();
    for expected in [
        "wl_compositor",
        "wl_data_device_manager",
        "wl_seat",
        "wl_shm",
        "wp_fractional_scale_manager_v1",
        "wp_single_pixel_buffer_manager_v1",
        "wp_viewporter",
        "xdg_activation_v1",
        "xdg_wm_base",
        "zwp_primary_selection_device_manager_v1",
        "zxdg_decoration_manager_v1",
    ] {
        assert!(
            interfaces.iter().any(|name| name == expected),
            "{expected} missing: {interfaces:?}"
        );
    }
    assert!(
        !interfaces.iter().any(|name| name == "zwp_linux_dmabuf_v1"),
        "the software renderer cannot import GPU buffers, so clients must not be offered them"
    );
}
