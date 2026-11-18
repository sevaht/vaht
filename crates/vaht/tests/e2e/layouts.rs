//! Layouts: a named set of presentation settings that lies under
//! `settings.toml`, swapped while vaht runs without touching the file (02 §10).
//! What a layout holds is read back through `vaht config get`'s
//! `taskbar.widgets` and `taskbar.rows`.

use std::path::Path;
use std::process::Output;

use vaht_test_support::HeadlessVaht;

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// A layout of a taskbar with just a clock and three rows.
const SLIM: &str = "[taskbar]\nwidgets = [\"clock\"]\nrows = 3\n";

/// A layout of a taskbar with a window list and one row.
const FULL: &str = "[taskbar]\nwidgets = [\"window-list\", \"clock\"]\nrows = 1\n";

/// Starts vaht with these configuration files.
fn start(files: &[(&str, &str)]) -> HeadlessVaht {
    HeadlessVaht::start_with_config(Path::new(EXECUTABLE), files)
}

/// Runs `vaht <arguments>` against `vaht`.
fn run(vaht: &HeadlessVaht, arguments: &[&str]) -> Output {
    std::process::Command::new(EXECUTABLE)
        .args(arguments)
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht runs")
}

/// Returns `vaht config get`, which must succeed.
fn settings(vaht: &HeadlessVaht) -> String {
    let output = run(vaht, &["config", "get"]);
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).expect("UTF-8")
}

#[test]
fn a_layout_supplies_the_settings_settings_toml_leaves_out() {
    let vaht = start(&[
        ("settings.toml", "layout = \"slim\"\n"),
        ("layouts/slim.toml", SLIM),
    ]);
    let shown = settings(&vaht);
    assert!(shown.contains("layout slim"), "{shown}");
    assert!(
        shown.contains("+ taskbar.rows = 3"),
        "supplied, marked +: {shown}"
    );
    assert!(shown.contains("+ taskbar.widgets = [\"clock\"]"), "{shown}");
}

#[test]
fn settings_toml_wins_over_the_layout() {
    let vaht = start(&[
        ("settings.toml", "layout = \"slim\"\n[taskbar]\nrows = 2\n"),
        ("layouts/slim.toml", SLIM),
    ]);
    let shown = settings(&vaht);
    assert!(
        shown.contains("  taskbar.rows = 2"),
        "the file's own, not marked: {shown}"
    );
    assert!(
        shown.contains("+ taskbar.widgets"),
        "the rest from the layout: {shown}"
    );
}

#[test]
fn swapping_the_layout_changes_the_presentation_and_leaves_the_file_alone() {
    let vaht = start(&[
        (
            "settings.toml",
            "# Mine.\nlayout = \"slim\"\n[focus]\nmodel = \"sloppy\"\n",
        ),
        ("layouts/slim.toml", SLIM),
        ("layouts/full.toml", FULL),
    ]);
    let file = vaht.config_directory().join("settings.toml");
    let before = std::fs::read(&file).expect("read");
    assert!(settings(&vaht).contains("taskbar.rows = 3"));
    let swapped = run(&vaht, &["run", "set-layout", "full"]);
    assert!(swapped.status.success(), "{swapped:?}");
    let shown = settings(&vaht);
    assert!(shown.contains("taskbar.rows = 1"), "{shown}");
    assert!(shown.contains("layout full"), "{shown}");
    assert!(
        shown.contains("focus.model = \"sloppy\""),
        "behavior is untouched: {shown}"
    );
    assert_eq!(
        std::fs::read(&file).expect("read"),
        before,
        "settings.toml is byte for byte the same"
    );
}

#[test]
fn a_swap_can_be_saved_and_then_the_file_names_the_layout() {
    let vaht = start(&[
        ("settings.toml", "layout = \"slim\"\n"),
        ("layouts/slim.toml", SLIM),
        ("layouts/full.toml", FULL),
    ]);
    assert!(run(&vaht, &["run", "set-layout", "full"]).status.success());
    assert!(run(&vaht, &["config", "save"]).status.success());
    let written =
        std::fs::read_to_string(vaht.config_directory().join("settings.toml")).expect("read");
    assert!(written.contains("layout = \"full\""), "{written}");
}

#[test]
fn editing_a_layout_file_changes_the_settings_at_once() {
    let vaht = start(&[
        ("settings.toml", "layout = \"slim\"\n"),
        ("layouts/slim.toml", SLIM),
    ]);
    assert!(settings(&vaht).contains("taskbar.rows = 3"));
    std::fs::write(
        vaht.config_directory().join("layouts").join("slim.toml"),
        "[taskbar]\nrows = 2\n",
    )
    .expect("edited");
    vaht.wait_for_logged("configuration reloaded", 1);
    assert!(settings(&vaht).contains("taskbar.rows = 2"));
}

#[test]
fn behavior_in_a_layout_is_left_out_and_reported_against_the_layout() {
    let vaht = start(&[
        ("settings.toml", "layout = \"bad\"\n"),
        (
            "layouts/bad.toml",
            "[taskbar]\nrows = 2\n[focus]\nmodel = \"sloppy\"\n",
        ),
    ]);
    let shown = settings(&vaht);
    assert!(
        shown.contains("taskbar.rows = 2"),
        "the presentation is used: {shown}"
    );
    assert!(
        shown.contains("focus.model = \"click\""),
        "the behavior is not: {shown}"
    );
    let files = String::from_utf8(run(&vaht, &["config", "files"]).stdout).expect("UTF-8");
    assert!(
        files.contains("layouts/bad.toml: focus"),
        "blamed on the layout: {files}"
    );
}

#[test]
fn the_layouts_there_are_listed_and_a_layout_that_is_missing_is_reported() {
    let vaht = start(&[
        ("settings.toml", "layout = \"nowhere\"\n"),
        ("layouts/slim.toml", SLIM),
    ]);
    let files = String::from_utf8(run(&vaht, &["config", "files"]).stdout).expect("UTF-8");
    assert!(files.contains("layouts: slim"), "{files}");
    assert!(
        files.contains("layout nowhere"),
        "the missing one is a problem: {files}"
    );
}

#[test]
fn a_layout_cannot_be_chosen_that_does_not_exist() {
    let vaht = start(&[("layouts/slim.toml", SLIM)]);
    let refused = run(&vaht, &["config", "set", "layout", "nowhere"]);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("nowhere"),
        "{refused:?}"
    );
}

/// A theme of the slim layout.
const CALM: &str = "layout = \"slim\"\n";

#[test]
fn a_theme_brings_its_layout_and_settings_toml_still_wins() {
    let vaht = start(&[
        ("settings.toml", "theme = \"calm\"\n"),
        ("themes/calm.toml", CALM),
        ("layouts/slim.toml", SLIM),
    ]);
    let shown = settings(&vaht);
    assert!(shown.contains("taskbar.rows = 3"), "{shown}");
    assert!(shown.contains("layout slim"), "{shown}");
}

#[test]
fn choosing_a_theme_puts_back_the_style_and_layout_that_would_win_over_it() {
    let vaht = start(&[
        ("settings.toml", "theme = \"calm\"\nlayout = \"full\"\n"),
        ("themes/calm.toml", CALM),
        ("layouts/slim.toml", SLIM),
        ("layouts/full.toml", FULL),
    ]);
    assert!(
        settings(&vaht).contains("taskbar.rows = 1"),
        "the file's layout wins"
    );
    let chosen = run(&vaht, &["run", "set-theme", "calm"]);
    assert!(chosen.status.success(), "{chosen:?}");
    assert!(
        settings(&vaht).contains("taskbar.rows = 3"),
        "now the theme's"
    );
    let files = String::from_utf8(run(&vaht, &["config", "files"]).stdout).expect("UTF-8");
    assert!(files.contains("themes: calm"), "{files}");
}
