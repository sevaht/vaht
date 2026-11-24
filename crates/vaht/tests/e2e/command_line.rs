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

/// Returns `path` as an argument.
fn path(path: &Path) -> String {
    path.display().to_string()
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
         session.screen0.toolbar.alpha: 200\nsession.screen0.toolbar.autoHide: true\n",
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
    assert!(settings.contains("layout = \"Night-Sky\""), "{settings}");
    assert!(
        !settings.contains("auto_hide"),
        "what is shown is the layout's, not settings.toml's: {settings}"
    );
    let layout =
        std::fs::read_to_string(target.path().join("layouts/Night-Sky.toml")).expect("written");
    assert!(layout.contains("auto_hide = true"), "{layout}");
    assert!(
        !target.path().join("themes").exists(),
        "there are no themes"
    );
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

/// A small Fluxbox style.
const FLUXBOX_STYLE: &str = "toolbar: Raised Gradient Vertical\n\
    toolbar.color: grey30\ntoolbar.colorTo: grey5\n\
    window.title.focus: Raised Gradient Vertical\n\
    window.title.focus.color: grey55\nwindow.title.focus.colorTo: grey5\n\
    window.label.focus.textColor: white\n\
    menu.frame: Flat Solid\nmenu.frame.color: black\nmenu.frame.textColor: white\n";

#[test]
fn a_fluxbox_style_of_images_is_converted_with_its_images() {
    let logs = log_directory();
    let work = tempfile::tempdir().expect("a temporary directory");
    let source = work.path().join("Pictured");
    std::fs::create_dir_all(source.join("pixmaps")).expect("made");
    std::fs::write(
        source.join("theme.cfg"),
        "toolbar:\ntoolbar.pixmap: bar.xpm\ntoolbar.clock.textColor: white\n",
    )
    .expect("written");
    std::fs::write(
        source.join("pixmaps/bar.xpm"),
        "/* XPM */\nstatic char * bar[] = {\n\"1 2 2 1\",\n\".\tc #202020\",\n\"+\tc #101010\",\n\".\",\n\"+\"};\n",
    )
    .expect("written");
    let configuration = work.path().join("config");
    let output = run_executable(
        &[
            "import",
            "fluxbox-style",
            source.to_str().expect("UTF-8"),
            "--to",
            configuration.join("styles").to_str().expect("UTF-8"),
        ],
        logs.path(),
    );
    assert!(output.status.success(), "{output:?}");
    let style =
        std::fs::read_to_string(configuration.join("styles/Pictured.toml")).expect("written");
    assert!(style.contains("image = \"Pictured/bar.xpm\""), "{style}");
    assert!(
        configuration.join("styles/Pictured/bar.xpm").is_file(),
        "the image is written beside the style"
    );
    std::fs::write(
        configuration.join("settings.toml"),
        "style = \"Pictured\"\n",
    )
    .expect("written");
    let checked = run_executable(
        &[
            "config",
            "check",
            "--config-directory",
            configuration.to_str().expect("UTF-8"),
        ],
        logs.path(),
    );
    assert!(
        checked.status.success(),
        "vaht reads it with its image: {checked:?}"
    );
}

/// A style whose titlebar is `base` and whose label is `accent`.
fn two_colored_style(base: &str, accent: &str) -> String {
    format!(
        "window.title.focus: Flat Solid\nwindow.title.focus.color: {base}\n\
         window.label.focus: Flat Solid\nwindow.label.focus.color: {accent}\n\
         menu.frame: Flat Solid\nmenu.frame.color: black\nmenu.frame.textColor: white\n"
    )
}

#[test]
fn variants_of_a_fluxbox_style_fold_into_one_style_with_options() {
    let logs = log_directory();
    let work = tempfile::tempdir().expect("a temporary directory");
    let source = work.path().join("Pack");
    std::fs::create_dir_all(&source).expect("made");
    for (name, base, accent) in [
        ("Pack-red_green", "#800000", "#008000"),
        ("Pack-red_blue", "#800000", "#000080"),
        ("Pack-dark-blue_green", "#000040", "#008000"),
        ("Pack-dark-blue_red", "#000040", "#ff0000"),
    ] {
        std::fs::write(source.join(name), two_colored_style(base, accent)).expect("written");
    }
    let configuration = work.path().join("config");
    let styles = configuration.join("styles");
    let output = run_executable(
        &[
            "import",
            "fluxbox-style",
            source.to_str().expect("UTF-8"),
            "--to",
            styles.to_str().expect("UTF-8"),
            "--options",
            "base,accent",
        ],
        logs.path(),
    );
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("4 variants folded into options"),
        "{output:?}"
    );
    let written: Vec<String> = std::fs::read_dir(&styles)
        .expect("read")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(written, ["Pack.toml"], "one style, not four");
    let style = std::fs::read_to_string(styles.join("Pack.toml")).expect("written");
    assert!(style.contains("[options.base.dark_blue]"), "{style}");
    assert!(style.contains("[options.accent.green]"), "{style}");
    // A pair that never was a file is a choice of two options.
    std::fs::write(
        configuration.join("settings.toml"),
        "style = \"Pack\"\nstyle_options = { base = \"red\", accent = \"red\" }\n",
    )
    .expect("written");
    let checked = run_executable(
        &[
            "config",
            "check",
            "--config-directory",
            configuration.to_str().expect("UTF-8"),
        ],
        logs.path(),
    );
    assert!(checked.status.success(), "{checked:?}");
}

#[test]
fn variants_stay_separate_styles_when_asked() {
    let logs = log_directory();
    let work = tempfile::tempdir().expect("a temporary directory");
    let source = work.path().join("Pack");
    std::fs::create_dir_all(&source).expect("made");
    for (name, base, accent) in [
        ("Pack-red_green", "#800000", "#008000"),
        ("Pack-red_blue", "#800000", "#000080"),
    ] {
        std::fs::write(source.join(name), two_colored_style(base, accent)).expect("written");
    }
    let styles = work.path().join("styles");
    let output = run_executable(
        &[
            "import",
            "fluxbox-style",
            source.to_str().expect("UTF-8"),
            "--to",
            styles.to_str().expect("UTF-8"),
            "--separate",
        ],
        logs.path(),
    );
    assert!(output.status.success(), "{output:?}");
    let mut written: Vec<String> = std::fs::read_dir(&styles)
        .expect("read")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(written, ["Pack-red_blue.toml", "Pack-red_green.toml"]);
}

#[test]
fn a_fluxbox_style_converts_on_its_own_and_vaht_accepts_it() {
    let logs = log_directory();
    let work = tempfile::tempdir().expect("a temporary directory");
    let source = work.path().join("Mine");
    std::fs::write(&source, FLUXBOX_STYLE).expect("written");
    let configuration = work.path().join("config");
    let styles = configuration.join("styles");
    let output = run_executable(
        &[
            "import",
            "fluxbox-style",
            &path(&source),
            "--to",
            &path(&styles),
        ],
        logs.path(),
    );
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Mine.toml"),
        "{output:?}"
    );
    let written: Vec<_> = std::fs::read_dir(&configuration)
        .expect("the configuration directory")
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(
        written,
        ["styles"],
        "only a style is written, no settings.toml or the like"
    );
    // vaht takes it as the style a configuration chooses.
    std::fs::write(configuration.join("settings.toml"), "style = \"Mine\"\n").expect("written");
    let checked = run_executable(
        &[
            "config",
            "check",
            "--config-directory",
            &path(&configuration),
        ],
        logs.path(),
    );
    assert!(checked.status.success(), "{checked:?}");
}

#[test]
fn a_style_is_written_to_the_configuration_directorys_styles_unless_told_where() {
    let logs = log_directory();
    let work = tempfile::tempdir().expect("a temporary directory");
    let source = work.path().join("Mine");
    std::fs::write(&source, FLUXBOX_STYLE).expect("written");
    let configuration = work.path().join("config");
    let output = run_executable(
        &[
            "import",
            "fluxbox-style",
            &path(&source),
            "--config-directory",
            &path(&configuration),
        ],
        logs.path(),
    );
    assert!(output.status.success(), "{output:?}");
    assert!(configuration.join("styles").join("Mine.toml").is_file());
}

#[test]
fn a_style_is_not_replaced_without_force() {
    let logs = log_directory();
    let work = tempfile::tempdir().expect("a temporary directory");
    let source = work.path().join("Mine");
    std::fs::write(&source, FLUXBOX_STYLE).expect("written");
    let styles = work.path().join("styles");
    let (source_argument, styles_argument) = (path(&source), path(&styles));
    let convert = |extra: &[&str]| {
        let mut arguments = vec![
            "import",
            "fluxbox-style",
            source_argument.as_str(),
            "--to",
            styles_argument.as_str(),
        ];
        arguments.extend_from_slice(extra);
        run_executable(&arguments, logs.path())
    };
    assert!(convert(&[]).status.success());
    std::fs::write(styles.join("Mine.toml"), "# mine, edited\n").expect("edited");
    let refused = convert(&[]);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("--force"),
        "{refused:?}"
    );
    assert_eq!(
        std::fs::read_to_string(styles.join("Mine.toml")).expect("read"),
        "# mine, edited\n",
        "the edited style is untouched"
    );
    assert!(convert(&["--force"]).status.success());
    assert!(
        std::fs::read_to_string(styles.join("Mine.toml"))
            .expect("read")
            .starts_with("# Converted from"),
        "replaced on request"
    );
}

#[test]
fn a_directory_of_styles_converts_each_style_and_leaves_the_rest() {
    let logs = log_directory();
    let work = tempfile::tempdir().expect("a temporary directory");
    let collection = work.path().join("styles");
    std::fs::create_dir_all(collection.join("more")).expect("directories");
    std::fs::write(collection.join("One"), FLUXBOX_STYLE).expect("written");
    std::fs::write(collection.join("more").join("Two.style"), FLUXBOX_STYLE).expect("written");
    std::fs::write(collection.join("README"), "About these styles.\n").expect("written");
    let out = work.path().join("out");
    let output = run_executable(
        &[
            "import",
            "fluxbox-style",
            &path(&collection),
            "--to",
            &path(&out),
        ],
        logs.path(),
    );
    assert!(output.status.success(), "{output:?}");
    let mut names: Vec<_> = std::fs::read_dir(&out)
        .expect("the output directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["One.toml", "Two.toml"]);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Converted 2 of 2 styles"),
        "{output:?}"
    );
}

#[test]
fn a_directory_with_no_styles_in_it_is_an_error() {
    let logs = log_directory();
    let work = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(work.path().join("README"), "Nothing to convert.\n").expect("written");
    let output = run_executable(
        &[
            "import",
            "fluxbox-style",
            &path(work.path()),
            "--to",
            &path(&work.path().join("out")),
        ],
        logs.path(),
    );
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no Fluxbox styles"),
        "{output:?}"
    );
}
