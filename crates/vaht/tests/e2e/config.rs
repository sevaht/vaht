//! Editing the configuration while vaht runs: changes made in place apply at
//! once, stay out of the files until saved, and are refused whole if the
//! configuration would not be valid. The settings' own values are checked
//! through the workspaces' names, which `vaht msg workspaces` lists.

use std::path::Path;
use std::process::Output;

use vaht_ipc::{Client, ConfigEdit, ConfigFile, ConfigPath, EventKind, Reply, Request};
use vaht_test_support::HeadlessVaht;

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// What the settings file says at the start of most tests.
const SETTINGS: &str = "# Mine.\n[workspaces]\nnames = [\"one\", \"two\"] # how many\n";

/// Starts vaht with `settings` as its `settings.toml`.
fn start(settings: &str) -> HeadlessVaht {
    HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("settings.toml", settings)])
}

/// Runs `vaht msg <arguments>` against `vaht`.
fn message(vaht: &HeadlessVaht, arguments: &[&str]) -> Output {
    std::process::Command::new(EXECUTABLE)
        .arg("msg")
        .args(arguments)
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht msg runs")
}

/// Runs `vaht msg config <arguments>`, which must succeed.
fn config(vaht: &HeadlessVaht, arguments: &[&str]) -> String {
    let output = message(vaht, &[&["config"], arguments].concat());
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).expect("UTF-8")
}

/// Returns the names of the workspaces vaht has.
fn workspace_names(vaht: &HeadlessVaht) -> Vec<String> {
    let output = message(vaht, &["--json", "workspaces"]);
    let reply: Reply = serde_json::from_slice(&output.stdout).expect("one JSON reply");
    let Reply::Workspaces { workspaces } = reply else {
        panic!("not a workspace list: {reply:?}");
    };
    workspaces
        .into_iter()
        .map(|workspace| workspace.name)
        .collect()
}

/// Returns what `settings.toml` says now.
fn settings_file(vaht: &HeadlessVaht) -> String {
    std::fs::read_to_string(vaht.config_directory().join("settings.toml")).expect("read")
}

#[test]
fn an_edit_applies_at_once_and_stays_out_of_the_file_until_saved() {
    let vaht = start(SETTINGS);
    config(&vaht, &["set", "workspaces.names", r#"["a", "b", "c"]"#]);
    assert_eq!(workspace_names(&vaht), ["a", "b", "c"], "it runs now");
    assert_eq!(settings_file(&vaht), SETTINGS, "the file is as it was");
    assert!(
        config(&vaht, &["get"]).contains("* workspaces.names = [\"a\",\"b\",\"c\"]\n"),
        "the key is marked changed"
    );
    assert!(config(&vaht, &["files"]).contains("settings.toml\tchanged"));
    config(&vaht, &["save"]);
    assert_eq!(
        settings_file(&vaht),
        "# Mine.\n[workspaces]\nnames = [\"a\", \"b\", \"c\"] # how many\n",
        "the file says it, its comments kept"
    );
    assert!(config(&vaht, &["files"]).contains("settings.toml\tclean"));
}

#[test]
fn an_invalid_edit_is_refused_and_changes_nothing() {
    let vaht = start("");
    let output = message(&vaht, &["config", "set", "blur.passes", "99"]);
    assert!(!output.status.success(), "{output:?}");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("passes"), "it says which setting: {error}");
    assert!(
        config(&vaht, &["get"]).contains("  blur.passes = 3\n"),
        "nothing changed"
    );
    let output = message(&vaht, &["config", "set", "blurr.passes", "3"]);
    assert!(!output.status.success(), "an unknown key is refused too");
}

#[test]
fn a_batch_of_edits_is_made_whole_or_not_at_all() {
    let vaht = start("");
    let mut client = Client::connect(&vaht.ipc_socket()).expect("connects");
    let edit = |key: &str, value: serde_json::Value| ConfigEdit::Set {
        path: ConfigPath::parse(key).expect("a key"),
        value,
    };
    let reply = client
        .request(&Request::ConfigEdit {
            file: ConfigFile::Settings,
            edits: vec![
                edit("blur.spread", 7.into()),
                edit("blur.passes", 99.into()),
            ],
        })
        .expect("answered");
    assert!(matches!(reply, Reply::Error { .. }), "{reply:?}");
    assert!(
        config(&vaht, &["get"]).contains("  blur.spread = 3\n"),
        "the valid edit was not made without the other"
    );
    let reply = client
        .request(&Request::ConfigEdit {
            file: ConfigFile::Settings,
            edits: vec![edit("blur.spread", 7.into()), edit("blur.passes", 4.into())],
        })
        .expect("answered");
    assert_eq!(reply, Reply::Done {});
    let text = config(&vaht, &["get"]);
    assert!(
        text.contains("* blur.spread = 7\n") && text.contains("* blur.passes = 4\n"),
        "{text}"
    );
}

#[test]
fn unsetting_puts_a_setting_back_to_its_default_and_saving_leaves_it_out() {
    let vaht = start("[blur]\npasses = 5\n");
    assert!(config(&vaht, &["get"]).contains("  blur.passes = 5\n"));
    config(&vaht, &["unset", "blur.passes"]);
    assert!(
        config(&vaht, &["get"]).contains("* blur.passes = 3\n"),
        "the default shows"
    );
    config(&vaht, &["save"]);
    assert!(
        !settings_file(&vaht).contains("passes"),
        "{}",
        settings_file(&vaht)
    );
}

#[test]
fn reverting_drops_the_changes_made_in_place() {
    let vaht = start(SETTINGS);
    config(&vaht, &["set", "workspaces.names", r#"["x"]"#]);
    assert_eq!(workspace_names(&vaht), ["x"]);
    config(&vaht, &["revert"]);
    assert_eq!(
        workspace_names(&vaht),
        ["one", "two"],
        "the file runs again"
    );
    assert!(config(&vaht, &["files"]).contains("settings.toml\tclean"));
}

#[test]
fn a_program_following_the_configuration_hears_of_each_new_one() {
    let vaht = start("");
    let mut client = Client::connect(&vaht.ipc_socket()).expect("connects");
    client
        .subscribe(vec![EventKind::Config])
        .expect("subscribed");
    let generation = |client: &mut Client| {
        let event = client.next_event().expect("an event");
        let vaht_ipc::Event::Config { generation, .. } = event else {
            panic!("not a configuration event: {event:?}");
        };
        generation
    };
    let first = generation(&mut client);
    config(&vaht, &["set", "blur.spread", "5"]);
    let second = generation(&mut client);
    assert!(second > first, "{first} then {second}");
    std::fs::write(
        vaht.config_directory().join("settings.toml"),
        "[blur]\npasses = 2\n",
    )
    .expect("written");
    let third = generation(&mut client);
    assert!(third > second, "a file changed on disk is news too");
}

#[test]
fn other_programs_may_read_but_not_edit_when_commands_are_off() {
    let vaht = start("[ipc]\nallow_commands = false\n");
    assert!(config(&vaht, &["get"]).contains("  ipc.allow_commands = false\n"));
    let output = message(&vaht, &["config", "set", "blur.spread", "5"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("allow_commands"));
}

#[test]
fn a_change_in_place_is_made_with_the_set_setting_action_too() {
    let vaht = start("");
    let output = message(&vaht, &["run", "unset-setting", "blur.spread"]);
    assert!(output.status.success(), "{output:?}");
    config(&vaht, &["set", "blur.spread", "9"]);
    let output = message(&vaht, &["run", "unset-setting", "blur.spread"]);
    assert!(output.status.success(), "{output:?}");
    assert!(config(&vaht, &["get"]).contains("* blur.spread = 3\n"));
}

/// Collects the dotted path of every setting `value` holds: the leaves, and
/// a table whose keys are free (an empty one has none to give).
fn leaf_paths(value: &serde_json::Value, prefix: &[String], paths: &mut Vec<Vec<String>>) {
    match value {
        serde_json::Value::Object(table) if !table.is_empty() => {
            for (key, inner) in table {
                let mut path = prefix.to_vec();
                path.push(key.clone());
                leaf_paths(inner, &path, paths);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_)
        | serde_json::Value::Array(_)
        | serde_json::Value::Object(_) => paths.push(prefix.to_vec()),
    }
}

#[test]
fn the_form_built_from_vahts_schema_has_every_setting_vaht_has() {
    use vaht_settings_model::{Form, SettingKind};
    let vaht = start("");
    let json = |arguments: &[&str]| -> serde_json::Value {
        let output = message(&vaht, &[&["--json", "config"], arguments].concat());
        serde_json::from_slice(&output.stdout).expect("one JSON reply")
    };
    let schema = json(&["schema"])["schema"].clone();
    let form = Form::from_schema(&schema).expect("the schema is a form");
    let reading = json(&["get"]);
    let mut paths = Vec::new();
    leaf_paths(&reading["value"], &[], &mut paths);
    assert!(paths.len() > 60, "{} settings", paths.len());
    for path in paths {
        let parts = ConfigPath::from_parts(path.clone()).expect("a key");
        let own = form.setting(&parts).is_some();
        // Or inside a setting that is a whole table or list.
        let inside = (1..path.len()).any(|length| {
            let outer = ConfigPath::from_parts(path[..length].to_vec()).expect("a key");
            form.setting(&outer).is_some_and(|setting| {
                matches!(setting.kind, SettingKind::Raw | SettingKind::Mapping(_))
            })
        });
        assert!(own || inside, "{parts} is in no form setting");
    }
}
