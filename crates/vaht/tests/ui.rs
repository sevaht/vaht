//! UI tests: vaht drawn and clicked, checked by screenshots.
//!
//! Each test runs a headless vaht, opens windows from a test client, moves
//! and clicks a virtual pointer, and checks the pixels of screenshots taken
//! through the same capture protocol `grim` uses. Positions are found from
//! the screenshot itself (the window's fill color), not assumed.

use std::path::Path;

use vaht_test_support::{
    BUTTON_LEFT, Bounds, HeadlessVaht, Screenshot, TestClient, WINDOW_COLOR, WINDOW_SIZE,
};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The built-in theme's titlebar height, in pixels.
const TITLEBAR_HEIGHT: u32 = 20;

/// The built-in theme's border width, in pixels.
const BORDER_WIDTH: u32 = 1;

/// The built-in theme's border color (`grey60`).
const BORDER_COLOR: [u8; 3] = [153, 153, 153];

/// A headless vaht with one client showing one window.
struct OneWindow {
    /// The compositor.
    _vaht: HeadlessVaht,
    /// The client owning the window.
    client: TestClient,
    /// Where the window's content is, found in a screenshot.
    content: Bounds,
}

impl OneWindow {
    /// Starts vaht and opens a window.
    fn open() -> Self {
        let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
        let mut client = TestClient::connect(vaht.socket());
        client.open_windows(1);
        let content = client
            .screenshot()
            .bounds_of(WINDOW_COLOR)
            .expect("the window's content is on screen");
        Self {
            _vaht: vaht,
            client,
            content,
        }
    }

    /// Returns the titlebar's area: directly above the content, separated
    /// from it by one border line.
    const fn titlebar(&self) -> Bounds {
        Bounds {
            left: self.content.left,
            top: self.content.top - BORDER_WIDTH - TITLEBAR_HEIGHT,
            right: self.content.right,
            bottom: self.content.top - BORDER_WIDTH - 1,
        }
    }

    /// Returns the close button's area: the titlebar's rightmost square.
    const fn close_button(&self) -> Bounds {
        let titlebar = self.titlebar();
        Bounds {
            left: titlebar.right + 1 - TITLEBAR_HEIGHT,
            ..titlebar
        }
    }

    /// Returns a point in the middle of `bounds`.
    const fn middle(bounds: Bounds) -> (u32, u32) {
        (
            bounds.left + (bounds.right - bounds.left) / 2,
            bounds.top + (bounds.bottom - bounds.top) / 2,
        )
    }

    /// Returns a point on the title, well left of the buttons.
    const fn title_point(&self) -> (u32, u32) {
        let titlebar = self.titlebar();
        (titlebar.left + 30, Self::middle(titlebar).1)
    }
}

/// Returns whether `color` is a shade of grey, as the built-in theme's
/// titlebar gradients are.
fn is_grey(color: [u8; 3]) -> bool {
    color[0] == color[1] && color[1] == color[2]
}

#[test]
fn a_window_is_shown_at_its_size_with_a_titlebar_and_border() {
    let mut window = OneWindow::open();
    let content = window.content;
    assert_eq!(
        (
            content.right - content.left + 1,
            content.bottom - content.top + 1
        ),
        WINDOW_SIZE
    );
    let screenshot = window.client.screenshot();
    assert_eq!(
        screenshot.rgb(content.left, content.top - 1),
        BORDER_COLOR,
        "a border line above the content"
    );
    assert_eq!(
        screenshot.rgb(content.left - 1, content.top + 10),
        BORDER_COLOR,
        "a border line left of the content"
    );
    let (x, y) = OneWindow::middle(window.titlebar());
    let titlebar_left = screenshot.rgb(window.titlebar().left + 2, y);
    assert!(
        is_grey(titlebar_left),
        "the titlebar's gradient is grey: {titlebar_left:?}"
    );
    assert_ne!(
        screenshot.rgb(x, window.titlebar().top + 1),
        screenshot.rgb(x, window.titlebar().bottom - 1),
        "the titlebar is a gradient"
    );
}

#[test]
fn clicking_the_close_button_asks_the_window_to_close() {
    let mut window = OneWindow::open();
    let (x, y) = OneWindow::middle(window.close_button());
    window.client.click(x, y, BUTTON_LEFT);
    assert_eq!(window.client.close_requests(), 1);
}

#[test]
fn holding_the_close_button_shows_it_pressed_and_releasing_elsewhere_cancels() {
    let mut window = OneWindow::open();
    let before = window.client.screenshot();
    let (x, y) = OneWindow::middle(window.close_button());
    window.client.move_pointer_to(x, y);
    window.client.press(BUTTON_LEFT);
    let held = window.client.screenshot();
    assert!(
        held.differs_within(&before, window.close_button()),
        "the button looks pressed"
    );
    let (away_x, away_y) = window.title_point();
    window.client.move_pointer_to(away_x, away_y);
    window.client.release(BUTTON_LEFT);
    assert_eq!(
        window.client.close_requests(),
        0,
        "releasing off the button cancels the click"
    );
    let after = window.client.screenshot();
    assert!(
        !after.differs_within(&before, window.close_button()),
        "the button looks released again"
    );
}

#[test]
fn holding_the_title_does_not_change_the_titlebar() {
    let mut window = OneWindow::open();
    let before = window.client.screenshot();
    let (x, y) = window.title_point();
    window.client.move_pointer_to(x, y);
    window.client.press(BUTTON_LEFT);
    let held: Screenshot = window.client.screenshot();
    window.client.release(BUTTON_LEFT);
    assert!(
        !held.differs_within(&before, window.titlebar()),
        "the titlebar looks the same while the title is held"
    );
}
