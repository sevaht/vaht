//! The executable, exercised the way a user runs it.

use std::process::{Command, Output};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Runs the executable with `arguments` and returns what it produced.
fn run_executable(arguments: &[&str]) -> Output {
    Command::new(EXECUTABLE)
        .args(arguments)
        .output()
        .expect("cargo builds the executable before running integration tests")
}

#[test]
fn version_reports_the_package_version() {
    let output = run_executable(&["--version"]);
    assert!(output.status.success());
    let version = String::from_utf8_lossy(&output.stdout);
    let expected = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"));
    assert_eq!(version.trim(), expected);
}

#[test]
fn verbosity_options_are_mutually_exclusive() {
    let output = run_executable(&["--verbose", "--quiet"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "clap's exit code for a usage error"
    );
}

#[test]
fn runs_successfully_without_arguments() {
    assert!(run_executable(&[]).status.success());
}
