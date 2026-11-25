//! Style tests: `styles/<name>.toml` named in `settings.toml` changing how
//! frames look, read at start and again when the file changes, checked by
//! screenshots.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, Screenshot, TestClient, WINDOW_COLOR};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Settings that choose the style `mine`.
const SETTINGS: &str = "style = \"mine\"\n";

/// The built-in style's border color, `grey60`.
const BUILT_IN_BORDER: [u8; 3] = [153, 153, 153];

/// A style whose focused titlebar is solid `color`, `height` pixels tall,
/// with centered titles, under a black border `border` pixels wide.
fn style(color: &str, height: u32, border: u32) -> String {
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
fn the_style_the_settings_name_styles_the_frames() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", SETTINGS),
            ("styles/mine.toml", &style("#ff00ff", 30, 2)),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let (color, height) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [0xff, 0x00, 0xff]);
    assert_eq!(height, 30);
}

#[test]
fn editing_the_style_restyles_the_open_windows() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", SETTINGS),
            ("styles/mine.toml", &style("#ff00ff", 30, 1)),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let reloads = vaht.logged_count("configuration reloaded");
    std::fs::write(
        vaht.config_directory().join("styles/mine.toml"),
        style("#00ffff", 24, 1),
    )
    .expect("the style is rewritten");
    vaht.wait_for_logged("configuration reloaded", reloads + 1);
    let (color, height) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [0x00, 0xff, 0xff]);
    assert_eq!(height, 24);
}

#[test]
fn without_a_style_named_the_built_in_one_applies() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let (_, height) = titlebar_above_content(&client.screenshot(), BUILT_IN_BORDER);
    assert_eq!(height, 20, "the built-in titlebar is 20 pixels tall");
}

#[test]
fn set_style_and_set_setting_apply_in_place_and_save_settings_writes_them() {
    let settings = "# My settings.\n[focus]\nmodel = \"click\"\n";
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", settings),
            ("styles/mine.toml", &style("#ff00ff", 30, 2)),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let run = |command: &str| {
        let output = std::process::Command::new(EXECUTABLE)
            .args(["run", command])
            .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
            .output()
            .expect("vaht runs");
        assert!(output.status.success(), "{output:?}");
    };
    run("set-style mine");
    let (color, _) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [255, 0, 255], "the style is in use at once");
    run("set-setting focus.model sloppy");
    run("set-setting taskbar.auto_hide_delay_ms 250");
    let file = vaht.config_directory().join("settings.toml");
    let read = || std::fs::read_to_string(&file).expect("the settings are there");
    assert_eq!(read(), settings, "nothing is written yet");
    run("save-settings");
    vaht.wait_for_logged("configuration reloaded", 1);
    let written = read();
    assert!(written.starts_with("# My settings.\n"), "{written}");
    assert!(written.contains("style = \"mine\"\n"), "{written}");
    assert!(written.contains("model = \"sloppy\""), "{written}");
    assert!(
        written.contains("[taskbar]\nauto_hide_delay_ms = 250\n"),
        "{written}"
    );
    let (color, _) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [255, 0, 255], "and still after the reload");
}

/// A style with a color-scheme option: the focused titlebar is `$face`, a
/// choice of `scheme`.
const OPTIONAL: &str = "\
[options.scheme]\ndefault = \"magenta\"\n\
[options.scheme.magenta]\nface = \"#ff00ff\"\n\
[options.scheme.cyan]\nface = \"#00ffff\"\n\
[default]\njustify = \"center\"\n\
[state.focused]\ntexture = { solid = \"$face\" }\n\
[window.titlebar]\nheight = 30\n\
[window.border]\nborder = { width = 2, color = \"black\" }\n";

/// Runs `vaht <arguments>` against `vaht`, which must succeed; returns what
/// it printed.
fn print(vaht: &HeadlessVaht, arguments: &[&str]) -> String {
    let output = std::process::Command::new(EXECUTABLE)
        .args(arguments)
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht runs");
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).expect("UTF-8")
}

#[test]
fn a_choice_of_a_styles_option_is_made_in_settings_toml_or_in_place() {
    let settings = "style = \"mine\"\n[style_options]\nscheme = \"cyan\"\n";
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", settings), ("styles/mine.toml", OPTIONAL)],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let (color, _) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [0, 255, 255], "the settings' choice");
    let files = print(&vaht, &["config", "files"]);
    assert!(
        files.contains("style option scheme: magenta, *cyan"),
        "{files}"
    );
    // A choice made in place applies at once and leaves the file alone.
    print(&vaht, &["run", "set-setting style_options.scheme magenta"]);
    let (color, _) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [255, 0, 255], "the choice in place");
    let file = vaht.config_directory().join("settings.toml");
    assert_eq!(
        std::fs::read_to_string(file).expect("the settings are there"),
        settings,
        "nothing is written until save-settings"
    );
}

#[test]
fn every_option_is_put_back_to_its_default_at_once_in_place() {
    let settings = "style = \"mine\"\n[style_options]\nscheme = \"cyan\"\n";
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", settings), ("styles/mine.toml", OPTIONAL)],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let (color, _) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [0, 255, 255], "the settings' choice");
    print(&vaht, &["run", "unset-setting style_options"]);
    let (color, _) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [255, 0, 255], "the style's default, magenta");
    let file = vaht.config_directory().join("settings.toml");
    assert_eq!(
        std::fs::read_to_string(file).expect("the settings are there"),
        settings,
        "nothing is written until save-settings"
    );
}

#[test]
fn a_choice_in_the_shared_library_can_be_made_and_wins_over_the_styles_own() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", "style = \"mine\"\n"),
            ("styles/mine.toml", OPTIONAL),
            ("options/scheme/lime.toml", "face = \"#00ff00\"\n"),
            ("options/scheme/cyan.toml", "face = \"#0000ff\"\n"),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let files = print(&vaht, &["config", "files"]);
    assert!(
        files.contains("style option scheme: *magenta, cyan, lime"),
        "{files}"
    );
    print(&vaht, &["run", "set-setting style_options.scheme lime"]);
    let (color, _) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [0, 255, 0], "a choice only the library has");
    print(&vaht, &["run", "set-setting style_options.scheme cyan"]);
    let (color, _) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(
        color,
        [0, 0, 255],
        "the library's wins over the style's own"
    );
}

#[test]
fn a_choice_nothing_has_is_reported_and_the_default_used() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            (
                "settings.toml",
                "style = \"mine\"\n[style_options]\nscheme = \"nowhere\"\n",
            ),
            ("styles/mine.toml", OPTIONAL),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let (color, _) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [255, 0, 255], "the default choice");
    let files = print(&vaht, &["config", "files"]);
    assert!(files.contains("nowhere"), "named as a problem: {files}");
}

#[test]
fn editing_a_choice_file_in_the_library_restyles_the_open_windows() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            (
                "settings.toml",
                "style = \"mine\"\n[style_options]\nscheme = \"lime\"\n",
            ),
            ("styles/mine.toml", OPTIONAL),
            ("options/scheme/lime.toml", "face = \"#00ff00\"\n"),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let (color, _) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [0, 255, 0]);
    let reloads = vaht.logged_count("configuration reloaded");
    std::fs::write(
        vaht.config_directory().join("options/scheme/lime.toml"),
        "face = \"#ffff00\"\n",
    )
    .expect("the choice is rewritten");
    vaht.wait_for_logged("configuration reloaded", reloads + 1);
    let (color, _) = titlebar_above_content(&client.screenshot(), [0, 0, 0]);
    assert_eq!(color, [255, 255, 0]);
}
