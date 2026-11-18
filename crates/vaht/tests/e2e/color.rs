//! Colour management tests (`wp_color_manager_v1`): what vaht says it can
//! do and what outputs show, and HDR content converted for the screen.
//!
//! The headless renderer has no shaders, so converting pixels is checked
//! only by [`hdr10_reference_white_is_captured_as_white`], run by hand
//! against a vaht on a GPU.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, Primaries, TestClient, TransferFunction, WINDOW_COLOR};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Starts vaht, and a client.
fn start() -> (HeadlessVaht, TestClient) {
    let vaht = HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[]);
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

#[test]
fn vaht_converts_hdr10_and_srgb() {
    let (_vaht, mut client) = start();
    let converted = client.converted_transfer_functions();
    assert!(
        converted.contains(&TransferFunction::St2084Pq),
        "{converted:?}"
    );
    assert!(converted.contains(&TransferFunction::Srgb), "{converted:?}");
}

#[test]
fn an_output_not_in_hdr_shows_srgb() {
    let (_vaht, mut client) = start();
    assert_eq!(
        client.output_colors(),
        (Some(TransferFunction::Srgb), Some(Primaries::Srgb))
    );
}

#[test]
fn a_window_saying_it_is_hdr10_still_shows() {
    let (_vaht, mut client) = start();
    client.open_windows(1);
    let before = client.screenshot().bounds_of(WINDOW_COLOR);
    client.describe_window_as_hdr10(0);
    assert_eq!(client.screenshot().bounds_of(WINDOW_COLOR), before);
}

/// HDR10's reference white, 203 cd/m², as an 8-bit PQ value.
const PQ_REFERENCE_WHITE: u8 = 148;

#[test]
#[ignore = "needs a vaht on a GPU: run it nested, and set VAHT_GPU_SOCKET to its socket"]
fn hdr10_reference_white_is_captured_as_white() {
    let socket = std::env::var("VAHT_GPU_SOCKET").expect("VAHT_GPU_SOCKET names a vaht's socket");
    let mut client = TestClient::connect(Path::new(&socket));
    client.open_windows(1);
    // Found by its own colour, which nothing else on screen has.
    let window = client
        .screenshot()
        .bounds_of(WINDOW_COLOR)
        .expect("the window shows");
    let (x, y) = (
        u32::midpoint(window.left, window.right),
        u32::midpoint(window.top, window.bottom),
    );
    let grey = [PQ_REFERENCE_WHITE; 3];
    client.redraw_reported(0, grey);
    assert_eq!(
        client.screenshot().rgb(x, y),
        grey,
        "unconverted while sRGB"
    );
    client.describe_window_as_hdr10(0);
    let shown = client.screenshot().rgb(x, y);
    // Captures are sRGB: reference white is white, rolled off a little
    // (to 70 of sRGB's 80 cd/m²: 240).
    assert!(
        shown.iter().all(|channel| channel.abs_diff(240) <= 3),
        "{shown:?}"
    );
}
