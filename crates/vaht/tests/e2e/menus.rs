//! Menu tests: menus opened by bindings, used with the pointer and the
//! keyboard, and checked by screenshots and what their items run.

use std::path::Path;

use vaht_test_support::{
    BUTTON_LEFT, BUTTON_RIGHT, HeadlessVaht, Screenshot, TestClient, keys, solid_png, wait_for_file,
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
fn the_pointer_starts_in_the_middle_of_the_screen() {
    let files = tempfile::tempdir().expect("a temporary directory");
    let (vaht, mut client) = start(&files.path().join("touched"));
    // A menu opens at the pointer, so where it opens is where the pointer is.
    run(&vaht, "root-menu");
    let opened = client.screenshot();
    let (x, y) = (960, 540);
    let first_item = (
        x + BORDER + 5,
        y + BORDER + TITLE_HEIGHT + BORDER + ITEM_HEIGHT / 2,
    );
    assert_eq!(
        at(&opened, first_item),
        MENU_BODY,
        "the menu is at the middle of the 1920 by 1080 screen"
    );
    let corner_item = (BORDER + 5, BORDER + TITLE_HEIGHT + BORDER + ITEM_HEIGHT / 2);
    assert_ne!(at(&opened, corner_item), MENU_BODY, "and not in its corner");
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
    let png = solid_png(32, icon);
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
            ("red.png", &solid_png(32, red)),
        ],
        &[(
            "icons/hicolor/32x32/apps/vaht-test-icon.png",
            &solid_png(32, green),
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

/// The color of the icon of the program the test menu runs.
const PROGRAM_ICON: [u8; 3] = [0xe0, 0x30, 0x30];

/// The color of the icon the test menu names.
const NAMED_ICON: [u8; 3] = [0x30, 0x30, 0xe0];

/// Starts vaht with a root menu of an item running a program with a desktop
/// entry and icon, one saying it has no icon, and one naming its own;
/// `settings` are added.
fn start_with_icons(settings: &str) -> (HeadlessVaht, TestClient) {
    let menus = "[menu.root]\ntitle = \"test\"\nitems = [\n\
         { label = \"Program\", command = \"exec vaht-test-program --flag\" },\n\
         { label = \"None\", command = \"exec vaht-test-program\", icon = \"\" },\n\
         { label = \"Named\", command = \"exec true\", icon = \"vaht-named-icon\" },\n]\n\
         [menu.window]\nitems = []\n[menu.workspace]\nitems = []\n";
    let (program, named) = (solid_png(32, PROGRAM_ICON), solid_png(32, NAMED_ICON));
    let vaht = HeadlessVaht::start_with_files(
        Path::new(EXECUTABLE),
        &[
            ("menus.toml", menus.as_bytes()),
            ("settings.toml", settings.as_bytes()),
        ],
        &[
            (
                "applications/vaht-test-program.desktop",
                b"[Desktop Entry]\nName=Test\nExec=vaht-test-program %U\nIcon=vaht-program-icon\n",
            ),
            ("icons/hicolor/32x32/apps/vaht-program-icon.png", &program),
            ("icons/hicolor/32x32/apps/vaht-named-icon.png", &named),
        ],
    );
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

/// Returns the left edge of the first light pixel of the text in item
/// `index`'s row, right of the margin's start.
fn text_left(screenshot: &Screenshot, index: u32) -> u32 {
    let (left, top) = (OPENED_AT.0 + BORDER, item_point(index).1 - ITEM_HEIGHT / 2);
    (left + 19..left + 120)
        .find(|x| {
            (top..top + ITEM_HEIGHT).any(|y| {
                let [red, green, blue] = screenshot.rgb(*x, y);
                red > 200 && green > 200 && blue > 200
            })
        })
        .expect("the label is drawn")
}

#[test]
fn an_item_running_a_program_shows_its_icon_in_the_margin_and_the_text_stays_put() {
    let (_vaht, mut client) = start_with_icons("");
    client.click(OPENED_AT.0, OPENED_AT.1, BUTTON_RIGHT);
    let opened = client.screenshot();
    assert_eq!(at(&opened, item_point(0)), PROGRAM_ICON, "inferred");
    assert_eq!(
        at(&opened, item_point(1)),
        MENU_BODY,
        "`icon = \"\"` says none"
    );
    assert_eq!(at(&opened, item_point(2)), NAMED_ICON, "named");
    // The text begins at the margin, as it does for an item with no icon.
    let margin = OPENED_AT.0 + BORDER + 21;
    for index in 0..3 {
        let left = text_left(&opened, index);
        assert!(
            (margin..margin + 8).contains(&left),
            "item {index}'s text starts at the margin, not after the icon: {left}"
        );
    }
}

#[test]
fn inferring_icons_can_be_turned_off() {
    let (_vaht, mut client) = start_with_icons("[menus]\ninfer_icons = false\n");
    client.click(OPENED_AT.0, OPENED_AT.1, BUTTON_RIGHT);
    let opened = client.screenshot();
    assert_eq!(at(&opened, item_point(0)), MENU_BODY, "not inferred");
    assert_eq!(at(&opened, item_point(2)), NAMED_ICON, "a named one stays");
}

/// Starts vaht with a root menu of one item running `vaht-late-program`,
/// and the data `files`.
fn start_with_late_program(files: &[(&str, &[u8])]) -> (HeadlessVaht, TestClient) {
    let menus = "[menu.root]\ntitle = \"test\"\nitems = [\n\
         { label = \"Late\", command = \"exec vaht-late-program\" },\n]\n\
         [menu.window]\nitems = []\n[menu.workspace]\nitems = []\n";
    let vaht = HeadlessVaht::start_with_files(
        Path::new(EXECUTABLE),
        &[("menus.toml", menus.as_bytes())],
        files,
    );
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

/// Writes the file `name` in `data`, making the directories it needs.
fn install(data: &Path, name: &str, contents: &[u8]) {
    let path = data.join(name);
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("made");
    std::fs::write(path, contents).expect("written");
}

/// Opens and closes the root menu until its first item's icon is `color`,
/// or a few seconds pass; returns whether it was.
fn shows_icon_in_time(client: &mut TestClient, color: [u8; 3]) -> bool {
    for _ in 0..60 {
        client.click(OPENED_AT.0, OPENED_AT.1, BUTTON_RIGHT);
        let shown = at(&client.screenshot(), item_point(0)) == color;
        client.type_chord(&[keys::ESCAPE]);
        if shown {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

const ENTRY: &[u8] =
    b"[Desktop Entry]\nName=Late\nExec=vaht-late-program %U\nIcon=vaht-late-icon\n";

#[test]
fn a_desktop_entry_installed_while_vaht_runs_gives_its_program_an_icon() {
    let (vaht, mut client) = start_with_late_program(&[(
        "applications/other.desktop",
        b"[Desktop Entry]\nExec=other\n",
    )]);
    client.click(OPENED_AT.0, OPENED_AT.1, BUTTON_RIGHT);
    assert_eq!(
        at(&client.screenshot(), item_point(0)),
        MENU_BODY,
        "no icon yet"
    );
    client.type_chord(&[keys::ESCAPE]);
    install(
        vaht.data_directory(),
        "icons/hicolor/32x32/apps/vaht-late-icon.png",
        &solid_png(32, PROGRAM_ICON),
    );
    install(vaht.data_directory(), "applications/late.desktop", ENTRY);
    assert!(
        shows_icon_in_time(&mut client, PROGRAM_ICON),
        "found without a reload"
    );
}

#[test]
fn an_edited_desktop_entry_changes_the_icon_and_one_in_a_new_directory_counts() {
    let (vaht, mut client) = start_with_late_program(&[
        ("applications/late.desktop", ENTRY),
        (
            "icons/hicolor/32x32/apps/vaht-late-icon.png",
            &solid_png(32, PROGRAM_ICON),
        ),
        (
            "icons/hicolor/32x32/apps/vaht-other-icon.png",
            &solid_png(32, NAMED_ICON),
        ),
    ]);
    assert!(
        shows_icon_in_time(&mut client, PROGRAM_ICON),
        "the entry's icon"
    );
    // Edited in place: the entry names another icon.
    install(
        vaht.data_directory(),
        "applications/late.desktop",
        b"[Desktop Entry]\nName=Late\nExec=vaht-late-program %U\nIcon=vaht-other-icon\n",
    );
    assert!(
        shows_icon_in_time(&mut client, NAMED_ICON),
        "the edited entry's icon"
    );
    // Removed, and one made in a new subdirectory.
    std::fs::remove_file(vaht.data_directory().join("applications/late.desktop")).expect("removed");
    install(
        vaht.data_directory(),
        "applications/vendor/late.desktop",
        b"[Desktop Entry]\nName=Late\nExec=vaht-late-program %U\nIcon=vaht-late-icon\n",
    );
    assert!(
        shows_icon_in_time(&mut client, PROGRAM_ICON),
        "the entry in the new directory"
    );
}

#[test]
fn an_applications_directory_made_after_vaht_started_is_watched() {
    let (vaht, mut client) = start_with_late_program(&[(
        "icons/hicolor/32x32/apps/vaht-late-icon.png",
        &solid_png(32, PROGRAM_ICON),
    )]);
    client.click(OPENED_AT.0, OPENED_AT.1, BUTTON_RIGHT);
    assert_eq!(
        at(&client.screenshot(), item_point(0)),
        MENU_BODY,
        "no icon yet"
    );
    client.type_chord(&[keys::ESCAPE]);
    install(vaht.data_directory(), "applications/late.desktop", ENTRY);
    assert!(
        shows_icon_in_time(&mut client, PROGRAM_ICON),
        "found without a reload"
    );
}
