//! IPC tests: the commands that ask a running vaht, and programs vaht starts
//! finding its socket.

use std::path::Path;
use std::process::{Command, Output};

use vaht_ipc::{Client, ConfigFile, Reply, Request, WindowInfo};
use vaht_test_support::{HeadlessVaht, TestClient, wait_for_file};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Runs `vaht` with `arguments` against the running vaht.
fn message(vaht: &HeadlessVaht, arguments: &[&str]) -> Output {
    Command::new(EXECUTABLE)
        .args(arguments)
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht runs")
}

/// Runs `vaht --json` with `arguments` and returns the reply.
fn ask(vaht: &HeadlessVaht, arguments: &[&str]) -> Reply {
    let output = message(vaht, &[&["--json"], arguments].concat());
    serde_json::from_slice(&output.stdout).expect("one JSON reply")
}

/// Returns the windows vaht lists.
fn windows(vaht: &HeadlessVaht) -> Vec<WindowInfo> {
    let reply = ask(vaht, &["windows"]);
    let Reply::Windows { windows } = reply else {
        panic!("not a window list: {reply:?}");
    };
    windows
}

#[test]
fn windows_are_listed_topmost_first_with_the_focused_one_marked() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(2);
    let listed = windows(&vaht);
    assert_eq!(listed.len(), 2, "{listed:?}");
    assert!(listed[0].is_focused, "the newest window is on top, focused");
    assert!(!listed[1].is_focused);
    assert_eq!(listed[0].workspace, Some(1));
    let readable = message(&vaht, &["windows"]);
    assert!(readable.status.success());
    assert_eq!(
        String::from_utf8_lossy(&readable.stdout).lines().count(),
        2,
        "a line per window"
    );
}

#[test]
fn a_command_runs_on_the_window_named() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(2);
    let below = windows(&vaht)[1].id.to_string();
    let output = message(&vaht, &["run", "--window", &below, "shade"]);
    assert!(output.status.success(), "{output:?}");
    let shaded: Vec<bool> = windows(&vaht)
        .iter()
        .map(|window| window.is_shaded)
        .collect();
    assert_eq!(shaded, [false, true], "only the window named");
}

#[test]
fn a_command_changes_the_workspace_shown() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let output = message(&vaht, &["run", "workspace", "2"]);
    assert!(output.status.success(), "{output:?}");
    let listed = String::from_utf8(message(&vaht, &["workspaces"]).stdout).expect("UTF-8");
    assert!(listed.contains("* 2\tWorkspace 2"), "{listed}");
}

#[test]
fn a_command_vaht_does_not_know_fails_with_its_reason() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let output = message(&vaht, &["run", "dance"]);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("dance"), "{error}");
    let output = message(&vaht, &["run", "--window", "999", "close"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no window 999"));
}

#[test]
fn the_log_filter_is_told_changed_and_reset_while_vaht_runs() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let filter = |arguments: &[&str]| {
        let reply = ask(&vaht, &[&["log-filter"], arguments].concat());
        let Reply::LogFilter { directives } = reply else {
            panic!("not a filter: {reply:?}");
        };
        directives
    };
    let started = filter(&[]);
    // A single directive: tracing prints several in an order of its own.
    assert_eq!(filter(&["vaht_compositor=trace"]), "vaht_compositor=trace");
    assert_eq!(filter(&[]), "vaht_compositor=trace", "kept");
    assert_eq!(
        filter(&["reset"]),
        started,
        "back to the one it started with"
    );
}

#[test]
fn an_invalid_log_filter_is_refused_and_the_filter_kept() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let before = message(&vaht, &["log-filter"]);
    let output = message(&vaht, &["log-filter", "vaht=nonsense"]);
    assert!(!output.status.success(), "refused");
    assert_eq!(
        message(&vaht, &["log-filter"]).stdout,
        before.stdout,
        "unchanged"
    );
}

#[test]
fn programs_vaht_starts_are_told_where_its_socket_is() {
    let files = tempfile::tempdir().expect("a temporary directory");
    let marker = files.path().join("socket");
    let settings = format!(
        "[session]\nautostart = [\"printf %s \\\"$VAHT_SOCKET\\\" > '{}'\"]\n",
        marker.display()
    );
    let vaht =
        HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("settings.toml", &settings)]);
    let told = wait_for_file(&marker, |contents| !contents.is_empty());
    assert_eq!(Path::new(&told), vaht.ipc_socket());
}

#[test]
fn connections_leave_nothing_behind() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    for _ in 0..3 {
        assert!(matches!(ask(&vaht, &["version"]), Reply::Version { .. }));
    }
    let socket = vaht.ipc_socket();
    let report = vaht.stop();
    report.assert_nothing_left_behind();
    assert!(!socket.exists(), "the socket is removed as vaht stops");
}

#[test]
fn a_window_in_a_lower_layer_stays_under_when_raised() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(2);
    let top = windows(&vaht)[0].id;
    let id = top.to_string();
    for command in [["set-layer", "bottom"], ["raise", ""]] {
        let mut arguments = vec!["run", "--window", &id, command[0]];
        arguments.extend(command[1..].iter().filter(|word| !word.is_empty()));
        let output = message(&vaht, &arguments);
        assert!(output.status.success(), "{output:?}");
    }
    let order: Vec<u64> = windows(&vaht).iter().map(|window| window.id).collect();
    assert_eq!(
        order.last(),
        Some(&top),
        "under the normal window: {order:?}"
    );
}

#[test]
fn cycling_in_opening_order_goes_through_windows_as_they_opened() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[(
            "bindings.toml",
            "[keys]\nF1 = { action = \"next-window\", order = \"opening\" }\n",
        )],
    );
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(3);
    let focused = |vaht: &HeadlessVaht| {
        windows(vaht)
            .into_iter()
            .find(|window| window.is_focused)
            .map(|window| window.id)
    };
    let mut opened: Vec<u64> = windows(&vaht).iter().map(|window| window.id).collect();
    opened.sort_unstable();
    assert_eq!(focused(&vaht), Some(opened[2]), "the newest");
    client.type_chord(&[vaht_test_support::keys::F1]);
    assert_eq!(focused(&vaht), Some(opened[0]), "around to the first");
    client.type_chord(&[vaht_test_support::keys::F1]);
    assert_eq!(focused(&vaht), Some(opened[1]));
}

#[test]
fn commands_can_be_turned_off_for_other_programs() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", "[ipc]\nallow_commands = false\n")],
    );
    let output = message(&vaht, &["run", "workspace", "2"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("allow_commands"),
        "{output:?}"
    );
    assert!(
        message(&vaht, &["workspaces"]).status.success(),
        "asking is still allowed"
    );
}

#[test]
fn programs_vaht_starts_learn_the_desktop_and_session() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let directory = tempfile::tempdir().expect("a directory for the result");
    let result = directory.path().join("environment");
    let command = format!(
        "exec printf '%s %s %s' \"$XDG_CURRENT_DESKTOP\" \"$XDG_SESSION_DESKTOP\" \
         \"$XDG_SESSION_TYPE\" > {}",
        result.display()
    );
    let output = message(&vaht, &["run", &command]);
    assert!(output.status.success(), "{output:?}");
    wait_for_file(&result, |text| text == "vaht vaht wayland");
}

#[test]
fn a_subscriber_hears_each_list_at_once_and_whenever_it_changes() {
    use std::io::BufRead as _;

    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut follower = Command::new(EXECUTABLE)
        .args(["--json", "subscribe", "workspaces", "windows"])
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("vaht runs");
    let (lines, received) = std::sync::mpsc::channel();
    let stdout = follower.stdout.take().expect("piped");
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if lines.send(line).is_err() {
                return;
            }
        }
    });
    let next = || -> vaht_ipc::Event {
        let line = received
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("an event in time");
        serde_json::from_str(&line).expect("an event")
    };
    let current = |event: &vaht_ipc::Event| match event {
        vaht_ipc::Event::Workspaces { workspaces } => workspaces
            .iter()
            .find(|workspace| workspace.is_current)
            .map(|workspace| workspace.number),
        vaht_ipc::Event::Windows { .. }
        | vaht_ipc::Event::Outputs { .. }
        | vaht_ipc::Event::Config { .. } => None,
    };
    let first = [next(), next()];
    assert!(
        first.iter().any(|event| current(event) == Some(1)),
        "{first:?}"
    );
    assert!(
        first.iter().any(
            |event| matches!(event, vaht_ipc::Event::Windows { windows } if windows.is_empty())
        ),
        "{first:?}"
    );

    assert!(message(&vaht, &["run", "workspace", "2"]).status.success());
    assert_eq!(current(&next()), Some(2), "only the list that changed");

    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let opened = loop {
        if let vaht_ipc::Event::Windows { windows } = next()
            && !windows.is_empty()
        {
            break windows;
        }
    };
    assert_eq!(opened[0].app_id, "vaht.test");
    follower.kill().expect("stopped");
    let _ = follower.wait();
}

#[test]
fn a_modal_dialog_is_listed_with_its_parent_and_keeps_the_focus() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(2);
    client.open_dialog(0, true);
    let id_of = |title: &str| {
        windows(&vaht)
            .into_iter()
            .find(|window| window.title == title)
            .unwrap_or_else(|| panic!("{title} is listed"))
            .id
    };
    let (parent, other, dialog) = (
        id_of("test window 1"),
        id_of("test window 2"),
        id_of("test dialog"),
    );
    let listed = windows(&vaht);
    assert_eq!(listed[0].id, dialog, "the new dialog is on top: {listed:?}");
    assert_eq!(listed[0].parent, Some(parent));
    let focus = |window: u64| {
        let output = message(&vaht, &["run", "--window", &window.to_string(), "focus"]);
        assert!(output.status.success(), "{output:?}");
        windows(&vaht)
            .into_iter()
            .find(|window| window.is_focused)
            .map(|window| window.id)
    };
    assert_eq!(focus(other), Some(other));
    assert_eq!(focus(parent), Some(dialog), "the modal dialog keeps it");
}

/// Starts `vaht pick --title Share` with `items` on its standard input,
/// once vaht shows them, and returns it running.
fn start_pick(vaht: &HeadlessVaht, items: &str) -> std::process::Child {
    use std::io::Write as _;
    let mut pick = Command::new(EXECUTABLE)
        .args(["pick", "--title", "Share"])
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("vaht pick runs");
    pick.stdin
        .take()
        .expect("its input")
        .write_all(items.as_bytes())
        .expect("the items are written");
    vaht.wait_for_logged("asking the user to pick", 1);
    pick
}

#[test]
fn a_pick_prints_the_item_chosen_from_vahts_menu() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    let items = ["Monitor: HEADLESS-1", "Window: notes"];
    let pick = start_pick(&vaht, &format!("{}\n\n{}\n", items[0], items[1]));
    client.press_key(vaht_test_support::keys::DOWN);
    client.release_key(vaht_test_support::keys::DOWN);
    client.press_key(vaht_test_support::keys::ENTER);
    client.release_key(vaht_test_support::keys::ENTER);
    let output = pick.wait_with_output().expect("vaht pick ends");
    assert!(output.status.success(), "{output:?}");
    let chosen = String::from_utf8(output.stdout).expect("UTF-8");
    assert!(
        items.contains(&chosen.trim_end()),
        "one of the items: {chosen:?}"
    );
    assert!(
        chosen.ends_with('\n') && chosen.lines().count() == 1,
        "{chosen:?}"
    );
}

#[test]
fn a_pick_closed_without_a_choice_fails_and_prints_nothing() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    let pick = start_pick(&vaht, "Monitor: HEADLESS-1\n");
    client.press_key(vaht_test_support::keys::ESCAPE);
    client.release_key(vaht_test_support::keys::ESCAPE);
    let output = pick.wait_with_output().expect("vaht pick ends");
    assert!(!output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
}

#[test]
fn nothing_to_pick_fails_at_once() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let output = Command::new(EXECUTABLE)
        .args(["pick"])
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .stdin(std::process::Stdio::null())
        .output()
        .expect("vaht pick runs");
    assert!(
        !output.status.success() && output.stdout.is_empty(),
        "{output:?}"
    );
}

/// Runs `vaht --json config <arguments>` and returns the reply.
fn config(vaht: &HeadlessVaht, arguments: &[&str]) -> Reply {
    ask(vaht, &[&["config"], arguments].concat())
}

/// Sends `request` over the socket, as the settings window does, and returns
/// the reply.
fn request(vaht: &HeadlessVaht, request: &Request) -> Reply {
    Client::connect(&vaht.ipc_socket())
        .expect("connects")
        .request(request)
        .expect("a reply")
}

#[test]
fn the_configuration_files_are_listed_with_their_state_and_the_styles() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("styles/mine.toml", "[default]\ntext_color = \"white\"\n")],
    );
    let Reply::ConfigFiles {
        files,
        styles,
        layouts,
        style_options,
        problems,
    } = config(&vaht, &["files"])
    else {
        panic!("not a file list");
    };
    assert_eq!(
        files,
        [vaht_ipc::ConfigFileInfo {
            file: ConfigFile::Settings,
            state: vaht_ipc::ConfigState::Clean,
        }]
    );
    assert!(styles.contains(&"mine".to_owned()), "{styles:?}");
    // Whatever is installed on this machine follows it.
    assert_eq!(
        layouts.first().map(String::as_str),
        Some("default"),
        "the default layout, which needs no file, first"
    );
    let declared: Vec<&str> = style_options
        .iter()
        .map(|option| option.name.as_str())
        .collect();
    assert_eq!(
        declared,
        [
            "bar_outline",
            "menu_body",
            "menu_outline",
            "menu_size",
            "palette",
            "shading",
            "taskbar_size",
            "titlebar_size",
            "unfocused"
        ],
        "the built-in style's options"
    );
    assert_eq!(style_options[4].chosen, "graphite");
    assert_eq!(
        problems,
        Vec::<String>::new(),
        "a valid configuration has none"
    );
}

#[test]
fn the_schema_describes_each_setting_with_its_range_and_documentation() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let Reply::ConfigSchema { schema } = request(
        &vaht,
        &Request::ConfigSchema {
            file: ConfigFile::Settings,
        },
    ) else {
        panic!("not a schema");
    };
    assert_eq!(
        schema["$schema"], "https://json-schema.org/draft/2020-12/schema",
        "{schema}"
    );
    let passes = &schema["$defs"]["BlurPasses"];
    assert_eq!(
        (&passes["minimum"], &passes["maximum"]),
        (&1.into(), &6.into()),
        "{passes}"
    );
    assert!(
        schema["$defs"]["BlurSettings"]["properties"]["enabled"]["description"]
            .as_str()
            .is_some_and(|text| text.contains("blur")),
        "the documentation is the doc comment's"
    );
}

#[test]
fn what_the_settings_say_now_is_read_with_what_is_changed_in_place() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", "[blur]\npasses = 5\n")],
    );
    let Reply::ConfigRead {
        value,
        defaults,
        changed,
        ..
    } = config(&vaht, &["get", "settings"])
    else {
        panic!("not a reading");
    };
    assert_eq!(value["blur"]["passes"], 5, "the file's value");
    assert_eq!(defaults["blur"]["passes"], 3, "vaht's default");
    assert_eq!(
        value["blur"]["spread"], 3,
        "what the file leaves out, as defaulted"
    );
    assert!(changed.is_empty(), "{changed:?}");
    let run = |command: &str| {
        let output = message(&vaht, &["run", "set-setting", command]);
        assert!(output.status.success(), "{output:?}");
    };
    run("blur.spread 7");
    let Reply::ConfigRead { value, changed, .. } = config(&vaht, &["get", "settings"]) else {
        panic!("not a reading");
    };
    assert_eq!(value["blur"]["spread"], 7, "a change made in place shows");
    assert_eq!(changed, [["blur".to_owned(), "spread".to_owned()]]);
}

#[test]
fn a_file_not_described_yet_is_refused_plainly() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let Reply::Error { message } = request(
        &vaht,
        &Request::ConfigSchema {
            file: ConfigFile::Bindings,
        },
    ) else {
        panic!("not an error");
    };
    assert!(message.contains("bindings.toml"), "{message}");
}

#[test]
fn what_msg_config_get_prints_is_a_line_per_setting() {
    let vaht = HeadlessVaht::start_with_config(
        Path::new(EXECUTABLE),
        &[("settings.toml", "[blur]\npasses = 5\n")],
    );
    let output = message(&vaht, &["config", "get"]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).expect("UTF-8");
    assert!(text.contains("  blur.passes = 5\n"), "{text}");
    assert!(
        text.contains("  logging.delete_after_days = unset\n"),
        "{text}"
    );
}
