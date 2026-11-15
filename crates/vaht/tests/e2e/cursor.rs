//! Pointer tests: vaht draws the pointer where no host does, and in
//! screenshots that ask for it, from the cursor theme or as its own arrow.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, TestClient, solid_xcursor};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Where the tests put the pointer.
const POINTER: (u32, u32) = (400, 300);

/// The theme's cursor color.
const CURSOR_COLOR: [u8; 3] = [0xe0, 0x30, 0x30];

#[test]
fn a_screenshot_asking_for_the_pointer_shows_the_themes_cursor() {
    let picture = solid_xcursor(16, CURSOR_COLOR);
    let vaht = HeadlessVaht::start_with_files(
        Path::new(EXECUTABLE),
        &[],
        &[("icons/default/cursors/default", &picture)],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.move_pointer_to(POINTER.0, POINTER.1);
    let shown = client.screenshot_with_pointer();
    // The cursor's hotspot is its middle: it covers 8 pixels each way.
    assert_eq!(shown.rgb(POINTER.0, POINTER.1), CURSOR_COLOR);
    assert_eq!(shown.rgb(POINTER.0 - 8, POINTER.1 - 8), CURSOR_COLOR);
    assert_ne!(shown.rgb(POINTER.0 + 8, POINTER.1 + 8), CURSOR_COLOR);
    let plain = client.screenshot();
    assert_ne!(
        plain.rgb(POINTER.0, POINTER.1),
        CURSOR_COLOR,
        "not asked for"
    );
}

#[test]
fn without_a_cursor_theme_the_pointer_is_vahts_arrow() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.move_pointer_to(POINTER.0, POINTER.1);
    let plain = client.screenshot();
    let shown = client.screenshot_with_pointer();
    // The arrow's tip is at the pointer; its body runs down and right.
    assert_ne!(
        shown.rgb(POINTER.0 + 2, POINTER.1 + 6),
        plain.rgb(POINTER.0 + 2, POINTER.1 + 6),
        "the arrow is drawn"
    );
    assert_eq!(
        shown.rgb(POINTER.0 - 4, POINTER.1 - 4),
        plain.rgb(POINTER.0 - 4, POINTER.1 - 4),
        "nothing above and left of its tip"
    );
}
