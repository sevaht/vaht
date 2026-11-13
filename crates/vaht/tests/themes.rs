//! Theme tests: `themes/<name>.toml` named in `settings.toml` changing how
//! frames look, read at start and again when the file changes, checked by
//! screenshots.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, Screenshot, TestClient, WINDOW_COLOR};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Settings that choose the theme `mine`.
const SETTINGS: &str = "theme = \"mine\"\n";

/// The built-in theme's border color, `grey60`.
const BUILT_IN_BORDER: [u8; 3] = [153, 153, 153];

/// A theme whose focused titlebar is solid `color`, `height` pixels tall,
/// with centered titles, under a black border `border` pixels wide.
fn theme(color: &str, height: u32, border: u32) -> String {
    format!(
        "[default]\njustify = \"center\"\n\
         [state.focused]\ntexture = {{ solid = \"{color}\" }}\n\
         [window.titlebar]\nheight = {height}\n\
         [window.border]\nborder = {{ width = {border}, color = \"black\" }}\n"
    )
}

/// Returns the color of the titlebar just above the window's content, at
/// its left end where no title is, and its height: the rows between the
/// border line under it and the one over it, both of `border_color`.
fn titlebar_above_content(screenshot: &Screenshot, border_color: [u8; 3]) -> ([u8; 3], u32) {
    let content = screenshot
        .bounds_of(WINDOW_COLOR)
        .expect("the window is shown");
    let x = content.left + 1;
    let below_titlebar = (0..content.top)
        .rev()
        .find(|&y| screenshot.rgb(x, y) != border_color)
        .expect("a titlebar above the content");
    let height = (0..=below_titlebar)
        .rev()
        .take_while(|&y| screenshot.rgb(x, y) != border_color)
        .count();
    (
        screenshot.rgb(x, below_titlebar),
        u32::try_from(height).expect("a titlebar is short"),
    )
}

#[test]
fn the_theme_the_settings_name_styles_the_frames() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", SETTINGS),
            ("themes/mine.toml", &theme("#ff00ff", 30, 2)),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let (color, height) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [0xff, 0x00, 0xff]);
    assert_eq!(height, 30);
}

#[test]
fn editing_the_theme_restyles_the_open_windows() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", SETTINGS),
            ("themes/mine.toml", &theme("#ff00ff", 30, 1)),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let reloads = vaht.logged_count("configuration reloaded");
    std::fs::write(
        vaht.config_directory().join("themes/mine.toml"),
        theme("#00ffff", 24, 1),
    )
    .expect("the theme is rewritten");
    vaht.wait_for_logged("configuration reloaded", reloads + 1);
    let (color, height) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [0x00, 0xff, 0xff]);
    assert_eq!(height, 24);
}

#[test]
fn without_a_theme_named_the_built_in_one_applies() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let (_, height) = titlebar_above_content(&client.screenshot(), BUILT_IN_BORDER);
    assert_eq!(height, 20, "the built-in titlebar is 20 pixels tall");
}

#[test]
fn set_theme_names_the_theme_in_the_settings_and_vaht_takes_it() {
    let settings = "# My settings.\n[focus]\nmodel = \"click\"\n";
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", settings),
            ("themes/mine.toml", &theme("red", 20, 1)),
        ],
    );
    let output = std::process::Command::new(EXECUTABLE)
        .args(["msg", "run", "set-theme", "mine"])
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht msg runs");
    assert!(output.status.success(), "{output:?}");
    vaht.wait_for_logged("configuration reloaded", 1);
    let written = std::fs::read_to_string(vaht.config_directory().join("settings.toml"))
        .expect("the settings are there");
    assert!(written.starts_with("# My settings.\n"), "{written}");
    assert!(written.contains("theme = \"mine\"\n"), "{written}");
    assert!(written.contains("model = \"click\""), "{written}");
}
