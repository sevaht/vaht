//! Several outputs: each shows its own part of the layout, which runs
//! from the left in the order the backend finds them, and windows move
//! from one to the next.

use std::path::Path;
use std::process::Command;

use vaht_ipc::{OutputInfo, Reply};
use vaht_test_support::{
    BUILT_IN_METRICS, BUTTON_LEFT, Bounds, HeadlessVaht, TestClient, TestLayer, WINDOW_COLOR, keys,
    solid_png, wait_for_file,
};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Two outputs of different sizes, side by side.
const OUTPUTS: &str = "800x600,640x480";

/// The first output's width: where the second begins.
const FIRST_WIDTH: u32 = 800;

/// The wallpaper's color.
const SKY: [u8; 3] = [0x30, 0xa0, 0x50];

/// A layer surface's color.
const BACKDROP: [u8; 3] = [0xc0, 0x40, 0x90];

/// Starts vaht on [`OUTPUTS`], with a client.
fn two_outputs() -> (HeadlessVaht, TestClient) {
    with_settings("")
}

/// Starts vaht on [`OUTPUTS`] with a solid wallpaper and `settings`, with
/// a client.
fn with_settings(settings: &str) -> (HeadlessVaht, TestClient) {
    with_files(settings, "")
}

/// Starts vaht on [`OUTPUTS`] with a solid wallpaper, `settings` and
/// `outputs` (`outputs.toml`), with a client.
fn with_files(settings: &str, outputs: &str) -> (HeadlessVaht, TestClient) {
    let picture = solid_png(16, SKY);
    let settings = format!("[wallpaper]\nimage = \"sky.png\"\nfit = \"tile\"\n{settings}");
    let vaht = HeadlessVaht::start_with_outputs(
        Path::new(EXECUTABLE),
        OUTPUTS,
        &[
            ("settings.toml", settings.as_bytes()),
            ("outputs.toml", outputs.as_bytes()),
            ("sky.png", &picture),
        ],
    );
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

/// Returns the outputs `vaht msg outputs` lists.
fn listed_outputs(vaht: &HeadlessVaht) -> Vec<OutputInfo> {
    let output = Command::new(EXECUTABLE)
        .args(["msg", "--json", "outputs"])
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht msg runs");
    match serde_json::from_slice(&output.stdout).expect("one JSON reply") {
        Reply::Outputs { outputs, .. } => outputs,
        other @ (Reply::Version { .. }
        | Reply::Done {}
        | Reply::Windows { .. }
        | Reply::Workspaces { .. }
        | Reply::Error { .. }) => panic!("not an output list: {other:?}"),
    }
}

/// Runs the command `command` in `vaht`, as `vaht msg run` does, and
/// waits until it is carried out.
fn run(vaht: &HeadlessVaht, command: &[&str]) {
    let output = Command::new(EXECUTABLE)
        .args(["msg", "run"])
        .args(command)
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht msg runs");
    assert!(output.status.success(), "{command:?}: {output:?}");
}

/// Returns which outputs `vaht msg outputs` lists as on, by connector,
/// the primary one marked with `*`.
fn outputs_on(vaht: &HeadlessVaht) -> Vec<String> {
    listed_outputs(vaht)
        .into_iter()
        .filter(|output| output.is_enabled)
        .map(|output| {
            let mark = if output.is_primary { "*" } else { "" };
            format!("{mark}{}", output.connector)
        })
        .collect()
}

/// Rewrites `vaht`'s `outputs.toml` with `outputs` and waits until it
/// reloads.
fn rewrite_outputs(vaht: &HeadlessVaht, outputs: &str) {
    let reloads = vaht.logged_count("configuration reloaded");
    std::fs::write(vaht.config_directory().join("outputs.toml"), outputs)
        .expect("outputs.toml is rewritten");
    vaht.wait_for_logged("configuration reloaded", reloads + 1);
}

#[test]
fn each_output_is_captured_at_its_own_size() {
    let (_vaht, mut client) = two_outputs();
    let first = client.screenshot_of("HEADLESS-1");
    let second = client.screenshot_of("HEADLESS-2");
    assert_eq!((first.width(), first.height()), (800, 600));
    assert_eq!((second.width(), second.height()), (640, 480));
}

#[test]
fn a_window_moved_past_the_first_output_shows_on_the_second() {
    let (_vaht, mut client) = two_outputs();
    client.open_windows(1);
    let content = client
        .screenshot_of("HEADLESS-1")
        .bounds_of(WINDOW_COLOR)
        .expect("the window opens on the first output");
    let middle = (
        u32::midpoint(content.left, content.right),
        u32::midpoint(content.top, content.bottom),
    );
    client.hold_alt();
    client.drag(middle, (middle.0 + FIRST_WIDTH, middle.1), BUTTON_LEFT);
    client.release_modifiers();
    assert_eq!(
        client.screenshot_of("HEADLESS-1").bounds_of(WINDOW_COLOR),
        None,
        "the window left the first output"
    );
    // The second output's left edge is the layout's x = 800, so the window
    // is as far into it as it was into the first.
    assert_eq!(
        client.screenshot_of("HEADLESS-2").bounds_of(WINDOW_COLOR),
        Some(content)
    );
}

#[test]
fn a_window_opens_on_the_output_the_pointer_is_on() {
    let (_vaht, mut client) = two_outputs();
    client.move_pointer_to(FIRST_WIDTH + 100, 100);
    client.open_windows(1);
    assert_eq!(
        client.screenshot_of("HEADLESS-1").bounds_of(WINDOW_COLOR),
        None
    );
    assert!(
        client
            .screenshot_of("HEADLESS-2")
            .bounds_of(WINDOW_COLOR)
            .is_some(),
        "the window opened on the second output"
    );
}

#[test]
fn a_window_maximizes_on_its_own_output() {
    let (_vaht, mut client) = two_outputs();
    client.move_pointer_to(FIRST_WIDTH + 100, 100);
    client.open_windows(1);
    client.type_chord(&[keys::LEFT_SUPER, keys::UP]);
    let second = client.screenshot_of("HEADLESS-2");
    // The taskbar is on the first output, so all of the second is usable.
    let whole = Bounds {
        left: 0,
        top: 0,
        right: second.width() - 1,
        bottom: second.height() - 1,
    };
    assert_eq!(
        second.bounds_of(WINDOW_COLOR),
        Some(BUILT_IN_METRICS.content_within(whole))
    );
    assert_eq!(
        client.screenshot_of("HEADLESS-1").bounds_of(WINDOW_COLOR),
        None
    );
}

#[test]
fn the_wallpaper_covers_every_output() {
    let (_vaht, mut client) = two_outputs();
    assert_eq!(client.screenshot_of("HEADLESS-1").rgb(5, 5), SKY);
    let second = client.screenshot_of("HEADLESS-2");
    assert_eq!(second.rgb(5, 5), SKY);
    assert_eq!(second.rgb(second.width() - 1, second.height() - 1), SKY);
}

#[test]
fn the_taskbar_shows_on_the_output_the_settings_name() {
    let (_vaht, mut client) = with_settings("[taskbar]\noutput = \"HEADLESS-2\"\n");
    let first = client.screenshot_of("HEADLESS-1");
    assert_eq!(first.rgb(5, first.height() - 2), SKY, "no taskbar here");
    let second = client.screenshot_of("HEADLESS-2");
    assert_ne!(
        second.rgb(5, second.height() - 2),
        SKY,
        "the taskbar is here"
    );
}

#[test]
fn a_layer_surface_opens_on_the_output_the_pointer_is_on() {
    let (_vaht, mut client) = two_outputs();
    client.move_pointer_to(FIRST_WIDTH + 100, 100);
    client.open_backdrop(TestLayer::Background, BACKDROP);
    assert_eq!(client.screenshot_of("HEADLESS-1").rgb(5, 5), SKY);
    assert_eq!(client.screenshot_of("HEADLESS-2").rgb(5, 5), BACKDROP);
}

#[test]
fn outputs_go_where_outputs_toml_puts_them() {
    let (vaht, mut client) = with_files(
        "",
        "[[output]]\nmatch = { connector = \"HEADLESS-2\" }\nposition = [0, 0]\n\
         mode = \"1024x768\"\nprimary = true\n",
    );
    let listed = listed_outputs(&vaht);
    let summary: Vec<_> = listed
        .iter()
        .map(|output| {
            (
                output.connector.as_str(),
                output.position,
                output.is_primary,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("HEADLESS-1", [1024, 0], false),
            ("HEADLESS-2", [0, 0], true)
        ]
    );
    let second = client.screenshot_of("HEADLESS-2");
    assert_eq!((second.width(), second.height()), (1024, 768));
    // The taskbar shows on the primary output by default.
    assert_ne!(
        second.rgb(5, second.height() - 2),
        SKY,
        "the taskbar is here"
    );
    let first = client.screenshot_of("HEADLESS-1");
    assert_eq!(first.rgb(5, first.height() - 2), SKY, "no taskbar here");
}

#[test]
fn turning_an_output_off_brings_its_windows_to_the_primary_one() {
    let (vaht, mut client) = with_files("", "");
    client.move_pointer_to(FIRST_WIDTH + 100, 100);
    client.open_windows(1);
    rewrite_outputs(
        &vaht,
        "[[output]]\nmatch = { connector = \"HEADLESS-2\" }\nenabled = false\n",
    );
    let listed = listed_outputs(&vaht);
    assert!(!listed[1].is_enabled, "{listed:?}");
    assert!(
        client
            .screenshot_of("HEADLESS-1")
            .bounds_of(WINDOW_COLOR)
            .is_some(),
        "the window came to the first output"
    );
}

/// Two profiles, the first applying itself when HEADLESS-2 is there, and
/// hooks writing what they are told into `results`.
fn profiles(results: &Path) -> String {
    let hook = |name: &str| format!("printf '%s\\n' >> {}", results.join(name).display());
    format!(
        "[[profile]]\nname = \"right\"\nwhen_connected = [{{ connector = \"HEADLESS-2\" }}]\n\
         outputs = [{{ match = {{ connector = \"HEADLESS-2\" }}, position = [0, 0] }}]\n\n\
         [[profile]]\nname = \"both\"\n\
         primary = [{{ connector = \"HEADLESS-1\" }}, {{ connector = \"HEADLESS-2\" }}]\n\
         outputs = [\n  {{ match = {{ connector = \"HEADLESS-1\" }} }},\n  \
         {{ match = {{ connector = \"HEADLESS-2\" }} }},\n]\n\n\
         [hooks]\nprofile_applied = \"{}\"\nprimary_output_changed = \"{}\"\n",
        hook("profile"),
        hook("primary")
    )
}

#[test]
fn a_profile_applies_itself_and_cycling_moves_through_profiles_and_primaries() {
    let results = tempfile::tempdir().expect("a directory for the hooks");
    let (vaht, _client) = with_files("", &profiles(results.path()));
    let profile_file = results.path().join("profile");
    let primary_file = results.path().join("primary");
    assert_eq!(
        outputs_on(&vaht),
        ["*HEADLESS-2"],
        "the right profile applied itself"
    );
    // At start, the hooks hear of the profile and the primary output.
    wait_for_file(&profile_file, |text| text == "right\n");
    wait_for_file(&primary_file, |text| text == "HEADLESS-2\n");

    run(&vaht, &["cycle-profile"]);
    assert_eq!(outputs_on(&vaht), ["*HEADLESS-1", "HEADLESS-2"]);
    wait_for_file(&profile_file, |text| text == "right\nboth\n");
    wait_for_file(&primary_file, |text| text == "HEADLESS-2\nHEADLESS-1\n");

    run(&vaht, &["cycle-primary-output"]);
    assert_eq!(outputs_on(&vaht), ["HEADLESS-1", "*HEADLESS-2"]);
    wait_for_file(&primary_file, |text| {
        text == "HEADLESS-2\nHEADLESS-1\nHEADLESS-2\n"
    });

    run(&vaht, &["apply-profile", "right"]);
    assert_eq!(outputs_on(&vaht), ["*HEADLESS-2"]);
}
