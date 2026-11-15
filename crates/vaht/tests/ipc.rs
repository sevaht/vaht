//! IPC tests: `vaht msg` asking a running vaht, and programs vaht starts
//! finding its socket.

use std::path::Path;
use std::process::{Command, Output};

use vaht_ipc::{Reply, WindowInfo};
use vaht_test_support::{HeadlessVaht, TestClient, wait_for_file};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Runs `vaht msg` with `arguments` against `vaht`.
fn message(vaht: &HeadlessVaht, arguments: &[&str]) -> Output {
    Command::new(EXECUTABLE)
        .arg("msg")
        .args(arguments)
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht msg runs")
}

/// Runs `vaht msg --json` with `arguments` and returns the reply.
fn ask(vaht: &HeadlessVaht, arguments: &[&str]) -> Reply {
    let output = message(vaht, &[&["--json"], arguments].concat());
    serde_json::from_slice(&output.stdout).expect("one JSON reply")
}

/// Returns the windows vaht lists.
fn windows(vaht: &HeadlessVaht) -> Vec<WindowInfo> {
    match ask(vaht, &["windows"]) {
        Reply::Windows { windows } => windows,
        other @ (Reply::Version { .. }
        | Reply::Done {}
        | Reply::Workspaces { .. }
        | Reply::Outputs { .. }
        | Reply::Error { .. }) => panic!("not a window list: {other:?}"),
    }
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
        .args(["msg", "--json", "subscribe", "workspaces", "windows"])
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("vaht msg runs");
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
        vaht_ipc::Event::Windows { .. } | vaht_ipc::Event::Outputs { .. } => None,
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
