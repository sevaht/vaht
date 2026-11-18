//! Records which commit vaht is built from, for `vaht --version`: the
//! installed vaht can then be told apart from the checkout (`launch.sh`
//! warns when they differ) and named in a bug report.
//!
//! `VAHT_COMMIT` is the commit's short hash, with `-modified` when tracked
//! files had uncommitted changes, or `unknown` outside a git checkout.

use std::path::Path;
use std::process::Command;

fn main() {
    let commit = git(&["rev-parse", "--short=12", "HEAD"]);
    let is_modified = git(&["status", "--porcelain", "--untracked-files=no"])
        .is_some_and(|changes| !changes.is_empty());
    let version = match commit {
        Some(commit) if is_modified => format!("{commit}-modified"),
        Some(commit) => commit,
        None => "unknown".to_owned(),
    };
    println!("cargo:rustc-env=VAHT_COMMIT={version}");
    // Run again when a commit is made or checked out, or a file changes.
    if let Some(directory) = git(&["rev-parse", "--absolute-git-dir"]) {
        let directory = Path::new(&directory);
        println!(
            "cargo:rerun-if-changed={}",
            directory.join("HEAD").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            directory.join("index").display()
        );
        if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!(
                "cargo:rerun-if-changed={}",
                directory.join(branch).display()
            );
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for watched in ["crates", "data", "packaging", "Cargo.toml", "Cargo.lock"] {
        println!("cargo:rerun-if-changed={}", root.join(watched).display());
    }
}

/// Returns what `git` with `arguments` prints, trimmed, if it succeeds.
fn git(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
