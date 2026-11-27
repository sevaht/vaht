//! UI tests: vaht drawn and clicked, checked by screenshots.
//!
//! Each test runs a headless vaht, opens windows from a test client, moves
//! and clicks a virtual pointer, and checks the pixels of screenshots taken
//! through the same capture protocol `grim` uses. Positions are found from
//! the screenshot itself (the window's fill color), not assumed.

use std::path::Path;

use vaht_test_support::{
    BUILT_IN_METRICS, BUTTON_LEFT, BUTTON_RIGHT, Bounds, HeadlessVaht, Screenshot, TestClient,
    WINDOW_COLOR, WINDOW_SIZE, keys,
};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The built-in style's border color, the focused accent (`grey55`).
const BORDER_COLOR: [u8; 3] = [140, 140, 140];

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
    /// Starts vaht and opens a window. Cascade placement keeps it off the
    /// output's edges, so drags have room on every side.
    fn open() -> Self {
        Self::open_with_settings("")
    }

    /// Starts vaht with `settings` added to its own and opens a window, as
    /// [`Self::open`] does.
    fn open_with_settings(settings: &str) -> Self {
        Self::open_with(settings, None)
    }

    /// Starts vaht with `bindings` as its bindings.toml and opens a window,
    /// as [`Self::open`] does.
    fn open_with_bindings(bindings: &str) -> Self {
        Self::open_with("", Some(bindings))
    }

    /// Starts vaht with `settings` added to its own and `bindings` as its
    /// bindings.toml, if given, and opens a window, as [`Self::open`] does.
    fn open_with(settings: &str, bindings: Option<&str>) -> Self {
        Self::open_with_files(settings, bindings, &[])
    }

    /// Starts vaht as [`Self::open_with`] does, with `others` (a path in the
    /// configuration directory and its text) too.
    fn open_with_files(settings: &str, bindings: Option<&str>, others: &[(&str, &str)]) -> Self {
        let settings = format!("{settings}\n[placement]\nstrategy = \"cascade\"\n");
        let mut files = vec![("settings.toml", settings.as_str())];
        files.extend(bindings.map(|bindings| ("bindings.toml", bindings)));
        files.extend_from_slice(others);
        let vaht = HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &files);
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
            top: self.content.top
                - BUILT_IN_METRICS.border_width
                - BUILT_IN_METRICS.titlebar_height,
            right: self.content.right,
            bottom: self.content.top - BUILT_IN_METRICS.border_width - 1,
        }
    }

    /// Returns the close button's area: the titlebar's rightmost square.
    const fn close_button(&self) -> Bounds {
        let titlebar = self.titlebar();
        Bounds {
            left: titlebar.right + 1 - BUILT_IN_METRICS.titlebar_height,
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

    /// Finds the window's content again, after it moved or resized.
    fn find_content(&mut self) -> Bounds {
        self.client
            .screenshot()
            .bounds_of(WINDOW_COLOR)
            .expect("the window's content is on screen")
    }

    /// Returns a point on the title, well left of the buttons.
    const fn title_point(&self) -> (u32, u32) {
        let titlebar = self.titlebar();
        (titlebar.left + 30, Self::middle(titlebar).1)
    }
}

/// Returns whether `color` is a shade of grey, as the built-in style's
/// titlebar gradients are.
fn is_grey(color: [u8; 3]) -> bool {
    color[0] == color[1] && color[1] == color[2]
}

#[test]
fn a_window_that_asks_to_draw_its_own_decorations_gets_no_frame() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", "[placement]\nstrategy = \"cascade\"\n")],
    );
    let mut client = TestClient::connect(vaht.socket());
    // Chromium asks so unless its "use system title bar" is on.
    client.open_self_decorated_windows(1);
    let screenshot = client.screenshot();
    let content = screenshot
        .bounds_of(WINDOW_COLOR)
        .expect("the window's content is on screen");
    let desktop = screenshot.rgb(2, 2);
    assert_eq!(
        screenshot.rgb(content.left + 40, content.top - 5),
        desktop,
        "no titlebar above it"
    );
    assert_eq!(
        screenshot.rgb(content.left - 1, content.top + 10),
        desktop,
        "no border beside it"
    );
}

#[test]
fn a_window_that_never_negotiates_decorations_gets_no_frame() {
    // With no xdg-decoration object a window draws its own decorations,
    // which is how Wine's windows without a caption (Electron's frameless
    // ones) are made.
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", "[placement]\nstrategy = \"cascade\"\n")],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows_without_decoration_negotiation(1);
    let screenshot = client.screenshot();
    let content = screenshot
        .bounds_of(WINDOW_COLOR)
        .expect("the window's content is on screen");
    let desktop = screenshot.rgb(2, 2);
    assert_eq!(
        screenshot.rgb(content.left + 40, content.top - 5),
        desktop,
        "no titlebar above it"
    );
    assert_eq!(
        screenshot.rgb(content.left - 1, content.top + 10),
        desktop,
        "no border beside it"
    );
}

/// Shows that a window asking the server's frame KDE's way (GTK 3 makes no
/// xdg-decoration object, and draws no frame of its own when it is told,
/// through KDE's protocol, that the server draws it) gets vaht's.
fn a_window_asking_the_servers_frame_kdes_way_has_vahts_frame(is_asked_first: bool) {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", "[placement]\nstrategy = \"cascade\"\n")],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_window_asking_kde_for_the_servers_frame(is_asked_first);
    let screenshot = client.screenshot();
    let content = screenshot
        .bounds_of(WINDOW_COLOR)
        .expect("the window's content is on screen");
    let desktop = screenshot.rgb(2, 2);
    assert_ne!(
        screenshot.rgb(content.left + 40, content.top - 5),
        desktop,
        "a titlebar above it"
    );
}

#[test]
fn a_window_asking_for_the_servers_frame_before_it_is_made_gets_vahts_frame() {
    a_window_asking_the_servers_frame_kdes_way_has_vahts_frame(true);
}

#[test]
fn a_window_asking_for_the_servers_frame_after_it_is_made_gets_vahts_frame() {
    a_window_asking_the_servers_frame_kdes_way_has_vahts_frame(false);
}

#[test]
fn a_frame_rule_frames_a_window_that_draws_its_own() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", "[placement]\nstrategy = \"cascade\"\n"),
            (
                "rules.toml",
                "[[rule]]\nmatch = { app_id = \"vaht.test\" }\nframe = \"full\"\n",
            ),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_self_decorated_windows(1);
    let screenshot = client.screenshot();
    let content = screenshot
        .bounds_of(WINDOW_COLOR)
        .expect("the window's content is on screen");
    assert_eq!(
        screenshot.rgb(content.left - 1, content.top + 10),
        BORDER_COLOR,
        "the rule's frame: its border"
    );
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

/// The color of the icon the window draws.
const ICON_COLOR: [u8; 3] = [0xff, 0xff, 0x00];

/// Opens a window with `settings`, has it draw a solid icon, and returns
/// the color at the titlebar's left end, where the icon goes.
fn titlebar_left_with_an_icon(settings: &str) -> [u8; 3] {
    let mut window = OneWindow::open_with_settings(settings);
    window.client.draw_window_icons(16, ICON_COLOR);
    let titlebar = window.titlebar();
    let point = (
        titlebar.left + BUILT_IN_METRICS.titlebar_height / 2,
        OneWindow::middle(titlebar).1,
    );
    window.client.screenshot().rgb(point.0, point.1)
}

#[test]
fn the_titlebar_shows_the_windows_icon_left_of_its_title() {
    assert_eq!(titlebar_left_with_an_icon(""), ICON_COLOR);
}

#[test]
fn a_titlebar_without_icons_shows_none() {
    assert_ne!(
        titlebar_left_with_an_icon("[windows.frame]\nicon = false\n"),
        ICON_COLOR
    );
}

#[test]
fn clicking_the_close_button_asks_the_window_to_close() {
    let mut window = OneWindow::open();
    let (x, y) = OneWindow::middle(window.close_button());
    window.client.click(x, y, BUTTON_LEFT);
    assert_eq!(window.client.close_requests(), 1);
}

/// The default style, with a hovered look for the window buttons.
fn style_with_hovered_buttons() -> String {
    format!(
        "{}\n[window.button.hovered]\ntexture = {{ solid = \"#ff0000\" }}\n",
        include_str!("../../../../data/styles/default.toml")
    )
}

#[test]
fn a_button_under_the_pointer_looks_hovered_and_goes_back_when_it_leaves() {
    let style = style_with_hovered_buttons();
    let mut window = OneWindow::open_with_files(
        "style = \"hovering\"\n",
        None,
        &[("styles/hovering.toml", style.as_str())],
    );
    let before = window.client.screenshot();
    let (x, y) = OneWindow::middle(window.close_button());
    window.client.move_pointer_to(x, y);
    let over = window.client.screenshot();
    assert!(
        over.differs_within(&before, window.close_button()),
        "the button looks hovered"
    );
    let (away_x, away_y) = window.title_point();
    window.client.move_pointer_to(away_x, away_y);
    let after = window.client.screenshot();
    assert!(
        !after.differs_within(&before, window.close_button()),
        "the button looks as it did"
    );
}

#[test]
fn a_style_with_no_hovered_look_leaves_a_button_as_it_was() {
    let mut window = OneWindow::open();
    let before = window.client.screenshot();
    let (x, y) = OneWindow::middle(window.close_button());
    window.client.move_pointer_to(x, y);
    let over = window.client.screenshot();
    assert!(
        !over.differs_within(&before, window.close_button()),
        "no hovered section, no change"
    );
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

/// Returns `bounds` moved by `x`, `y`.
fn shifted(bounds: Bounds, x: i32, y: i32) -> Bounds {
    let shift = |value: u32, by: i32| value.checked_add_signed(by).expect("stays on screen");
    Bounds {
        left: shift(bounds.left, x),
        top: shift(bounds.top, y),
        right: shift(bounds.right, x),
        bottom: shift(bounds.bottom, y),
    }
}

#[test]
fn dragging_the_title_moves_the_window() {
    let mut window = OneWindow::open();
    let (x, y) = window.title_point();
    window.client.drag((x, y), (x + 60, y + 40), BUTTON_LEFT);
    assert_eq!(window.find_content(), shifted(window.content, 60, 40));
}

#[test]
fn alt_dragging_anywhere_on_the_window_moves_it() {
    let mut window = OneWindow::open();
    let (x, y) = OneWindow::middle(window.content);
    window.client.hold_alt();
    window.client.drag((x, y), (x + 50, y + 70), BUTTON_LEFT);
    window.client.release_modifiers();
    assert_eq!(window.find_content(), shifted(window.content, 50, 70));
}

#[test]
fn dragging_a_border_corner_moves_the_window_as_fluxbox_does() {
    let mut window = OneWindow::open();
    let content = window.content;
    // The bottom border's right end: one pixel below and right of the content.
    let corner = (content.right + 1, content.bottom + 1);
    window
        .client
        .drag(corner, (corner.0 + 40, corner.1 + 30), BUTTON_LEFT);
    assert_eq!(window.find_content(), shifted(content, 40, 30));
}

#[test]
fn dragging_the_left_border_moves_the_window() {
    let mut window = OneWindow::open();
    let content = window.content;
    let (_, middle_y) = OneWindow::middle(content);
    let border = (content.left - 1, middle_y);
    window
        .client
        .drag(border, (border.0 + 25, border.1), BUTTON_LEFT);
    assert_eq!(window.find_content(), shifted(content, 25, 0));
}

/// Bindings that make dragging a border resize the window from the
/// nearest corner, as it does not by default.
const BORDER_RESIZES: &str = "[mouse.border]\n\"Drag+Left\" = \"start-resizing nearest-corner\"\n";

#[test]
fn a_border_bound_to_resizing_still_resizes() {
    let mut window = OneWindow::open_with_bindings(BORDER_RESIZES);
    let content = window.content;
    let corner = (content.right + 1, content.bottom + 1);
    window
        .client
        .drag(corner, (corner.0 + 40, corner.1 + 30), BUTTON_LEFT);
    assert_eq!(
        window.find_content(),
        Bounds {
            right: content.right + 40,
            bottom: content.bottom + 30,
            ..content
        }
    );
}

#[test]
fn alt_right_dragging_resizes_from_the_nearest_corner() {
    let mut window = OneWindow::open();
    let content = window.content;
    // Inside the content's top-left quarter: the top-left corner moves.
    let start = (content.left + 20, content.top + 20);
    window.client.hold_alt();
    window
        .client
        .drag(start, (start.0 - 30, start.1 - 15), BUTTON_RIGHT);
    window.client.release_modifiers();
    assert_eq!(
        window.find_content(),
        Bounds {
            left: content.left - 30,
            top: content.top - 15,
            ..content
        }
    );
}

#[test]
fn an_alt_right_press_that_never_moves_leaves_a_maximized_window_maximized() {
    let mut window = OneWindow::open();
    // Alt+F10 maximizes.
    window.client.hold_alt();
    window.client.press_key(keys::F10);
    window.client.release_key(keys::F10);
    window.client.release_modifiers();
    let maximized = window.find_content();
    assert_ne!(maximized, window.content, "the window maximized");
    let inside = (maximized.left + 20, maximized.top + 20);
    window.client.hold_alt();
    window.client.click(inside.0, inside.1, BUTTON_RIGHT);
    window.client.release_modifiers();
    assert_eq!(window.find_content(), maximized);
    // Still maximized, so Alt+F10 restores it.
    window.client.hold_alt();
    window.client.press_key(keys::F10);
    window.client.release_key(keys::F10);
    window.client.release_modifiers();
    assert_eq!(
        window.find_content(),
        window.content,
        "it was still maximized"
    );
}

#[test]
fn dragging_an_edge_past_the_opposite_one_stops_at_the_smallest_size() {
    let mut window = OneWindow::open_with_bindings(BORDER_RESIZES);
    let content = window.content;
    let (_, middle_y) = OneWindow::middle(content);
    let border = (content.right + 1, middle_y);
    // To the screen's left edge, past the window's: the raw width would be
    // negative (the window is narrower than its distance from the right).
    assert!(
        content.right + 1 > WINDOW_SIZE.0,
        "the drag overshoots the left edge"
    );
    window.client.drag(border, (0, middle_y), BUTTON_LEFT);
    let resized = window.find_content();
    assert_eq!(resized.left, content.left, "the left edge stays");
    assert_eq!(
        resized.right, resized.left,
        "one pixel wide: the smallest a window can be"
    );
}

#[test]
fn the_handle_runs_under_the_content_with_a_grip_at_each_end() {
    const LINE: [u8; 3] = [0, 0, 255];
    const HANDLE: [u8; 3] = [255, 0, 0];
    const GRIP: [u8; 3] = [0, 255, 0];
    /// Fluxbox's grip width, which vaht keeps.
    const GRIP_WIDTH: u32 = 20;
    let style = "\
[window.border]
border = { width = 1, color = \"#0000ff\" }
[window.handle]
height = 4
texture = { solid = \"#ff0000\" }
[window.grip]
texture = { solid = \"#00ff00\" }
";
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            (
                "settings.toml",
                "style = \"test\"\n[windows.frame]\nhandle = true\n[placement]\nstrategy = \"cascade\"\n",
            ),
            ("styles/test.toml", style),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let screenshot = client.screenshot();
    let content = screenshot.bounds_of(WINDOW_COLOR).expect("shown");
    let middle = OneWindow::middle(content).0;
    assert_eq!(
        screenshot.rgb(middle, content.bottom + 1),
        LINE,
        "a line under the content"
    );
    let row = content.bottom + 3;
    let expected = [
        (content.left - 1, LINE),
        (content.left, GRIP),
        (content.left + GRIP_WIDTH - 1, GRIP),
        (content.left + GRIP_WIDTH, LINE),
        (content.left + GRIP_WIDTH + 1, HANDLE),
        (middle, HANDLE),
        (content.right - GRIP_WIDTH - 1, HANDLE),
        (content.right - GRIP_WIDTH, LINE),
        (content.right - GRIP_WIDTH + 1, GRIP),
        (content.right, GRIP),
        (content.right + 1, LINE),
    ];
    for (x, color) in expected {
        assert_eq!(screenshot.rgb(x, row), color, "at x = {x}");
    }
    assert_eq!(
        screenshot.rgb(middle, content.bottom + 6),
        LINE,
        "and a last line"
    );
}

/// Settings that show the handle, which the built-in ones leave out.
const SHOWN_HANDLE: &str = "[windows.frame]\nhandle = true\n";

/// A grip: at one end of the handle.
#[derive(Debug, Clone, Copy)]
enum Grip {
    /// At the left end.
    Left,
    /// At the right end.
    Right,
}

/// Drags `grip` of a window `across` and `down`, and checks that the
/// window resized from that bottom corner.
fn check_grip_drag(grip: Grip, across: i32, down: i32) {
    let mut window = OneWindow::open_with_settings(SHOWN_HANDLE);
    let content = window.content;
    // The handle's row, a few pixels inside the grip.
    let handle_row = content.bottom + 2;
    let start = match grip {
        Grip::Left => (content.left + 5, handle_row),
        Grip::Right => (content.right - 5, handle_row),
    };
    let moved = |coordinate: u32, by: i32| {
        coordinate
            .checked_add_signed(by)
            .expect("the drag stays on the output")
    };
    window.client.drag(
        start,
        (moved(start.0, across), moved(start.1, down)),
        BUTTON_LEFT,
    );
    let bottom = moved(content.bottom, down);
    let expected = match grip {
        Grip::Left => Bounds {
            left: moved(content.left, across),
            bottom,
            ..content
        },
        Grip::Right => Bounds {
            right: moved(content.right, across),
            bottom,
            ..content
        },
    };
    assert_eq!(window.find_content(), expected, "{grip:?}");
}

#[test]
fn dragging_the_right_grip_resizes_from_the_bottom_right_corner() {
    check_grip_drag(Grip::Right, 40, 30);
}

#[test]
fn dragging_the_left_grip_resizes_from_the_bottom_left_corner() {
    check_grip_drag(Grip::Left, -25, 20);
}

#[test]
fn a_rounded_corner_is_cut_away_with_the_line_following_it() {
    const LINE: [u8; 3] = [0, 0, 255];
    const TITLEBAR: [u8; 3] = [255, 0, 0];
    let style = "\
[default]
text_effect = \"none\"
[window.titlebar]
texture = { solid = \"#ff0000\" }
[window.label]
texture = { solid = \"#ff0000\" }
text_color = \"#ff0000\"
[window.border]
border = { width = 1, color = \"#0000ff\" }
corner_radius = 8
corners = [\"top-left\", \"top-right\"]
";
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            (
                "settings.toml",
                "style = \"test\"\n[placement]\nstrategy = \"cascade\"\n",
            ),
            ("styles/test.toml", style),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let screenshot = client.screenshot();
    let content = screenshot.bounds_of(WINDOW_COLOR).expect("shown");
    let (left, top) = (
        content.left - BUILT_IN_METRICS.border_width,
        content.top - BUILT_IN_METRICS.above_content(),
    );
    let right = content.right + BUILT_IN_METRICS.border_width;
    let desktop = screenshot.rgb(left - 4, top + 4);
    assert_eq!(screenshot.rgb(left, top), desktop, "the corner is cut away");
    assert_eq!(screenshot.rgb(right, top), desktop, "both top corners");
    assert_eq!(
        screenshot.rgb(left + 10, top),
        LINE,
        "the top line, past the corner"
    );
    assert_eq!(
        screenshot.rgb(left, top + 10),
        LINE,
        "the left line, below it"
    );
    assert_eq!(
        screenshot.rgb(left + 4, top + 4),
        TITLEBAR,
        "inside the arc"
    );
    let arc = screenshot.rgb(left + 2, top + 2);
    assert!(
        arc[2] > arc[0] && arc != desktop,
        "the arc is drawn in the line's color: {arc:?}"
    );
}

#[test]
fn an_outline_resize_shows_the_size_and_resizes_on_release() {
    let mut window =
        OneWindow::open_with("[windows]\nopaque_resize = false\n", Some(BORDER_RESIZES));
    let content = window.content;
    let corner = (content.right + 1, content.bottom + 1);
    let target = (corner.0 + 40, corner.1 + 30);
    window.client.move_pointer_to(corner.0, corner.1);
    window.client.press(BUTTON_LEFT);
    window.client.move_pointer_to(target.0, target.1);
    let during = window.client.screenshot();
    // The outline covers the frame's border and the content's outermost
    // column; within, the window is as it was, and does not reach further.
    let (_, middle_y) = OneWindow::middle(content);
    assert_eq!(during.rgb(content.right - 5, middle_y), WINDOW_COLOR);
    assert_ne!(
        during.rgb(content.right + 10, middle_y),
        WINDOW_COLOR,
        "the window keeps its size while the outline moves"
    );
    // The outline's right line is the frame's last two columns there.
    let [red, green, blue] = during.rgb(target.0, content.top + 10);
    assert!(
        red > 180 && green > 180 && blue > 180,
        "a light outline: {red} {green} {blue}"
    );
    window.client.release(BUTTON_LEFT);
    assert_eq!(
        window.find_content(),
        Bounds {
            right: content.right + 40,
            bottom: content.bottom + 30,
            ..content
        }
    );
}

#[test]
fn a_window_dragged_to_the_left_edge_shows_where_and_takes_the_left_half() {
    let mut window = OneWindow::open_with_settings("[windows.snapping]\nenabled = true\n");
    let (x, y) = OneWindow::middle(window.titlebar());
    window.client.move_pointer_to(x, y);
    window.client.press(BUTTON_LEFT);
    window.client.move_pointer_to(2, 500);
    let during = window.client.screenshot();
    // Low in the left half, clear of the window: the preview's blue.
    let [red, _, blue] = during.rgb(300, 1000);
    assert!(blue > red + 20, "a blue preview: {red} {blue}");
    window.client.release(BUTTON_LEFT);
    let content = window.find_content();
    let half = during.width() / 2;
    assert!(content.left <= 2, "{content:?}");
    assert!(
        content.right + 3 >= half && content.right < half,
        "to the middle: {content:?}"
    );
    let after = window.client.screenshot();
    let [red, _, blue] = after.rgb(1500, 1000);
    assert!(blue <= red + 20, "no preview on the right: {red} {blue}");
}

#[test]
fn what_a_window_draws_outside_its_window_geometry_is_not_shown() {
    // A client's shadow, or an interface it laid out past the size it was
    // given, lies outside its window geometry: it is not part of the window.
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", "[placement]\nstrategy = \"cascade\"\n")],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_window_drawing_outside_its_geometry(24);
    let screenshot = client.screenshot();
    assert_eq!(
        screenshot.bounds_of(vaht_test_support::OUTSIDE_COLOR),
        None,
        "nothing of the rim shows"
    );
    let content = screenshot
        .bounds_of(WINDOW_COLOR)
        .expect("the geometry shows");
    assert_eq!(
        (
            content.right - content.left + 1,
            content.bottom - content.top + 1
        ),
        (WINDOW_SIZE.0 - 48, WINDOW_SIZE.1 - 48),
        "the window is its geometry, no more"
    );
}

#[test]
fn a_window_shared_alone_is_its_window_geometry_and_no_more() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", "[placement]\nstrategy = \"cascade\"\n")],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_window_drawing_outside_its_geometry(24);
    // The capture is the content rectangle, whose corner is the geometry's, so
    // the rim lies outside it with no clipping.
    let shared = vaht_test_support::WindowCapturer::connect(vaht.socket()).capture("test window 1");
    assert_eq!(
        shared.bounds_of(vaht_test_support::OUTSIDE_COLOR),
        None,
        "nothing of the rim is in what is shared"
    );
    assert!(
        shared.bounds_of(WINDOW_COLOR).is_some(),
        "the window's content is"
    );
}
