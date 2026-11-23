//! Pointer tests for games: relative motion (how the mouse moved, before
//! anything holds the pointer), pointer locks, and warps.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, TestClient, WINDOW_COLOR, solid_xcursor};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The cursor theme's color, so screenshots show where the pointer is.
const CURSOR_COLOR: [u8; 3] = [0xe0, 0x30, 0x30];

/// Starts vaht with a solid cursor, and a client with one window, the
/// pointer in its middle; returns the middle.
fn game() -> (HeadlessVaht, TestClient, (u32, u32)) {
    let cursor = solid_xcursor(8, CURSOR_COLOR);
    let vaht = HeadlessVaht::start_with_files(
        Path::new(EXECUTABLE),
        &[],
        &[("icons/default/cursors/default", &cursor)],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let content = client
        .screenshot()
        .bounds_of(WINDOW_COLOR)
        .expect("the window shows");
    let middle = (
        u32::midpoint(content.left, content.right),
        u32::midpoint(content.top, content.bottom),
    );
    client.move_pointer_to(middle.0, middle.1);
    (vaht, client, middle)
}

#[test]
fn the_window_under_the_pointer_hears_how_the_mouse_moved() {
    let (_vaht, mut client, _) = game();
    client.move_pointer_by(12.0, -5.0);
    client.move_pointer_by(3.0, 2.0);
    assert_eq!(client.relative_motion(), (15.0, -3.0));
}

#[test]
fn a_fullscreen_window_opening_under_a_still_pointer_takes_pointer_focus() {
    let (vaht, mut game_client, _) = game();
    assert!(game_client.pointer_is_over_surface());
    let mut game = TestClient::connect(vaht.socket());
    game.open_windows(1);
    game.make_window_fullscreen(0);

    assert!(
        game.pointer_is_over_surface(),
        "the game hears the pointer without it moving"
    );
    assert!(
        !game_client.pointer_is_over_surface(),
        "the game below no longer has it"
    );
}

#[test]
fn a_locked_pointer_stays_put_while_the_game_hears_the_mouse() {
    let (_vaht, mut client, middle) = game();
    assert!(
        client.lock_pointer(),
        "the focused window under the pointer may lock it"
    );
    client.move_pointer_by(40.0, 30.0);
    client.move_pointer_by(40.0, 30.0);
    assert_eq!(client.relative_motion(), (80.0, 60.0));
    let shown = client.screenshot_with_pointer();
    assert_eq!(
        shown.rgb(middle.0, middle.1),
        CURSOR_COLOR,
        "still where it was"
    );
    assert_ne!(shown.rgb(middle.0 + 80, middle.1 + 60), CURSOR_COLOR);
}

#[test]
fn a_window_in_the_background_cannot_lock_the_pointer() {
    let (_vaht, mut client, _) = game();
    // A second window takes the keyboard before the first asks.
    client.open_windows(1);
    assert!(!client.lock_pointer());
}

#[test]
fn a_lock_ends_when_its_window_loses_the_keyboard() {
    let (_vaht, mut client, _) = game();
    assert!(client.lock_pointer());
    // Another window opening takes the keyboard, as switching away does.
    client.open_windows(1);
    assert!(
        !client.is_pointer_locked(),
        "the pointer is the user's again"
    );
}

#[test]
fn a_window_under_the_pointer_may_move_it_within_itself() {
    let (_vaht, mut client, _) = game();
    assert_eq!(
        client.warp_pointer(0, 7.0, 9.0),
        Some((7.0, 9.0)),
        "moved where the window asked"
    );
}

#[test]
fn a_window_may_not_move_the_pointer_off_itself() {
    let (_vaht, mut client, _) = game();
    let before = client.warp_pointer(0, 7.0, 9.0);
    assert_eq!(
        client.warp_pointer(0, -50.0, 9.0),
        before,
        "the pointer stayed on the window"
    );
}
