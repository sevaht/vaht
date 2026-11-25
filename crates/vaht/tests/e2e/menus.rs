//! Menu tests: menus opened by bindings, used with the pointer and the
//! keyboard, and checked by screenshots and what their items run.

use std::path::Path;

use vaht_test_support::{
    BUTTON_LEFT, BUTTON_RIGHT, HeadlessVaht, Screenshot, TestClient, keys, wait_for_file,
};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Where the menu is opened: right-clicked on the desktop.
const OPENED_AT: (u32, u32) = (300, 300);

/// The built-in style's menu sizes: the outline, the title and an item.
const BORDER: u32 = 1;
/// The title's height.
const TITLE_HEIGHT: u32 = 20;
/// An item's height.
const ITEM_HEIGHT: u32 = 20;

/// The built-in style's menu body: black.
const MENU_BODY: [u8; 3] = [0, 0, 0];

/// Starts vaht with a root menu whose one item touches `marker`.
fn start(marker: &Path) -> (HeadlessVaht, TestClient) {
    let menus = format!(
        "[menu.root]\ntitle = \"test\"\nitems = [\n\
         {{ label = \"Touch\", command = \"exec touch '{}'\" }},\n\
         {{ label = \"Other\", command = \"exec true\" }},\n]\n\
         [menu.window]\nitems = []\n[menu.workspace]\nitems = []\n",
        marker.display()
    );
    let vaht = HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("menus.toml", &menus)]);
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

/// Returns a point in the left part of the menu's item `index`, clear of
/// its text.
const fn item_point(index: u32) -> (u32, u32) {
    let (x, y) = OPENED_AT;
    (
        x + BORDER + 5,
        y + BORDER + TITLE_HEIGHT + BORDER + index * ITEM_HEIGHT + ITEM_HEIGHT / 2,
    )
}

/// Returns the color at `point`.
fn at(screenshot: &Screenshot, point: (u32, u32)) -> [u8; 3] {
    screenshot.rgb(point.0, point.1)
}

#[test]
fn a_right_click_on_the_desktop_opens_the_root_menu_and_an_item_runs_its_command() {
    let files = tempfile::tempdir().expect("a temporary directory");
    let marker = files.path().join("touched");
    let (_vaht, mut client) = start(&marker);
    let before = client.screenshot();
    assert_ne!(at(&before, item_point(0)), MENU_BODY, "no menu yet");
    client.click(OPENED_AT.0, OPENED_AT.1, BUTTON_RIGHT);
    let opened = client.screenshot();
    assert_eq!(
        at(&opened, item_point(0)),
        MENU_BODY,
        "the menu, at the pointer"
    );
    client.move_pointer_to(item_point(0).0, item_point(0).1);
    let hovered = client.screenshot();
    assert_ne!(
        at(&hovered, item_point(0)),
        MENU_BODY,
        "the item under the pointer is highlighted"
    );
    assert_eq!(at(&hovered, item_point(1)), MENU_BODY, "the other is not");
    client.click(item_point(0).0, item_point(0).1, BUTTON_LEFT);
    wait_for_file(&marker, |_| true);
    let closed = client.screenshot();
    assert_eq!(
        at(&closed, item_point(0)),
        at(&before, item_point(0)),
        "and the menu closed"
    );
}

#[test]
fn the_keyboard_moves_through_a_menu_and_escape_closes_it() {
    let files = tempfile::tempdir().expect("a temporary directory");
    let marker = files.path().join("touched");
    let (_vaht, mut client) = start(&marker);
    let before = client.screenshot();
    client.click(OPENED_AT.0, OPENED_AT.1, BUTTON_RIGHT);
    client.type_chord(&[keys::DOWN]);
    let highlighted = client.screenshot();
    assert_ne!(
        at(&highlighted, item_point(0)),
        MENU_BODY,
        "Down highlights the first item"
    );
    client.type_chord(&[keys::ESCAPE]);
    let closed = client.screenshot();
    assert_eq!(
        at(&closed, item_point(0)),
        at(&before, item_point(0)),
        "Escape closed it"
    );
    assert!(!marker.exists(), "nothing was chosen");
}

#[test]
fn a_click_elsewhere_closes_the_menu() {
    let files = tempfile::tempdir().expect("a temporary directory");
    let marker = files.path().join("touched");
    let (_vaht, mut client) = start(&marker);
    let before = client.screenshot();
    client.click(OPENED_AT.0, OPENED_AT.1, BUTTON_RIGHT);
    client.click(900, 700, BUTTON_LEFT);
    let closed = client.screenshot();
    assert_eq!(at(&closed, item_point(0)), at(&before, item_point(0)));
}

#[test]
fn the_window_menu_toggles_the_window_it_was_opened_on() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let content = client
        .screenshot()
        .bounds_of(vaht_test_support::WINDOW_COLOR)
        .expect("shown");
    let titlebar = (content.left + 30, content.top - BORDER - 10);
    client.click(titlebar.0, titlebar.1, BUTTON_RIGHT);
    // The built-in window menu has no title: its first item, Shade, is
    // first, just inside its outline.
    let shade = (
        titlebar.0 + BORDER + 5,
        titlebar.1 + BORDER + ITEM_HEIGHT / 2,
    );
    client.click(shade.0, shade.1, BUTTON_LEFT);
    assert_eq!(
        client
            .screenshot()
            .bounds_of(vaht_test_support::WINDOW_COLOR),
        None,
        "shaded: the content is rolled up"
    );
}

/// Returns whether a window of vaht has the focus, from its window list.
fn has_a_focused_window(vaht: &HeadlessVaht) -> bool {
    let output = std::process::Command::new(EXECUTABLE)
        .args(["--json", "windows"])
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht runs");
    let reply: vaht_ipc::Reply = serde_json::from_slice(&output.stdout).expect("one JSON reply");
    let vaht_ipc::Reply::Windows { windows } = reply else {
        panic!("not a window list: {reply:?}");
    };
    windows.iter().any(|window| window.is_focused)
}

/// Runs `vaht run <command>` on `vaht`.
fn run(vaht: &HeadlessVaht, command: &str) {
    let status = std::process::Command::new(EXECUTABLE)
        .args(["run", command])
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .status()
        .expect("vaht runs");
    assert!(status.success(), "{command}");
}

#[test]
fn opening_a_menu_from_the_desktop_lets_go_of_the_focus_as_a_click_there_does() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    assert!(has_a_focused_window(&vaht), "the window has the focus");
    client.click(1500, 900, BUTTON_RIGHT);
    assert!(
        !has_a_focused_window(&vaht),
        "the menu is up and no window looks active"
    );
}

#[test]
fn a_style_option_changed_from_the_menu_leaves_the_windows_unfocused_while_it_stays_up() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    client.click(1500, 900, BUTTON_RIGHT);
    assert!(!has_a_focused_window(&vaht), "the menu is up");
    run(&vaht, "set-setting style_options.palette blue");
    assert!(
        !has_a_focused_window(&vaht),
        "the reload did not hand the focus back to a window"
    );
}

#[test]
fn changing_a_style_option_does_not_give_the_focus_to_a_window_when_none_had_it() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    client.click(1500, 900, BUTTON_LEFT);
    assert!(!has_a_focused_window(&vaht), "nothing has the focus");
    run(&vaht, "set-setting style_options.palette blue");
    assert!(
        !has_a_focused_window(&vaht),
        "the reload left nothing focused"
    );
}

/// Opens a window whose application has a solid icon, then the window-list
/// menu at the pointer, with `settings`; returns the color where the first
/// item's icon goes.
fn window_list_icon_spot(settings: &str) -> [u8; 3] {
    let icon = [0x00, 0xff, 0x00];
    let png = vaht_test_support::solid_png(32, icon);
    let vaht = HeadlessVaht::start_with_files(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", settings.as_bytes()),
            ("bindings.toml", b"[keys]\nF1 = \"window-list-menu\"\n"),
        ],
        &[
            (
                "applications/vaht.test.desktop",
                b"[Desktop Entry]\nName=Test\nIcon=vaht-test-icon\n",
            ),
            ("icons/hicolor/32x32/apps/vaht-test-icon.png", &png),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    client.move_pointer_to(OPENED_AT.0, OPENED_AT.1);
    client.type_chord(&[keys::F1]);
    // The list has no title: the first item is just inside the outline.
    client.screenshot().rgb(
        OPENED_AT.0 + BORDER + 4,
        OPENED_AT.1 + BORDER + ITEM_HEIGHT / 2,
    )
}

#[test]
fn the_window_list_shows_each_window_s_icon() {
    assert_eq!(window_list_icon_spot(""), [0x00, 0xff, 0x00]);
    assert_ne!(
        window_list_icon_spot("[menus]\nwindow_icons = false\n"),
        [0x00, 0xff, 0x00]
    );
}

#[test]
fn a_menu_item_shows_the_icon_it_names_or_the_image_it_points_to() {
    let green = [0x00, 0xff, 0x00];
    let red = [0xff, 0x00, 0x00];
    let menus = "[menu.root]\ntitle = \"test\"\nitems = [\n\
                 { label = \"Named\", command = \"exec true\", icon = \"vaht-test-icon\" },\n\
                 { label = \"File\", command = \"exec true\", icon = \"./red.png\" },\n\
                 { label = \"None\", command = \"exec true\" },\n\
                 { label = \"More\", submenu = \"more\", icon = \"vaht-test-icon\" },\n]\n\
                 [menu.more]\nitems = []\n\
                 [menu.window]\nitems = []\n[menu.workspace]\nitems = []\n";
    let vaht = HeadlessVaht::start_with_files(
        Path::new(EXECUTABLE),
        &[
            ("menus.toml", menus.as_bytes()),
            ("bindings.toml", b"[keys]\nF1 = \"root-menu\"\n"),
            ("red.png", &vaht_test_support::solid_png(32, red)),
        ],
        &[(
            "icons/hicolor/32x32/apps/vaht-test-icon.png",
            &vaht_test_support::solid_png(32, green),
        )],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.move_pointer_to(OPENED_AT.0, OPENED_AT.1);
    client.type_chord(&[keys::F1]);
    let screenshot = client.screenshot();
    let icon_spot = |index: u32| {
        screenshot.rgb(
            OPENED_AT.0 + BORDER + 4,
            OPENED_AT.1 + BORDER + TITLE_HEIGHT + BORDER + index * ITEM_HEIGHT + ITEM_HEIGHT / 2,
        )
    };
    assert_eq!(icon_spot(0), green, "the icon theme's");
    assert_eq!(icon_spot(1), red, "the image file");
    assert_eq!(icon_spot(2), MENU_BODY, "no icon");
    assert_eq!(icon_spot(3), green, "a submenu's");
}

#[test]
fn a_pipe_menu_shows_what_its_command_prints() {
    let files = tempfile::tempdir().expect("a temporary directory");
    let marker = files.path().join("chosen");
    let script = files.path().join("recent");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho 'items = [{{ label = \"Touch\", command = \"exec touch {}\" }}]'\n",
            marker.display()
        ),
    )
    .expect("written");
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("made runnable");
    let menus = format!(
        "[menu.root]\nitems = [{{ label = \"Recent\", pipe = \"{}\" }}]\n\
         [menu.window]\nitems = []\n[menu.workspace]\nitems = []\n",
        script.display()
    );
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            ("menus.toml", &menus),
            ("bindings.toml", "[keys]\nF1 = \"root-menu\"\n"),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.move_pointer_to(OPENED_AT.0, OPENED_AT.1);
    client.type_chord(&[keys::F1]);
    client.type_chord(&[keys::DOWN]);
    client.type_chord(&[keys::RIGHT]);
    vaht.wait_for_logged("pipe menu filled", 1);
    client.type_chord(&[keys::DOWN]);
    client.type_chord(&[keys::ENTER]);
    wait_for_file(&marker, |_| true);
}
