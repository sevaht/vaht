//! A configuration that is not all valid: vaht uses what it can and says so
//! on every output (02 §2). A setting that is not valid is the default, a
//! file that is not is the one in effect (or the built-in one), and nothing
//! else is lost; a red banner names what was left out until the files are
//! fixed or the banner is clicked.

use std::path::Path;
use std::process::Output;

use vaht_test_support::{BUTTON_LEFT, HeadlessVaht, Screenshot, TestClient};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// A settings file with a good setting, a setting with a value out of range
/// and a key that is not a setting at all.
const MOSTLY_VALID: &str = "[workspaces]\nnames = [\"one\", \"two\", \"three\"]\n\
                            [blur]\npasses = 99\n[taskbar]\nnonsense = true\n";

/// Starts vaht with `settings` as its `settings.toml`.
fn start(settings: &str) -> HeadlessVaht {
    HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("settings.toml", settings)])
}

/// Runs `vaht <arguments>` against `vaht`.
fn message(vaht: &HeadlessVaht, arguments: &[&str]) -> Output {
    std::process::Command::new(EXECUTABLE)
        .args(arguments)
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht runs")
}

/// Runs `vaht config <arguments>`, which must succeed.
fn config(vaht: &HeadlessVaht, arguments: &[&str]) -> String {
    let output = message(vaht, &[&["config"], arguments].concat());
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).expect("UTF-8")
}

/// Returns whether the pixel at `x`, `y` is the banner's red.
fn is_red(screenshot: &Screenshot, x: u32, y: u32) -> bool {
    let [red, green, blue] = screenshot.rgb(x, y);
    red > 100 && green < 40 && blue < 40
}

/// Returns whether the banner is drawn: red along the top of the output,
/// in the middle.
fn shows_banner(client: &mut TestClient) -> bool {
    let screenshot = client.screenshot();
    let x = screenshot.width() / 2;
    // The middle of the first line of the banner, which is 12 pixels down.
    (14..40).any(|y| is_red(&screenshot, x, y))
}

#[test]
fn a_setting_that_is_not_valid_leaves_the_rest_in_effect_and_is_reported() {
    let vaht = start(MOSTLY_VALID);
    let files = config(&vaht, &["files"]);
    assert!(files.contains("blur.passes"), "{files}");
    assert!(files.contains("taskbar.nonsense"), "{files}");
    assert!(files.contains("2 problem(s)"), "{files}");
    let settings = config(&vaht, &["get"]);
    assert!(
        settings.contains("workspaces.names = [\"one\",\"two\",\"three\"]"),
        "the valid setting is in effect: {settings}"
    );
    assert!(
        settings.contains("  blur.passes = 3\n"),
        "the one that is not is the default: {settings}"
    );
}

#[test]
fn a_valid_configuration_reports_nothing_and_shows_no_banner() {
    let vaht = start("[workspaces]\nnames = [\"one\"]\n");
    assert!(!config(&vaht, &["files"]).contains("problem"));
    let mut client = TestClient::connect(vaht.socket());
    assert!(!shows_banner(&mut client));
}

#[test]
fn the_problems_are_drawn_in_red_across_the_top_until_fixed() {
    let vaht = start(MOSTLY_VALID);
    let mut client = TestClient::connect(vaht.socket());
    assert!(shows_banner(&mut client), "a red banner at the top");
    std::fs::write(
        vaht.config_directory().join("settings.toml"),
        "[workspaces]\nnames = [\"one\", \"two\", \"three\"]\n",
    )
    .expect("the file is fixed");
    vaht.wait_for_logged("configuration reloaded", 1);
    assert!(!shows_banner(&mut client), "gone once the files are valid");
}

#[test]
fn a_click_hides_the_banner_until_the_problems_change() {
    let vaht = start(MOSTLY_VALID);
    let mut client = TestClient::connect(vaht.socket());
    assert!(shows_banner(&mut client));
    let screenshot = client.screenshot();
    client.click(screenshot.width() / 2, 24, BUTTON_LEFT);
    assert!(!shows_banner(&mut client), "hidden by the click");
    // The same problems again (the file saved without a change): still hidden.
    let path = vaht.config_directory().join("settings.toml");
    std::fs::write(&path, format!("{MOSTLY_VALID}\n")).expect("saved");
    vaht.wait_for_logged("configuration reloaded", 1);
    assert!(!shows_banner(&mut client), "the same problems stay hidden");
    // A different problem is news.
    std::fs::write(&path, "[blur]\npasses = 77\n").expect("changed");
    vaht.wait_for_logged("configuration reloaded", 2);
    assert!(shows_banner(&mut client), "a new problem shows again");
}

#[test]
fn a_file_that_is_not_toml_keeps_what_was_in_effect_and_the_others_are_used() {
    let vaht = start("[workspaces]\nnames = [\"one\", \"two\"]\n");
    let path = vaht.config_directory().join("settings.toml");
    std::fs::write(&path, "[workspaces\nnames = ").expect("broken");
    vaht.wait_for_logged("the configuration has a problem", 1);
    assert!(
        config(&vaht, &["get"]).contains("workspaces.names = [\"one\",\"two\"]"),
        "what the file said when it last worked"
    );
    assert!(config(&vaht, &["files"]).contains("settings.toml"));
}

#[test]
fn settings_can_be_changed_while_the_files_have_problems_but_a_bad_change_is_refused() {
    let vaht = start(MOSTLY_VALID);
    let output = message(&vaht, &["config", "set", "focus.model", "sloppy"]);
    assert!(
        output.status.success(),
        "problems the files already had do not stop a change: {output:?}"
    );
    assert!(config(&vaht, &["get"]).contains("focus.model = \"sloppy\""));
    let output = message(&vaht, &["config", "set", "blur.spread", "999"]);
    assert!(
        !output.status.success(),
        "a change that is itself bad is refused"
    );
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("spread"), "it says which setting: {error}");
}
