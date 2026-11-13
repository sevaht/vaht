//! Window management tests: maximizing, snapping, fullscreen, minimizing,
//! workspaces and window cycling, driven through vaht's built-in bindings
//! and checked by screenshots.
//!
//! Every test uses the built-in `bindings.toml`, so it also checks that the
//! defaults bind what they say.

use std::path::Path;

use vaht_test_support::{
    BUTTON_LEFT, Bounds, HeadlessVaht, Screenshot, TestClient, WINDOW_COLOR, WINDOW_SIZE, keys,
};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The built-in theme's titlebar height, in pixels.
const TITLEBAR_HEIGHT: u32 = 20;

/// The built-in theme's border width, in pixels.
const BORDER_WIDTH: u32 = 1;

/// How far each new window is placed from the one before, in pixels.
const CASCADE_STEP: u32 = 32;

/// A headless vaht with one client and its windows.
struct Desktop {
    /// The compositor.
    _vaht: HeadlessVaht,
    /// The client owning the windows.
    client: TestClient,
}

impl Desktop {
    /// Starts vaht with its built-in configuration and opens `count`
    /// windows.
    fn with_windows(count: usize) -> Self {
        let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
        let mut client = TestClient::connect(vaht.socket());
        client.open_windows(count);
        Self {
            _vaht: vaht,
            client,
        }
    }

    /// Takes a screenshot.
    fn screenshot(&mut self) -> Screenshot {
        self.client.screenshot()
    }

    /// Returns where the windows' content is on screen, if any is.
    fn content(&mut self) -> Option<Bounds> {
        self.screenshot().bounds_of(WINDOW_COLOR)
    }

    /// Types `chord`, such as Super+Up.
    fn type_chord(&mut self, chord: &[u32]) {
        self.client.type_chord(chord);
    }
}

/// Returns the content of a window whose frame fills `outer`.
const fn content_within(outer: Bounds) -> Bounds {
    Bounds {
        left: outer.left + BORDER_WIDTH,
        top: outer.top + 2 * BORDER_WIDTH + TITLEBAR_HEIGHT,
        right: outer.right - BORDER_WIDTH,
        bottom: outer.bottom - BORDER_WIDTH,
    }
}

/// Returns the whole of `screenshot`'s output.
const fn whole_output(screenshot: &Screenshot) -> Bounds {
    Bounds {
        left: 0,
        top: 0,
        right: screenshot.width() - 1,
        bottom: screenshot.height() - 1,
    }
}

#[test]
fn snap_step_up_maximizes_and_down_restores() {
    let mut desktop = Desktop::with_windows(1);
    let floating = desktop.content().expect("the window is shown");

    desktop.type_chord(&[keys::LEFT_SUPER, keys::UP]);
    let maximized = desktop.screenshot();
    assert_eq!(
        maximized.bounds_of(WINDOW_COLOR),
        Some(content_within(whole_output(&maximized))),
        "the frame fills the output"
    );

    desktop.type_chord(&[keys::LEFT_SUPER, keys::DOWN]);
    assert_eq!(desktop.content(), Some(floating), "back where it was");
}

#[test]
fn snap_step_left_fills_the_left_half_and_right_undoes_it() {
    let mut desktop = Desktop::with_windows(1);
    let floating = desktop.content().expect("the window is shown");

    desktop.type_chord(&[keys::LEFT_SUPER, keys::LEFT]);
    let snapped = desktop.screenshot();
    let output = whole_output(&snapped);
    let left_half = Bounds {
        right: snapped.width() / 2 - 1,
        ..output
    };
    assert_eq!(
        snapped.bounds_of(WINDOW_COLOR),
        Some(content_within(left_half))
    );

    desktop.type_chord(&[keys::LEFT_SUPER, keys::RIGHT]);
    assert_eq!(desktop.content(), Some(floating));
}

#[test]
fn snap_step_down_minimizes_and_up_restores_while_super_is_held() {
    let mut desktop = Desktop::with_windows(1);
    let floating = desktop.content().expect("the window is shown");

    desktop.client.press_key(keys::LEFT_SUPER);
    desktop.type_chord(&[keys::DOWN]);
    assert_eq!(desktop.content(), None, "minimized");
    desktop.type_chord(&[keys::UP]);
    desktop.client.release_key(keys::LEFT_SUPER);
    assert_eq!(desktop.content(), Some(floating), "restored, not maximized");
}

#[test]
fn fullscreen_covers_the_whole_output_without_a_frame() {
    let mut desktop = Desktop::with_windows(1);
    let floating = desktop.content().expect("the window is shown");

    desktop.type_chord(&[keys::LEFT_ALT, keys::F11]);
    let fullscreen = desktop.screenshot();
    assert_eq!(
        fullscreen.bounds_of(WINDOW_COLOR),
        Some(whole_output(&fullscreen))
    );

    desktop.type_chord(&[keys::LEFT_ALT, keys::F11]);
    assert_eq!(desktop.content(), Some(floating));
}

#[test]
fn the_maximize_button_maximizes() {
    let mut desktop = Desktop::with_windows(1);
    let content = desktop.content().expect("the window is shown");
    // The buttons are squares at the titlebar's right end: maximize is the
    // second from the right.
    let titlebar_middle = content.top - BORDER_WIDTH - TITLEBAR_HEIGHT / 2;
    let maximize = content.right + 1 - TITLEBAR_HEIGHT - TITLEBAR_HEIGHT / 2;
    desktop.client.click(maximize, titlebar_middle, BUTTON_LEFT);
    let maximized = desktop.screenshot();
    assert_eq!(
        maximized.bounds_of(WINDOW_COLOR),
        Some(content_within(whole_output(&maximized)))
    );
}

#[test]
fn a_window_sent_to_another_workspace_shows_only_there() {
    let mut desktop = Desktop::with_windows(1);
    let content = desktop.content().expect("the window is shown");

    desktop.type_chord(&[keys::LEFT_SUPER, keys::F2]);
    assert_eq!(desktop.content(), None, "gone from workspace 1");
    desktop.type_chord(&[keys::LEFT_CONTROL, keys::F2]);
    assert_eq!(desktop.content(), Some(content), "shown on workspace 2");
    desktop.type_chord(&[keys::LEFT_CONTROL, keys::F1]);
    assert_eq!(desktop.content(), None, "and not on workspace 1");
}

#[test]
fn alt_tab_walks_windows_by_recent_focus_and_restarts_after_alt_is_let_go() {
    // Three cascaded windows, the third on top. Each window's titlebar lies
    // across the content of the one before, so a spot on that titlebar
    // shows the window color only while the earlier window is above it.
    let mut desktop = Desktop::with_windows(3);
    let union = desktop.content().expect("the windows are shown");
    let titlebar_of = |window: u32| {
        let top = union.top + CASCADE_STEP * window;
        (
            union.left + CASCADE_STEP * window + WINDOW_SIZE.0 / 4,
            top - BORDER_WIDTH - TITLEBAR_HEIGHT / 2,
        )
    };
    // Whether window `lower` (counting from 0) is above window `lower + 1`.
    let is_above_next = |desktop: &mut Desktop, lower: u32| {
        let (x, y) = titlebar_of(lower + 1);
        desktop.screenshot().rgb(x, y) == WINDOW_COLOR
    };
    assert!(!is_above_next(&mut desktop, 0) && !is_above_next(&mut desktop, 1));

    desktop.type_chord(&[keys::LEFT_ALT, keys::TAB]);
    assert!(
        is_above_next(&mut desktop, 1),
        "Alt+Tab raised the second window, the one focused before"
    );

    desktop.type_chord(&[keys::LEFT_ALT, keys::TAB]);
    assert!(
        !is_above_next(&mut desktop, 1),
        "with Alt let go in between, Alt+Tab goes back to the third"
    );
    assert!(
        !is_above_next(&mut desktop, 0),
        "not on to the first, as a cycle still in progress would"
    );
}
