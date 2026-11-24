//! Taskbar tests: its window list, its buttons, and the room it keeps,
//! checked by screenshots and clicks. A style of solid colors tells its
//! parts apart.

use std::path::Path;

use vaht_test_support::{
    BUILT_IN_METRICS, BUTTON_LEFT, BUTTON_RIGHT, Bounds, HeadlessVaht, Screenshot, TestClient,
    WINDOW_COLOR, solid_png,
};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The taskbar's height in the test style, in pixels.
const HEIGHT: u32 = 20;

/// The most an entry is wide where the tests limit it, in pixels; well
/// inside an entry however wide.
const ENTRY_WIDTH: u32 = 128;

/// The focused window's entry.
const FOCUSED: [u8; 3] = [0xff, 0x00, 0x00];

/// Another window's entry.
const UNFOCUSED: [u8; 3] = [0x00, 0x00, 0xff];

/// The taskbar where nothing else is.
const BACKGROUND: [u8; 3] = [0x00, 0xff, 0x00];

/// A style whose taskbar parts are solid colors, text in the color it is
/// on, so it never shows.
const STYLE: &str = "\
[default]
text_effect = \"none\"
[taskbar]
height = 20
texture = { solid = \"#00ff00\" }
[taskbar.window-list.empty]
texture = { solid = \"#00ff00\" }
[taskbar.window-list.entry.focused]
texture = { solid = \"#ff0000\" }
text_color = \"#ff0000\"
[taskbar.window-list.entry.unfocused]
texture = { solid = \"#0000ff\" }
text_color = \"#0000ff\"
";

/// Starts vaht with the test style and a taskbar of `widgets`, and a
/// client.
fn start(widgets: &str) -> (HeadlessVaht, TestClient) {
    start_with(widgets, "")
}

/// Starts vaht with the test style, a taskbar of `widgets`, and
/// `window_list` settings.
fn start_with(widgets: &str, window_list: &str) -> (HeadlessVaht, TestClient) {
    let settings = format!(
        "style = \"test\"\n[taskbar]\nwidgets = {widgets}\n[taskbar.window_list]\n{window_list}"
    );
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", &settings), ("styles/test.toml", STYLE)],
    );
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

/// Returns the runs of color along the taskbar's middle row, left to right:
/// each color and how many pixels it covers.
fn taskbar_runs(screenshot: &Screenshot) -> Vec<([u8; 3], u32)> {
    let y = screenshot.height() - HEIGHT / 2;
    let mut runs: Vec<([u8; 3], u32)> = Vec::new();
    for x in 0..screenshot.width() {
        let color = screenshot.rgb(x, y);
        match runs.last_mut() {
            Some((last, length)) if *last == color => *length += 1,
            _ => runs.push((color, 1)),
        }
    }
    runs
}

#[test]
fn each_window_has_an_entry_and_the_focused_ones_stands_out() {
    let (_vaht, mut client) = start("[\"window-list\"]");
    client.open_windows(2);
    let screenshot = client.screenshot();
    let half = screenshot.width() / 2;
    assert_eq!(
        taskbar_runs(&screenshot),
        [(UNFOCUSED, half), (FOCUSED, half)],
        "in the order they opened, the second focused, sharing the whole list"
    );
}

#[test]
fn entries_limited_in_width_leave_the_rest_of_the_list_empty() {
    let (_vaht, mut client) = start_with(
        "[\"window-list\"]",
        &format!("entry_width = {{ max = {ENTRY_WIDTH} }}\n"),
    );
    client.open_windows(2);
    let runs = taskbar_runs(&client.screenshot());
    assert_eq!(
        runs.iter().take(2).copied().collect::<Vec<_>>(),
        [(UNFOCUSED, ENTRY_WIDTH), (FOCUSED, ENTRY_WIDTH)],
    );
    assert_eq!(runs.get(2).map(|(color, _)| *color), Some(BACKGROUND));
}

#[test]
fn activate_window_list_entry_does_what_clicking_the_entry_does() {
    let (vaht, mut client) = start("[\"window-list\"]");
    client.open_windows(2);
    let run = |command: &str| {
        let output = std::process::Command::new(EXECUTABLE)
            .args(["run", command])
            .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
            .output()
            .expect("vaht runs");
        assert!(output.status.success(), "{output:?}");
    };
    let entries = |client: &mut TestClient| {
        taskbar_runs(&client.screenshot())
            .iter()
            .take(2)
            .map(|(color, _)| *color)
            .collect::<Vec<_>>()
    };
    run("activate-window-list-entry 1");
    assert_eq!(
        entries(&mut client),
        [FOCUSED, UNFOCUSED],
        "the first focused"
    );
    run("activate-window-list-entry 1");
    assert_eq!(
        entries(&mut client),
        [UNFOCUSED, FOCUSED],
        "the focused one minimized, focus passing on"
    );
    run("activate-window-list-entry 3");
    assert_eq!(entries(&mut client), [UNFOCUSED, FOCUSED], "no third entry");
}

#[test]
fn right_clicking_an_entry_opens_its_windows_menu() {
    // A window menu of one item, which touches a file when chosen.
    let files = tempfile::tempdir().expect("a temporary directory");
    let marker = files.path().join("chosen");
    let menus = format!(
        "[menu.root]\nitems = []\n[menu.workspace]\nitems = []\n\
         [menu.window]\nitems = [{{ label = \"Touch\", command = \"exec touch '{}'\" }}]\n",
        marker.display()
    );
    let settings = "style = \"test\"\n[taskbar]\nwidgets = [\"window-list\"]\n";
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", settings),
            ("styles/test.toml", STYLE),
            ("menus.toml", &menus),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let height = client.screenshot().height();
    client.click(ENTRY_WIDTH / 2, height - HEIGHT / 2, BUTTON_RIGHT);
    // Opened at the pointer, which is on the taskbar, the menu is moved up
    // to clear it: its one item sits just above the taskbar.
    // A screenshot waits until the menu is drawn.
    client.screenshot();
    let item = (ENTRY_WIDTH / 2 + 5, height - HEIGHT - 1 - 10);
    client.click(item.0, item.1, BUTTON_LEFT);
    vaht_test_support::wait_for_file(&marker, |_| true);
}

#[test]
fn clicking_an_entry_minimizes_its_focused_window_and_again_restores_it() {
    let (_vaht, mut client) = start("[\"window-list\"]");
    client.open_windows(1);
    let screenshot = client.screenshot();
    let y = screenshot.height() - HEIGHT / 2;
    client.click(ENTRY_WIDTH / 2, y, BUTTON_LEFT);
    let minimized = client.screenshot();
    assert_eq!(minimized.bounds_of(WINDOW_COLOR), None, "minimized");
    assert_eq!(
        taskbar_runs(&minimized).first().map(|(color, _)| *color),
        Some(UNFOCUSED),
        "its entry stays, unfocused"
    );
    client.click(ENTRY_WIDTH / 2, y, BUTTON_LEFT);
    let restored = client.screenshot();
    assert!(restored.bounds_of(WINDOW_COLOR).is_some(), "shown again");
    assert_eq!(
        taskbar_runs(&restored).first().map(|(color, _)| *color),
        Some(FOCUSED)
    );
}

#[test]
fn the_next_workspace_button_shows_the_next_workspace() {
    let (_vaht, mut client) = start("[\"workspace-next\", \"window-list\"]");
    client.open_windows(1);
    let screenshot = client.screenshot();
    client.click(HEIGHT / 2, screenshot.height() - HEIGHT / 2, BUTTON_LEFT);
    let next = client.screenshot();
    assert_eq!(
        next.bounds_of(WINDOW_COLOR),
        None,
        "the window stays on workspace 1"
    );
    assert!(
        !taskbar_runs(&next)
            .iter()
            .any(|(color, _)| *color == FOCUSED || *color == UNFOCUSED),
        "and the list shows workspace 2's windows: none"
    );
}

/// Returns which of `count` entries, sharing the whole window list, is
/// the focused one's, counted from the left.
fn focused_entry(screenshot: &Screenshot, count: u32) -> u32 {
    let mut x = 0;
    let (mut list_start, mut list_end, mut focused) = (None, 0, None);
    for (color, length) in taskbar_runs(screenshot) {
        if color == FOCUSED || color == UNFOCUSED {
            list_start.get_or_insert(x);
            list_end = x + length;
        }
        if color == FOCUSED {
            focused = Some(x);
        }
        x += length;
    }
    let start = list_start.expect("the list shows entries");
    let focused = focused.expect("an entry is focused");
    (focused - start) * count / (list_end - start)
}

#[test]
fn the_window_arrows_go_through_the_window_list_in_its_order() {
    let (_vaht, mut client) = start("[\"window-previous\", \"window-next\", \"window-list\"]");
    client.open_windows(3);
    let bottom = client.screenshot().height() - HEIGHT / 2;
    assert_eq!(focused_entry(&client.screenshot(), 3), 2, "the last opened");
    // The arrows are as wide as the taskbar is tall: previous, then next.
    let (previous, next) = (HEIGHT / 2, HEIGHT + HEIGHT / 2);
    client.click(next, bottom, BUTTON_LEFT);
    assert_eq!(
        focused_entry(&client.screenshot(), 3),
        0,
        "next wraps around"
    );
    client.click(next, bottom, BUTTON_LEFT);
    assert_eq!(focused_entry(&client.screenshot(), 3), 1);
    client.click(previous, bottom, BUTTON_LEFT);
    client.click(previous, bottom, BUTTON_LEFT);
    assert_eq!(
        focused_entry(&client.screenshot(), 3),
        2,
        "previous wraps around"
    );
}

/// Returns how tall the taskbar is: how far up from the screen's bottom
/// its colors reach at its left end.
fn taskbar_height(screenshot: &Screenshot) -> u32 {
    let is_taskbar = |y| [FOCUSED, UNFOCUSED, BACKGROUND].contains(&screenshot.rgb(2, y));
    (0..screenshot.height())
        .take_while(|up| is_taskbar(screenshot.height() - 1 - up))
        .count()
        .try_into()
        .expect("a height fits in u32")
}

/// Maximizes the focused window and returns the lowest row any window's
/// content reaches: the maximized one's.
fn maximize_and_find_bottom(client: &mut TestClient) -> u32 {
    client.type_chord(&[
        vaht_test_support::keys::LEFT_SUPER,
        vaht_test_support::keys::UP,
    ]);
    lowest_window_row(client)
}

/// Returns the lowest row any window's content reaches.
fn lowest_window_row(client: &mut TestClient) -> u32 {
    client
        .screenshot()
        .bounds_of(WINDOW_COLOR)
        .expect("a window shows")
        .bottom
}

#[test]
fn a_taskbar_of_two_rows_wraps_its_list_and_keeps_windows_off_both() {
    let (_vaht, mut client) = start_with("[\"window-list\"]\nrows = 2", "");
    client.open_windows(3);
    let screenshot = client.screenshot();
    assert_eq!(taskbar_height(&screenshot), 2 * HEIGHT);
    // Two entries on the top row, the third, focused, below, as wide.
    let half = screenshot.width() / 2;
    let top_row = screenshot.height() - HEIGHT - HEIGHT / 2;
    assert_eq!(screenshot.rgb(half / 2, top_row), UNFOCUSED);
    assert_eq!(screenshot.rgb(half + half / 2, top_row), UNFOCUSED);
    assert_eq!(
        taskbar_runs(&screenshot),
        [(FOCUSED, half), (BACKGROUND, half)]
    );
    let two_rows = maximize_and_find_bottom(&mut client);
    let (_one_row_vaht, mut one_row_client) = start_with("[\"window-list\"]", "");
    one_row_client.open_windows(3);
    let one_row = maximize_and_find_bottom(&mut one_row_client);
    assert_eq!(
        two_rows,
        one_row - HEIGHT,
        "a maximized window stops above both rows"
    );
}

#[test]
fn an_auto_taskbar_takes_a_row_when_entries_get_too_narrow() {
    // 1920 pixels wide: two entries of 960 fit, three of 640 do not.
    let (_vaht, mut client) = start_with("[\"window-list\"]\nrows = \"auto\"\nrow_limit = 700", "");
    client.open_windows(2);
    assert_eq!(taskbar_height(&client.screenshot()), HEIGHT);
    let one_row = maximize_and_find_bottom(&mut client);
    client.open_windows(1);
    let screenshot = client.screenshot();
    assert_eq!(taskbar_height(&screenshot), 2 * HEIGHT, "a second row");
    assert_eq!(
        lowest_window_row(&mut client),
        one_row - HEIGHT,
        "the maximized window shrinks to keep off it"
    );
    client.close_windows();
}

#[test]
fn an_auto_taskbar_takes_a_row_at_a_share_of_the_outputs_width() {
    // 1920 pixels wide, so 36% is 691 pixels: two entries of 960 fit, three
    // of 640 do not.
    let (_vaht, mut client) = start_with(
        "[\"window-list\"]\nrows = \"auto\"\nrow_limit = { percent = 36 }",
        "",
    );
    client.open_windows(2);
    assert_eq!(taskbar_height(&client.screenshot()), HEIGHT);
    let one_row = maximize_and_find_bottom(&mut client);
    client.open_windows(1);
    let screenshot = client.screenshot();
    assert_eq!(taskbar_height(&screenshot), 2 * HEIGHT, "a second row");
    assert_eq!(
        lowest_window_row(&mut client),
        one_row - HEIGHT,
        "the maximized window shrinks to keep off it"
    );
    client.close_windows();
}

#[test]
fn an_auto_taskbar_takes_a_row_for_so_many_windows() {
    let (_vaht, mut client) = start_with(
        "[\"window-list\"]\nrows = \"auto\"\nrow_limit = { windows = 2 }",
        "",
    );
    client.open_windows(2);
    assert_eq!(
        taskbar_height(&client.screenshot()),
        HEIGHT,
        "two fit a row"
    );
    client.open_windows(1);
    assert_eq!(
        taskbar_height(&client.screenshot()),
        2 * HEIGHT,
        "a third takes a second row, however wide they are"
    );
    client.close_windows();
}

#[test]
fn a_taskbar_never_takes_a_row_for_its_tray() {
    let (_vaht, mut client) = start_with("[\"window-list\", \"tray\"]\nrows = \"auto\"", "");
    client.open_windows(1);
    assert_eq!(taskbar_height(&client.screenshot()), HEIGHT);
}

#[test]
fn without_a_taskbar_windows_maximize_over_the_whole_output() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", "[taskbar]\nenabled = false\n")],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    client.type_chord(&[
        vaht_test_support::keys::LEFT_SUPER,
        vaht_test_support::keys::UP,
    ]);
    let maximized = client.screenshot();
    let content = maximized.bounds_of(WINDOW_COLOR).expect("shown");
    assert_eq!(
        content.bottom,
        maximized.height() - 1 - BUILT_IN_METRICS.below_content(),
        "only the frame's bottom below it"
    );
}

#[test]
fn a_clock_showing_seconds_ticks_each_second() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[(
            "settings.toml",
            "[taskbar]\nwidgets = [\"clock\"]\n[taskbar.clock]\nformat = \"%H:%M:%S\"\n",
        )],
    );
    let mut client = TestClient::connect(vaht.socket());
    let before = client.screenshot();
    let taskbar = Bounds {
        left: 0,
        top: before.height() - HEIGHT,
        right: before.width() - 1,
        bottom: before.height() - 1,
    };
    let mut ticks = 0;
    let mut last = before;
    // Three ticks: each re-arms the clock from the tick before it.
    for _ in 0..3 {
        // Time passing is what is tested: this waits for a moment, it does
        // not look for a change (General F9).
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        let now = client.screenshot();
        if now.differs_within(&last, taskbar) {
            ticks += 1;
        }
        last = now;
    }
    assert_eq!(ticks, 3, "the clock's text changed every second");
}

/// The test client's icon, as its desktop entry names it.
const ICON: [u8; 3] = [0xff, 0x00, 0xff];

/// The icon a window names itself.
const NAMED_ICON: [u8; 3] = [0x00, 0xff, 0xff];

/// Starts vaht with the test style, a window list, `settings` added, and an
/// icon for the test client's app id; opens a window.
fn start_with_icon(settings: &str) -> (HeadlessVaht, TestClient) {
    let settings = format!("style = \"test\"\n[taskbar]\nwidgets = [\"window-list\"]\n{settings}");
    let png = solid_png(32, ICON);
    let named_png = solid_png(32, NAMED_ICON);
    let vaht = HeadlessVaht::start_with_files(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", settings.as_bytes()),
            ("styles/test.toml", STYLE.as_bytes()),
        ],
        &[
            (
                "applications/vaht.test.desktop",
                b"[Desktop Entry]\nName=Test\nIcon=vaht-test-icon\n",
            ),
            ("icons/hicolor/32x32/apps/vaht-test-icon.png", &png),
            ("icons/hicolor/32x32/apps/vaht-named-icon.png", &named_png),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    (vaht, client)
}

#[test]
fn an_entry_shows_its_application_s_icon_left_of_its_title() {
    let (_vaht, mut client) = start_with_icon("");
    let screenshot = client.screenshot();
    let middle = screenshot.height() - HEIGHT / 2;
    assert_eq!(screenshot.rgb(0, middle), FOCUSED, "the entry's edge");
    assert_eq!(screenshot.rgb(1, middle), ICON, "the icon, a pixel in");
    assert_eq!(
        screenshot.rgb(HEIGHT - 2, middle),
        ICON,
        "as tall as the bar, less a pixel each side"
    );
    assert_eq!(
        screenshot.rgb(HEIGHT, middle),
        FOCUSED,
        "then the entry again"
    );
    assert_eq!(
        screenshot.rgb(9, screenshot.height() - HEIGHT),
        FOCUSED,
        "a pixel above it"
    );
}

#[test]
fn entries_can_leave_icons_out() {
    let (_vaht, mut client) = start_with_icon("[taskbar.window_list]\nicons = false\n");
    let screenshot = client.screenshot();
    assert_eq!(screenshot.rgb(9, screenshot.height() - HEIGHT / 2), FOCUSED);
}

#[test]
fn the_icon_a_window_names_comes_before_its_application_s() {
    let (_vaht, mut client) = start_with_icon("");
    client.name_window_icons("vaht-named-icon");
    let screenshot = client.screenshot();
    assert_eq!(
        screenshot.rgb(9, screenshot.height() - HEIGHT / 2),
        NAMED_ICON
    );
}

#[test]
fn an_auto_hiding_taskbar_shows_at_its_edge_and_hides_when_left() {
    let (vaht, mut client) = start("[\"window-list\"]\nauto_hide = true\nauto_hide_delay_ms = 50");
    let bottom_middle = |screenshot: &Screenshot| {
        screenshot.rgb(screenshot.width() / 2, screenshot.height() - HEIGHT / 2)
    };
    let hidden = client.screenshot();
    assert_ne!(bottom_middle(&hidden), BACKGROUND, "hidden at first");
    let (width, height) = (hidden.width(), hidden.height());
    client.move_pointer_to(width / 2, height - 1);
    assert_eq!(
        bottom_middle(&client.screenshot()),
        BACKGROUND,
        "shown at the edge"
    );
    client.move_pointer_to(width / 2, height / 2);
    vaht.wait_for_logged("taskbar hidden", 1);
    assert_ne!(
        bottom_middle(&client.screenshot()),
        BACKGROUND,
        "hidden again once the pointer left"
    );
}

#[test]
fn a_narrower_taskbar_sits_where_it_is_aligned() {
    let (_vaht, mut client) = start("[\"window-list\"]\nwidth_percent = 50\nalign = \"right\"");
    let screenshot = client.screenshot();
    let row = screenshot.height() - HEIGHT / 2;
    let half = screenshot.width() / 2;
    assert_ne!(
        screenshot.rgb(half - 1, row),
        BACKGROUND,
        "the left half is free"
    );
    assert_eq!(
        screenshot.rgb(half, row),
        BACKGROUND,
        "the bar starts halfway"
    );
    assert_eq!(screenshot.rgb(screenshot.width() - 1, row), BACKGROUND);
}

#[test]
fn a_window_s_drawn_icon_is_shown() {
    const DRAWN_ICON: [u8; 3] = [0xff, 0xff, 0x00];
    let (_vaht, mut client) = start_with_icon("");
    client.draw_window_icons(16, DRAWN_ICON);
    let screenshot = client.screenshot();
    assert_eq!(
        screenshot.rgb(9, screenshot.height() - HEIGHT / 2),
        DRAWN_ICON,
        "the picture the window drew, before its application's icon"
    );
}

#[test]
fn a_drawn_icon_with_an_empty_name_is_shown_as_drawn() {
    // Wine's Wayland driver names every icon "" as well as drawing it, which
    // is no name: the picture shows, not the nothing the name finds.
    const DRAWN_ICON: [u8; 3] = [0xff, 0xff, 0x00];
    let (_vaht, mut client) = start_with_icon("");
    client.draw_window_icons_with_empty_name(16, DRAWN_ICON);
    let screenshot = client.screenshot();
    assert_eq!(
        screenshot.rgb(9, screenshot.height() - HEIGHT / 2),
        DRAWN_ICON
    );
}

#[test]
fn a_title_too_long_for_its_entry_shows_in_a_tooltip_until_a_click() {
    let settings = "[taskbar]\nwidgets = [\"window-list\"]\n[taskbar.tooltips]\ndelay_ms = 10\n\
                    [taskbar.window_list]\nentry_width = { max = 30 }\n";
    let vaht =
        HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("settings.toml", settings)]);
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let height = client.screenshot().height();
    let pointer = (15, height - HEIGHT / 2);
    // Inside the tooltip's bottom padding: it is above the pointer, and
    // kept on the output at its left edge.
    let spot = (3, pointer.1 - 13);
    let before = client.screenshot().rgb(spot.0, spot.1);
    client.move_pointer_to(pointer.0, pointer.1);
    vaht.wait_for_logged("tooltip shown", 1);
    let shown = client.screenshot().rgb(spot.0, spot.1);
    assert_eq!(shown, [0, 0, 0], "the built-in tooltip is black");
    assert_ne!(before, shown);
    client.press(BUTTON_RIGHT);
    client.release(BUTTON_RIGHT);
    assert_eq!(
        client.screenshot().rgb(spot.0, spot.1),
        before,
        "a click hides it"
    );
}

/// Starts vaht on two outputs (800 × 600, then 640 × 480) with the test
/// style, a window list taskbar on every output listing `windows`, and one
/// window open on the first output; returns the first entry's color on
/// each output's taskbar.
fn every_output_with(windows: &str) -> ([u8; 3], [u8; 3]) {
    let settings = format!(
        "style = \"test\"\n[taskbar]\nwidgets = [\"window-list\"]\non_every_output = true\n\
         windows = \"{windows}\"\n"
    );
    let vaht = HeadlessVaht::start_with_outputs(
        Path::new(EXECUTABLE),
        "800x600,640x480",
        &[
            ("settings.toml", settings.as_bytes()),
            ("styles/test.toml", STYLE.as_bytes()),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let first_entry = |client: &mut TestClient, output: &str| {
        let screenshot = client.screenshot_of(output);
        screenshot.rgb(5, screenshot.height() - HEIGHT / 2)
    };
    (
        first_entry(&mut client, "HEADLESS-1"),
        first_entry(&mut client, "HEADLESS-2"),
    )
}

#[test]
fn every_output_can_have_a_taskbar_listing_all_windows_or_its_own() {
    assert_eq!(
        every_output_with("all"),
        (FOCUSED, FOCUSED),
        "both list the window"
    );
    assert_eq!(
        every_output_with("own-output"),
        (FOCUSED, BACKGROUND),
        "only the window's own output lists it"
    );
}
