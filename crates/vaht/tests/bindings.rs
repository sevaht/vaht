//! Bindings tests: keys and buttons running commands, and the configuration
//! that binds them, edited while vaht runs.
//!
//! Each test gives a headless vaht its own `bindings.toml`, types on a
//! virtual keyboard or clicks a virtual pointer, and checks what the bound
//! command did: usually a program it started wrote a file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use vaht_test_support::{BUTTON_LEFT, HeadlessVaht, TestClient, WINDOW_COLOR, keys, wait_for_file};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Super+T, as typed.
const SUPER_T: [u32; 2] = [keys::LEFT_SUPER, keys::LETTER_T];

/// A directory for the files the bound programs write.
struct WrittenFiles(tempfile::TempDir);

impl WrittenFiles {
    /// Creates the directory.
    fn new() -> Self {
        Self(tempfile::tempdir().expect("a temporary directory"))
    }

    /// Returns the path of the output file `name`.
    fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }

    /// Waits until the output file `name` exists with something in it, and
    /// returns its contents.
    fn wait_for(&self, name: &str) -> String {
        wait_for_file(&self.path(name), |contents| !contents.is_empty())
    }
}

/// Returns a bindings file binding Super+T to write `text` into `path`.
fn super_t_writes(path: &Path, text: &str) -> String {
    format!(
        "[keys]\n\"Super+T\" = \"exec echo {text} > '{}'\"\n",
        path.display()
    )
}

#[test]
fn a_key_binding_starts_a_program_that_vaht_reaps() {
    let files = WrittenFiles::new();
    // `ls` lists its own descriptors: 0, 1 and 2, plus the one it reads the
    // directory with. Anything more would be inherited from vaht.
    let bindings = format!(
        "[keys]\n\"Super+T\" = \"exec ls /proc/self/fd > '{}'; \
         grep -E 'SigBlk|SigIgn' /proc/self/status > '{}'\"\n",
        files.path("descriptors").display(),
        files.path("mask").display()
    );
    let vaht =
        HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("bindings.toml", &bindings)]);
    let mut client = TestClient::connect(vaht.socket());
    client.type_chord(&SUPER_T);
    let mask = files.wait_for("mask");
    assert_eq!(
        files.wait_for("descriptors"),
        "0\n1\n2\n3\n",
        "programs must inherit no descriptors from vaht"
    );
    assert_eq!(
        mask.trim(),
        "SigBlk:\t0000000000000000\nSigIgn:\t0000000000000000",
        "programs must not inherit the signals vaht blocks or ignores"
    );
    vaht.wait_for_logged("program exited", 1);
    drop(client);
    // The client's memory pool goes with the frame after it leaves.
    vaht.wait_for_disconnections(1);
    let report = vaht.stop();
    assert_eq!(report.live_count("program"), 0, "the program was reaped");
    report.assert_nothing_left_behind();
}

#[test]
fn changing_the_bindings_file_takes_effect_and_a_broken_one_is_rejected() {
    let files = WrittenFiles::new();
    let first = super_t_writes(&files.path("result"), "first");
    let vaht = HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("bindings.toml", &first)]);
    let mut client = TestClient::connect(vaht.socket());
    let bindings_file = vaht.config_directory().join("bindings.toml");

    // Saved the way editors save: written beside, then renamed over.
    let second = super_t_writes(&files.path("result"), "second");
    let temporary = vaht.config_directory().join(".bindings.toml.swp");
    std::fs::write(&temporary, second).expect("the file is written");
    std::fs::rename(&temporary, &bindings_file).expect("the file is renamed");
    vaht.wait_for_logged("configuration reloaded", 1);
    client.type_chord(&SUPER_T);
    assert_eq!(files.wait_for("result").trim(), "second");

    std::fs::remove_file(files.path("result")).expect("the output is removed");
    std::fs::write(&bindings_file, "[keys]\n\"Super+T\" = \"explode\"\n").expect("written");
    vaht.wait_for_logged("configuration rejected", 1);
    client.type_chord(&SUPER_T);
    assert_eq!(
        files.wait_for("result").trim(),
        "second",
        "the last valid bindings stay in effect"
    );
}

#[test]
fn a_repeating_binding_repeats_while_its_key_is_held_and_stops_on_release() {
    let bindings = "[keys]\n\"Super+T\" = { repeat = \"exec true\", rate = 50 }\n";
    let vaht =
        HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("bindings.toml", bindings)]);
    let mut client = TestClient::connect(vaht.socket());
    client.press_key(keys::LEFT_SUPER);
    client.press_key(keys::LETTER_T);
    vaht.wait_for_logged("program started", 3);
    client.release_key(keys::LETTER_T);
    client.release_key(keys::LEFT_SUPER);
    // Long enough for the log to catch up, and for a repeat that failed to
    // stop to show.
    let settle = Duration::from_millis(300);
    std::thread::sleep(settle);
    let started_at_release = vaht.logged_count("program started");
    std::thread::sleep(settle);
    assert_eq!(
        vaht.logged_count("program started"),
        started_at_release,
        "no more repeats after the release"
    );
}

#[test]
fn double_clicking_the_titlebar_runs_its_binding() {
    let files = WrittenFiles::new();
    let bindings = format!(
        "[mouse.titlebar]\n\"Double+Left\" = \"exec touch '{}'\"\n\"Drag+Left\" = \"start-moving\"\n",
        files.path("double").display()
    );
    let vaht =
        HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("bindings.toml", &bindings)]);
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let content = client
        .screenshot()
        .bounds_of(WINDOW_COLOR)
        .expect("the window is on screen");
    // On the title: a little into the titlebar, which is directly above the
    // content.
    let title = (content.left + 20, content.top - 10);
    client.click(title.0, title.1, BUTTON_LEFT);
    client.click(title.0, title.1, BUTTON_LEFT);
    vaht.wait_for_logged("program exited", 1);
    assert!(files.path("double").exists());
}

#[test]
fn the_exit_action_stops_vaht_cleanly() {
    let bindings = "[keys]\n\"Super+T\" = \"exit\"\n";
    let vaht =
        HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("bindings.toml", bindings)]);
    let mut client = TestClient::connect(vaht.socket());
    client.press_key(keys::LEFT_SUPER);
    client.press_key(keys::LETTER_T);
    let report = vaht.wait_for_exit();
    assert_eq!(report.live_count("window"), 0);
}
