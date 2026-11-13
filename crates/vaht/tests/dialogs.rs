//! Dialog tests: the run dialog, shown by a binding, typed into on a
//! virtual keyboard, running what is typed through vaht.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, Screenshot, TestClient, keys, wait_for_file};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Super+T, bound to the run dialog.
const SUPER_T: [u32; 2] = [keys::LEFT_SUPER, keys::LETTER_T];

/// Starts vaht with Super+T showing the run dialog, and shows it.
fn start_with_dialog() -> (HeadlessVaht, TestClient) {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("bindings.toml", "[keys]\n\"Super+T\" = \"run-dialog\"\n")],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.type_chord(&SUPER_T);
    // Placed once it has drawn; it has the keyboard from then on.
    vaht.wait_for_logged("placed first", 1);
    (vaht, client)
}

/// Returns how many pixels of `screenshot` are pure white.
fn white_pixels(screenshot: &Screenshot) -> usize {
    let (width, height) = (screenshot.width(), screenshot.height());
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|(x, y)| screenshot.rgb(*x, *y) == [255, 255, 255])
        .count()
}

#[test]
fn the_run_dialog_runs_what_is_typed_and_remembers_it() {
    let files = tempfile::tempdir().expect("a temporary directory");
    let marker = files.path().join("ran");
    let (vaht, mut client) = start_with_dialog();
    let line = format!("touch {}", marker.display());
    client.type_text(&line);
    client.type_chord(&[keys::ENTER]);
    wait_for_file(&marker, |_| true);
    vaht.wait_for_logged("window closed", 1);
    let history = vaht.state_directory().join("vaht/run_history");
    assert_eq!(
        std::fs::read_to_string(history).expect("the history is saved"),
        format!("{line}\n")
    );
}

#[test]
fn the_run_dialog_is_a_white_line_in_the_built_in_theme() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("bindings.toml", "[keys]\n\"Super+T\" = \"run-dialog\"\n")],
    );
    let mut client = TestClient::connect(vaht.socket());
    let before = white_pixels(&client.screenshot());
    client.type_chord(&SUPER_T);
    vaht.wait_for_logged("placed first", 1);
    let shown = white_pixels(&client.screenshot());
    // 200 pixels wide, a line of 12-point text tall, less the cursor.
    assert!(
        shown >= before + 200 * 20 - 30,
        "{before} white pixels before, {shown} after"
    );
}

#[test]
fn escape_closes_the_run_dialog_without_running_anything() {
    let (vaht, mut client) = start_with_dialog();
    client.type_text("true");
    client.type_chord(&[keys::ESCAPE]);
    vaht.wait_for_logged("window closed", 1);
    assert!(
        !vaht.state_directory().join("vaht/run_history").exists(),
        "nothing ran, so nothing is remembered"
    );
    // The dialog's program has gone too, so both clients disconnect.
    drop(client);
    vaht.wait_for_disconnections(2);
    let report = vaht.stop();
    report.assert_nothing_left_behind();
}

#[test]
fn the_command_dialog_runs_a_vaht_command() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[(
            "bindings.toml",
            "[keys]\n\"Super+T\" = \"command-dialog\"\n",
        )],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.type_chord(&SUPER_T);
    vaht.wait_for_logged("placed first", 1);
    client.type_text("workspace 2");
    client.type_chord(&[keys::ENTER]);
    vaht.wait_for_logged("workspace shown", 1);
    vaht.wait_for_logged("window closed", 1);
}

#[test]
fn the_title_dialog_retitles_the_window() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[(
            "bindings.toml",
            "[keys]\n\"Super+T\" = \"set-title-dialog\"\n",
        )],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    client.type_chord(&SUPER_T);
    vaht.wait_for_logged("placed first", 2);
    // Ctrl+U clears the title shown; the new one replaces it.
    client.type_chord(&[keys::LEFT_CONTROL, keys::typing('u').expect("a key").0]);
    client.type_text("notes");
    client.type_chord(&[keys::ENTER]);
    vaht.wait_for_logged("window closed", 1);
    let listed = std::process::Command::new(EXECUTABLE)
        .args(["msg", "windows"])
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht msg runs");
    let listed = String::from_utf8(listed.stdout).expect("UTF-8");
    assert!(listed.trim_end().ends_with("\tnotes"), "{listed}");
}

#[test]
fn what_a_dialog_writes_to_its_errors_reaches_vahts_log() {
    // Not valid, so vaht runs on its built-in configuration, and the
    // dialog says so as it starts.
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", "[nonsense]\n")],
    );
    let mut client = TestClient::connect(vaht.socket());
    let shown = std::process::Command::new(EXECUTABLE)
        .args(["msg", "run", "run-dialog"])
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .status()
        .expect("vaht msg runs");
    assert!(shown.success());
    vaht.wait_for_logged("placed first", 1);
    vaht.wait_for_logged("dialog=\"run\"", 1);
    assert!(vaht.logged_count("using the built-in theme") >= 1);
    client.type_chord(&[keys::ESCAPE]);
    vaht.wait_for_logged("window closed", 1);
}

#[test]
fn vahts_own_dialogs_run_commands_when_other_programs_may_not() {
    let files = tempfile::tempdir().expect("a temporary directory");
    let marker = files.path().join("ran");
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[
            ("bindings.toml", "[keys]\n\"Super+T\" = \"run-dialog\"\n"),
            ("settings.toml", "[ipc]\nallow_commands = false\n"),
        ],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.type_chord(&SUPER_T);
    vaht.wait_for_logged("placed first", 1);
    client.type_text(&format!("touch {}", marker.display()));
    client.type_chord(&[keys::ENTER]);
    wait_for_file(&marker, |_| true);
}
