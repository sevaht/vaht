//! Several outputs: each shows its own part of the layout, which runs
//! from the left in the order the backend finds them, and windows move
//! from one to the next.

use std::path::Path;
use std::process::Command;

use vaht_ipc::{OutputInfo, Reply};
use vaht_test_support::{
    BUILT_IN_METRICS, BUTTON_LEFT, Bounds, ConfigurationResult, GammaControl, HeadChange,
    HeadlessVaht, OutputConfigurator, OutputPowerControl, TestClient, TestLayer, WINDOW_COLOR,
    keys, solid_png, wait_for_file,
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

#[test]
fn older_tools_copy_the_screen_or_a_region_of_it() {
    let (_vaht, mut client) = with_files("", "");
    for waits_for_damage in [false, true] {
        let whole = client
            .wlr_screenshot(None, waits_for_damage)
            .expect("the screen copies");
        assert_eq!((whole.width(), whole.height()), (800, 600));
        assert_eq!(whole.rgb(400, 300), SKY, "the wallpaper");
        let part = client
            .wlr_screenshot(Some((100, 50, 20, 10)), waits_for_damage)
            .expect("the region copies");
        assert_eq!((part.width(), part.height()), (20, 10));
        assert_eq!(part.rgb(10, 5), SKY);
    }
}

#[test]
fn a_window_is_told_the_scale_of_the_output_it_is_on() {
    let (_vaht, mut client) = with_files(
        "",
        "[[output]]\nmatch = { connector = \"HEADLESS-1\" }\nscale = 1.5\n",
    );
    client.open_windows(1);
    // Apps that scale themselves (Firefox, GTK 4, Qt 6) draw at the
    // output's scale, not at 1 to be enlarged blurry; others draw at the
    // whole scale above it.
    assert_eq!(client.wait_for_fractional_scale(1.5), Some(1.5));
    assert_eq!(client.wait_for_preferred_scale(2), Some(2));
}

/// Returns the outputs `vaht outputs` lists.
fn listed_outputs(vaht: &HeadlessVaht) -> Vec<OutputInfo> {
    let output = Command::new(EXECUTABLE)
        .args(["--json", "outputs"])
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht runs");
    let reply: Reply = serde_json::from_slice(&output.stdout).expect("one JSON reply");
    let Reply::Outputs { outputs, .. } = reply else {
        panic!("not an output list: {reply:?}");
    };
    outputs
}

/// Runs the command `command` in `vaht`, as `vaht run` does, and
/// waits until it is carried out.
fn run(vaht: &HeadlessVaht, command: &[&str]) {
    let output = Command::new(EXECUTABLE)
        .args(["run"])
        .args(command)
        .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
        .output()
        .expect("vaht runs");
    assert!(output.status.success(), "{command:?}: {output:?}");
}

/// Returns which outputs `vaht outputs` lists as on, by connector,
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
fn taskbar_to_output_moves_the_taskbar_by_head_or_connector() {
    let (vaht, mut client) = two_outputs();
    let has_taskbar = |client: &mut TestClient, connector: &str| {
        let screenshot = client.screenshot_of(connector);
        screenshot.rgb(5, screenshot.height() - 2) != SKY
    };
    assert!(
        has_taskbar(&mut client, "HEADLESS-1"),
        "on the primary output"
    );
    run(&vaht, &["taskbar-to-output", "next"]);
    assert!(!has_taskbar(&mut client, "HEADLESS-1"));
    assert!(
        has_taskbar(&mut client, "HEADLESS-2"),
        "moved to the next head"
    );
    run(&vaht, &["taskbar-to-output", "HEADLESS-1"]);
    assert!(
        has_taskbar(&mut client, "HEADLESS-1"),
        "back by its connector"
    );
    assert!(!has_taskbar(&mut client, "HEADLESS-2"));
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

/// A desk of two monitors whose outputs file is rewritten to plug and
/// unplug them, with a client.
struct Desk {
    /// The compositor.
    vaht: HeadlessVaht,
    /// The client.
    client: TestClient,
    /// Holds the outputs file.
    directory: tempfile::TempDir,
}

impl Desk {
    /// Starts vaht with LEFT (800×600) and RIGHT (640×480) plugged in.
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("a directory for the outputs file");
        let file = directory.path().join("outputs");
        std::fs::write(&file, "LEFT 800x600\nRIGHT 640x480\n").expect("the file is written");
        let vaht = HeadlessVaht::start_with_output_file(Path::new(EXECUTABLE), &file, &[]);
        let client = TestClient::connect(vaht.socket());
        Self {
            vaht,
            client,
            directory,
        }
    }

    /// Makes the monitors plugged in the ones `outputs` lists, and waits
    /// until vaht has followed: `event` logged once more.
    fn plug(&self, outputs: &str, event: &str) {
        let seen = self.vaht.logged_count(event);
        let file = self.directory.path().join("outputs");
        let temporary = self.directory.path().join("outputs.new");
        std::fs::write(&temporary, outputs).expect("the file is written");
        std::fs::rename(&temporary, &file).expect("the file is replaced");
        self.vaht.wait_for_logged(event, seen + 1);
    }
}

#[test]
fn a_monitor_plugged_in_joins_the_layout_on_the_right() {
    let mut desk = Desk::new();
    desk.plug(
        "LEFT 800x600\nRIGHT 640x480\nTHIRD 320x240\n",
        "output connected",
    );
    let listed = listed_outputs(&desk.vaht);
    let third = listed
        .iter()
        .find(|output| output.connector == "THIRD")
        .expect("listed");
    assert_eq!(third.position, [1440, 0]);
    let screenshot = desk.client.screenshot_of("THIRD");
    assert_eq!((screenshot.width(), screenshot.height()), (320, 240));
}

#[test]
fn unplugging_a_monitor_brings_its_windows_to_the_others() {
    let mut desk = Desk::new();
    desk.client.move_pointer_to(900, 100);
    desk.client.open_windows(1);
    assert!(
        desk.client
            .screenshot_of("RIGHT")
            .bounds_of(WINDOW_COLOR)
            .is_some(),
        "the window opened on the right"
    );
    desk.plug("LEFT 800x600\n", "output disconnected");
    assert_eq!(
        outputs_on(&desk.vaht),
        ["*LEFT"],
        "only the left monitor is left"
    );
    assert!(
        desk.client
            .screenshot_of("LEFT")
            .bounds_of(WINDOW_COLOR)
            .is_some(),
        "the window came to the left monitor"
    );
}

#[test]
fn a_window_goes_back_to_its_monitor_when_it_is_plugged_in_again() {
    let mut desk = Desk::new();
    desk.client.move_pointer_to(900, 100);
    desk.client.open_windows(1);
    let on_right = desk
        .client
        .screenshot_of("RIGHT")
        .bounds_of(WINDOW_COLOR)
        .expect("the window opened on the right");
    desk.plug("LEFT 800x600\n", "output disconnected");
    desk.plug("LEFT 800x600\nRIGHT 640x480\n", "output connected");
    let back = desk
        .client
        .screenshot_of("RIGHT")
        .bounds_of(WINDOW_COLOR)
        .expect("the window is back on the right");
    assert_eq!(
        (back.left, back.top),
        (on_right.left, on_right.top),
        "where it was"
    );
    assert!(
        desk.client
            .screenshot_of("LEFT")
            .bounds_of(WINDOW_COLOR)
            .is_none(),
        "and gone from the left"
    );
}

#[test]
fn a_window_moved_while_its_monitor_was_away_stays_put() {
    let mut desk = Desk::new();
    desk.client.move_pointer_to(900, 100);
    desk.client.open_windows(1);
    desk.plug("LEFT 800x600\n", "output disconnected");
    run(&desk.vaht, &["move", "-100", "10"]);
    desk.plug("LEFT 800x600\nRIGHT 640x480\n", "output connected");
    assert!(
        desk.client
            .screenshot_of("RIGHT")
            .bounds_of(WINDOW_COLOR)
            .is_none(),
        "the user put it on the left"
    );
}

#[test]
fn unplugging_the_primary_monitor_moves_the_taskbar() {
    let mut desk = Desk::new();
    desk.plug("RIGHT 640x480\n", "output disconnected");
    assert_eq!(outputs_on(&desk.vaht), ["*RIGHT"]);
    let right = desk.client.screenshot_of("RIGHT");
    assert_ne!(
        right.rgb(5, right.height() - BUILT_IN_METRICS.taskbar_height / 2),
        right.rgb(5, 5),
        "the taskbar is on the right monitor now"
    );
}

#[test]
fn a_monitor_whose_mode_changes_is_laid_out_again() {
    let desk = Desk::new();
    desk.plug("LEFT 1024x768\nRIGHT 640x480\n", "output changed");
    let listed = listed_outputs(&desk.vaht);
    let right = listed
        .iter()
        .find(|output| output.connector == "RIGHT")
        .expect("listed");
    assert_eq!(right.position, [1024, 0], "moved over for the wider left");
}

#[test]
fn a_panel_on_a_monitor_unplugged_is_closed() {
    let mut desk = Desk::new();
    desk.client.move_pointer_to(900, 100);
    desk.client.open_backdrop(TestLayer::Background, BACKDROP);
    assert_eq!(desk.client.closed_layers(), 0);
    desk.plug("LEFT 800x600\n", "output disconnected");
    assert_eq!(
        desk.client.closed_layers(),
        1,
        "told, so it can open again elsewhere"
    );
}

#[test]
fn an_output_turned_off_comes_on_again_when_asked_or_at_any_input() {
    let (vaht, mut client) = two_outputs();
    let mut control = OutputPowerControl::connect(vaht.socket());
    let mut watcher = OutputPowerControl::connect(vaht.socket());
    assert!(control.wait_for(true), "on to begin with");
    control.set(false);
    assert!(watcher.wait_for(false), "every control is told");
    control.set(true);
    assert!(watcher.wait_for(true));
    control.set(false);
    assert!(watcher.wait_for(false));
    client.move_pointer_to(10, 10);
    assert!(control.wait_for(true), "input wakes it");
}

#[test]
fn one_night_light_at_a_time_changes_an_outputs_colors() {
    let (vaht, _client) = two_outputs();
    let mut first = GammaControl::connect(vaht.socket());
    let size = first.size().expect("told the ramps' size");
    let second = GammaControl::connect(vaht.socket());
    assert_eq!(second.size(), None, "another program controls the output");

    let ramps: Vec<u16> = (0..size * 3)
        .map(|level| u16::try_from(level).unwrap_or(0))
        .collect();
    first.send_ramps(&ramps);
    vaht.wait_for_logged("gamma set output=\"HEADLESS-1\" is_normal=false", 1);
    first.release();
    vaht.wait_for_logged("gamma set output=\"HEADLESS-1\" is_normal=true", 1);

    let mut third = GammaControl::connect(vaht.socket());
    assert!(third.size().is_some(), "free again");
    third.send_ramps(&ramps[..3]);
    assert!(third.has_failed(), "too few values");
}

#[test]
fn a_program_arranging_outputs_sees_them_and_moves_them() {
    let (vaht, _client) = two_outputs();
    let mut randr = OutputConfigurator::connect(vaht.socket());
    let heads = randr.wait_for(|heads| heads.len() == 2);
    let summary: Vec<_> = heads
        .iter()
        .map(|head| {
            (
                head.name.as_str(),
                head.is_enabled,
                head.position,
                head.size,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("HEADLESS-1", true, (0, 0), Some((800, 600))),
            ("HEADLESS-2", true, (800, 0), Some((640, 480))),
        ]
    );
    let stale = randr.serial().map(|serial| serial.wrapping_sub(1));

    assert_eq!(
        randr.apply(
            &[
                ("HEADLESS-2", HeadChange::At(0, 0)),
                ("HEADLESS-1", HeadChange::At(640, 0))
            ],
            None
        ),
        Some(ConfigurationResult::Succeeded)
    );
    let heads = randr.wait_for(|heads| heads.iter().any(|head| head.position == (640, 0)));
    assert_eq!(heads[0].position, (640, 0), "told of the change");
    let listed: Vec<_> = listed_outputs(&vaht)
        .iter()
        .map(|output| (output.connector.clone(), output.position))
        .collect();
    assert_eq!(
        listed,
        [
            ("HEADLESS-1".to_owned(), [640, 0]),
            ("HEADLESS-2".to_owned(), [0, 0])
        ]
    );

    assert_eq!(
        randr.apply(&[("HEADLESS-1", HeadChange::At(0, 0))], stale),
        Some(ConfigurationResult::Cancelled),
        "made for outputs since changed"
    );
    assert_eq!(
        randr.apply(
            &[
                ("HEADLESS-1", HeadChange::Off),
                ("HEADLESS-2", HeadChange::Off)
            ],
            None
        ),
        Some(ConfigurationResult::Failed),
        "every output off"
    );
    assert_eq!(
        randr.apply(&[("HEADLESS-2", HeadChange::Off)], None),
        Some(ConfigurationResult::Succeeded)
    );
    assert_eq!(outputs_on(&vaht), ["*HEADLESS-1"]);
}

#[test]
fn a_turned_output_is_captured_as_it_shows() {
    let (_vaht, mut client) = with_files(
        "",
        "[[output]]\nmatch = { connector = \"HEADLESS-1\" }\ntransform = \"90\"\n",
    );
    let turned = client.screenshot_of("HEADLESS-1");
    assert_eq!((turned.width(), turned.height()), (600, 800), "upright");
    assert_ne!(
        turned.rgb(5, turned.height() - 2),
        SKY,
        "the taskbar along its bottom"
    );
    assert_eq!(turned.rgb(5, 2), SKY, "the wallpaper above");
}
