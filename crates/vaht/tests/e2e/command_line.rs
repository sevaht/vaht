//! The executable, exercised the way a user runs it.
//!
//! These runs have no host compositor (the display variables are removed),
//! so starting vaht goes for the hardware, and libseat is pointed at a
//! backend that does not exist, so taking the seat fails without touching
//! the developer's seat. That exercises everything up to the backend
//! without opening windows on the developer's screen or taking it over.

use std::path::Path;
use std::process::{Command, Output};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Part of the error vaht reports when it has nowhere to run.
const NO_BACKEND_MESSAGE: &str = "could not take the seat";

/// Runs the executable with `arguments`, logging only into `log_directory`
/// so the tests never touch the real state directory, the journal or the
/// screen.
fn run_executable(arguments: &[&str], log_directory: &Path) -> Output {
    Command::new(EXECUTABLE)
        .args(arguments)
        .env("VAHT_LOG_DIRECTORY", log_directory)
        .env("VAHT_JOURNALD", "off")
        .env_remove("VAHT_LOG")
        .env_remove("WAYLAND_DISPLAY")
        // Nor a running vaht's socket: the commands that ask one must find
        // none here, not the vaht of whoever runs the tests.
        .env_remove("VAHT_SOCKET")
        .env_remove("DISPLAY")
        // A libseat backend that does not exist: the seat is never taken.
        .env("LIBSEAT_BACKEND", "vaht-test-none")
        .output()
        .expect("cargo builds the executable before running integration tests")
}

/// A fresh directory for one test's logs.
fn log_directory() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary directory")
}

#[test]
fn version_reports_the_package_version_and_commit() {
    let logs = log_directory();
    let output = run_executable(&["--version"], logs.path());
    assert!(output.status.success());
    let version = String::from_utf8_lossy(&output.stdout);
    let expected = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"), " (");
    assert!(version.starts_with(expected), "{version}");
    // The commit it was built from (build.rs): what launch.sh compares.
    let commit = version
        .trim()
        .strip_prefix(expected)
        .and_then(|rest| rest.strip_suffix(')'))
        .expect("a commit in parentheses");
    let hash = commit.strip_suffix("-modified").unwrap_or(commit);
    assert!(
        hash == "unknown"
            || (hash.len() == 12 && hash.chars().all(|digit| digit.is_ascii_hexdigit())),
        "{commit}"
    );
}

#[test]
fn verbosity_options_are_mutually_exclusive() {
    let logs = log_directory();
    let output = run_executable(&["--verbose", "--quiet"], logs.path());
    assert_eq!(
        output.status.code(),
        Some(2),
        "clap's exit code for a usage error"
    );
}

#[test]
fn without_a_host_compositor_vaht_logs_why_it_cannot_start() {
    let logs = log_directory();
    assert!(!run_executable(&[], logs.path()).status.success());
    let log = std::fs::read_to_string(logs.path().join("vaht.log")).expect("the log file exists");
    assert!(log.contains("vaht starting"), "log was: {log}");
    assert!(log.contains(NO_BACKEND_MESSAGE), "log was: {log}");
}

#[test]
fn an_invalid_filter_fails_before_anything_runs() {
    let logs = log_directory();
    let output = run_executable(&["--log", "vaht=loud"], logs.path());
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid log filter"));
}

#[test]
fn with_no_log_file_or_journal_messages_go_to_standard_error() {
    let logs = log_directory();
    let blocker = logs.path().join("not-a-directory");
    std::fs::write(&blocker, "").expect("writable");
    let output = run_executable(&[], &blocker.join("logs"));
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(
        errors.contains("running without a log file"),
        "stderr was: {errors}"
    );
    assert!(errors.contains(NO_BACKEND_MESSAGE), "stderr was: {errors}");
}

/// Runs `vaht config check` (with `extra` arguments) on a configuration directory
/// holding `files`, each a path in it and its contents.
fn check(files: &[(&str, &str)], extra: &[&str]) -> Output {
    let config = tempfile::tempdir().expect("a temporary directory");
    for (name, contents) in files {
        let path = config.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the file's directory is made");
        }
        std::fs::write(path, contents).expect("written");
    }
    let logs = log_directory();
    let mut arguments = vec!["config", "check", "--config-directory"];
    let directory = config.path().to_str().expect("temporary paths are UTF-8");
    arguments.push(directory);
    arguments.extend_from_slice(extra);
    let output = run_executable(&arguments, logs.path());
    assert!(
        std::fs::read_dir(logs.path())
            .expect("the log directory exists")
            .next()
            .is_none(),
        "vaht config check must not touch the compositor's log"
    );
    output
}

#[test]
fn check_accepts_a_valid_configuration() {
    let output = check(
        &[("bindings.toml", "[keys]\n\"Super+T\" = \"exec kitty\"\n")],
        &[],
    );
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("is valid"));
}

#[test]
fn check_names_the_file_line_and_column_of_a_problem() {
    let output = check(
        &[(
            "bindings.toml",
            "[keys]\n\"Super+T\" = \"exec kitty\"\n\"Super+Q\" = \"quit\"\n",
        )],
        &[],
    );
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("bindings.toml:3:"), "{error}");
    assert!(error.contains("unknown action `quit`"), "{error}");
}

#[test]
fn check_reads_the_style_the_settings_name() {
    let output = check(
        &[
            ("settings.toml", "style = \"mine\"\n"),
            (
                "styles/mine.toml",
                "[window.titlebar]\nheight = 30\n[window.lable]\nheight = 1\n",
            ),
        ],
        &[],
    );
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("styles/mine.toml:3:"), "{error}");
    assert!(error.contains("`window.lable` is no element"), "{error}");
    let missing = check(&[("settings.toml", "style = \"absent\"\n")], &[]);
    let error = String::from_utf8_lossy(&missing.stderr);
    assert!(error.contains("no style `absent`"), "{error}");
}

#[test]
fn check_prints_the_built_in_files() {
    let output = check(&[], &["--print-defaults"]);
    assert!(output.status.success());
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(printed.contains("# ===== bindings.toml ====="), "{printed}");
    assert!(printed.contains("\"Alt+F4\" = \"close\""), "{printed}");
    assert!(printed.contains("the built-in style ====="), "{printed}");
}

#[test]
fn import_converts_a_fluxbox_setup_and_reports_what_it_left_out() {
    let setup = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(
        setup.path().join("init"),
        "session.screen0.focusModel: MouseFocus\nsession.styleFile: styles/Night Sky\n\
         session.screen0.toolbar.alpha: 200\n",
    )
    .expect("written");
    std::fs::create_dir_all(setup.path().join("styles/Night Sky")).expect("made");
    std::fs::write(
        setup.path().join("styles/Night Sky/theme.cfg"),
        "*.color: SteelBlue\nwindow.title.height: 24\nslit.color: black\n",
    )
    .expect("written");
    std::fs::write(
        setup.path().join("keys"),
        "Mod4 T :Exec kitty\nMod4 Tab :NextTab\nMod4 x Mod4 t :Exec xterm\n\
         Mod4 m :KeyMode moving\nmoving: Left :MoveLeft 10\n",
    )
    .expect("written");
    let target = tempfile::tempdir().expect("a temporary directory");
    let logs = log_directory();
    let path = |directory: &tempfile::TempDir| {
        directory
            .path()
            .to_str()
            .expect("temporary paths are UTF-8")
            .to_owned()
    };
    let output = run_executable(
        &[
            "import",
            "fluxbox",
            "--from",
            &path(&setup),
            "--to",
            &path(&target),
        ],
        logs.path(),
    );
    assert!(output.status.success(), "{output:?}");
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(printed.contains("keys:2: dropped"), "{printed}");
    let settings = std::fs::read_to_string(target.path().join("settings.toml")).expect("written");
    assert!(settings.contains("model = \"sloppy\""), "{settings}");
    assert!(settings.contains("style = \"Night-Sky\""), "{settings}");
    let style =
        std::fs::read_to_string(target.path().join("styles/Night-Sky.toml")).expect("written");
    assert!(style.contains("SteelBlue"), "{style}");
    assert!(style.contains("height = 24"), "{style}");
    assert!(
        style.contains("alpha = 200"),
        "the toolbar's alpha: {style}"
    );
    assert!(
        printed.contains("slit.color: the slit is gone"),
        "{printed}"
    );
    assert!(
        std::fs::read_dir(setup.path()).expect("readable").count() == 3,
        "the source is untouched"
    );
    let bindings = std::fs::read_to_string(target.path().join("bindings.toml")).expect("written");
    assert!(bindings.contains("[key_modes.moving]"), "{bindings}");
    let checked = run_executable(
        &["config", "check", "--config-directory", &path(&target)],
        logs.path(),
    );
    assert!(
        checked.status.success(),
        "what import writes is valid: {checked:?}"
    );
}

#[test]
fn ipc_schema_prints_the_ipc_protocols_json_schema() {
    let logs = log_directory();
    let output = run_executable(&["ipc-schema"], logs.path());
    assert!(output.status.success(), "{output:?}");
    let schema: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert!(schema["$defs"]["Request"].is_object(), "{schema}");
}

#[test]
fn config_schema_describes_the_settings_without_a_running_vaht() {
    let logs = log_directory();
    let output = run_executable(&["config", "schema"], logs.path());
    assert!(output.status.success(), "{output:?}");
    let schema: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON");
    let passes = &schema["$defs"]["BlurPasses"];
    assert_eq!(
        (&passes["minimum"], &passes["maximum"]),
        (&1.into(), &6.into())
    );
    let other = run_executable(&["config", "schema", "bindings"], logs.path());
    assert!(
        !other.status.success(),
        "a file with no schema yet is refused"
    );
    assert!(
        String::from_utf8_lossy(&other.stderr).contains("bindings.toml"),
        "{other:?}"
    );
}

#[test]
fn the_commands_that_ask_the_running_vaht_fail_plainly_without_one() {
    let logs = log_directory();
    let output = run_executable(&["windows"], logs.path());
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("VAHT_SOCKET"),
        "it says what it needs: {output:?}"
    );
    let removed = run_executable(&["msg", "windows"], logs.path());
    assert!(!removed.status.success(), "there is no `msg` any more");
}
