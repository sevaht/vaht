//! The executable, exercised the way a user runs it.
//!
//! These runs have no host compositor (the display variables are removed),
//! so starting vaht stops at choosing a backend. That exercises everything
//! up to the backend without opening windows on the developer's screen.

use std::path::Path;
use std::process::{Command, Output};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Part of the error vaht reports when it has nowhere to run.
const NO_BACKEND_MESSAGE: &str = "no host compositor to run nested in";

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
        .env_remove("DISPLAY")
        .output()
        .expect("cargo builds the executable before running integration tests")
}

/// A fresh directory for one test's logs.
fn log_directory() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary directory")
}

#[test]
fn version_reports_the_package_version() {
    let logs = log_directory();
    let output = run_executable(&["--version"], logs.path());
    assert!(output.status.success());
    let version = String::from_utf8_lossy(&output.stdout);
    let expected = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"));
    assert_eq!(version.trim(), expected);
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
