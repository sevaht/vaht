//! Leak tests: vaht running headless, driven by real Wayland clients.
//!
//! Each test opens and closes windows from test clients, stops vaht with
//! SIGTERM and reads its exit report. It must show every window, frame,
//! client and capture session gone, and as many open descriptors as at
//! startup: anything else is a leak.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, TestClient, TestLayer};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// Starts a headless vaht for one test.
fn start_vaht() -> HeadlessVaht {
    HeadlessVaht::start(Path::new(EXECUTABLE))
}

#[test]
fn windows_are_shown_and_leave_nothing_behind_when_closed() {
    let vaht = start_vaht();
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(3);
    client.close_windows();
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn many_clients_coming_and_going_leave_nothing_behind() {
    const ROUNDS: usize = 20;
    const WINDOWS_PER_CLIENT: usize = 5;
    let vaht = start_vaht();
    for _ in 0..ROUNDS {
        let mut client = TestClient::connect(vaht.socket());
        client.open_windows(WINDOWS_PER_CLIENT);
        client.close_windows();
    }
    vaht.wait_for_disconnections(ROUNDS);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn a_client_that_disconnects_with_windows_open_leaves_nothing_behind() {
    let vaht = start_vaht();
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(4);
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn a_client_that_only_takes_screenshots_leaves_nothing_behind() {
    let vaht = start_vaht();
    let mut client = TestClient::connect(vaht.socket());
    let _ = client.screenshot();
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn screenshots_leave_nothing_behind() {
    let vaht = start_vaht();
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(2);
    for _ in 0..5 {
        let _ = client.screenshot();
    }
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn clients_find_the_protocols_everyday_programs_use() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let client = TestClient::connect(vaht.socket());
    let interfaces = client.global_interfaces();
    for expected in [
        "ext_data_control_manager_v1",
        "ext_foreign_toplevel_list_v1",
        "wl_compositor",
        "wl_data_device_manager",
        "wl_fixes",
        "wl_seat",
        "wl_shm",
        "wp_alpha_modifier_v1",
        "wp_commit_timing_manager_v1",
        "wp_content_type_manager_v1",
        "wp_cursor_shape_manager_v1",
        "wp_fifo_manager_v1",
        "wp_fractional_scale_manager_v1",
        "wp_pointer_warp_v1",
        "wp_presentation",
        "wp_single_pixel_buffer_manager_v1",
        "wp_viewporter",
        "xdg_activation_v1",
        "xdg_system_bell_v1",
        "xdg_toplevel_tag_manager_v1",
        "xdg_wm_base",
        "zwlr_data_control_manager_v1",
        "zwp_pointer_gestures_v1",
        "zwp_primary_selection_device_manager_v1",
        "zxdg_decoration_manager_v1",
        "zxdg_exporter_v2",
        "zxdg_importer_v2",
    ] {
        assert!(
            interfaces.iter().any(|name| name == expected),
            "{expected} missing: {interfaces:?}"
        );
    }
    assert_eq!(
        interfaces
            .iter()
            .filter(|name| *name == "zwp_primary_selection_device_manager_v1")
            .count(),
        1,
        "one primary selection"
    );
    assert!(
        !interfaces.iter().any(|name| name == "zwp_linux_dmabuf_v1"),
        "the software renderer cannot import GPU buffers, so clients must not be offered them"
    );
}

#[test]
fn layer_surfaces_leave_nothing_behind() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_bar(TestLayer::Top, 30, true, [200, 10, 10]);
    client.open_backdrop(TestLayer::Background, [10, 200, 10]);
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn a_client_leaving_after_its_windows_closed_and_were_redrawn_leaves_nothing_behind() {
    // The frame after the windows close is drawn before the client leaves,
    // so only the client's leaving can prompt the cleanup frame that
    // wait_for_disconnections waits for.
    let vaht = start_vaht();
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(2);
    let drawn = vaht.logged_count("frame drawn");
    client.close_windows();
    vaht.wait_for_logged("frame drawn", drawn + 1);
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn programs_are_told_to_prefer_wayland() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let written = directory.path().join("toolkits");
    let settings = format!(
        "[session]\nautostart = [\"echo \\\"$QT_QPA_PLATFORM|$SDL_VIDEODRIVER\\\" > {}\"]\n",
        written.display()
    );
    let _vaht =
        HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("settings.toml", &settings)]);
    let toolkits = vaht_test_support::wait_for_file(&written, |text| text.ends_with('\n'));
    assert_eq!(toolkits, "wayland;xcb|wayland,x11\n");
}

/// Fake `dbus-update-activation-environment` and `systemctl` programs that
/// record what they are told, so a test never touches the user's own
/// service manager.
struct FakeSessionTools {
    /// Holds the programs.
    directory: tempfile::TempDir,
    /// What they were told, a line each.
    calls: std::path::PathBuf,
}

impl FakeSessionTools {
    /// Makes the programs; `delay` is how long the first one sleeps.
    fn new(delay: &str) -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let calls = directory.path().join("calls");
        let tool = directory.path().join("dbus-update-activation-environment");
        std::fs::write(
            &tool,
            format!(
                "#!/bin/sh\nsleep {delay}\necho \"$*\" >> {}\n",
                calls.display()
            ),
        )
        .expect("written");
        let service_manager = directory.path().join("systemctl");
        std::fs::write(
            &service_manager,
            format!("#!/bin/sh\necho \"systemctl $*\" >> {}\n", calls.display()),
        )
        .expect("written");
        for program in [&tool, &service_manager] {
            std::fs::set_permissions(program, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .expect("made runnable");
        }
        Self { directory, calls }
    }

    /// Returns the `PATH` vaht is started with: the programs first.
    fn path(&self) -> String {
        format!(
            "{}:{}",
            self.directory.path().display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }
}

#[test]
fn the_activation_environment_learns_of_the_session_and_forgets_it_on_exit() {
    // Slow, so autostart racing it would find nothing.
    let tools = FakeSessionTools::new("0.2");
    let (calls, seen) = (tools.calls.clone(), tools.directory.path().join("seen"));
    let vaht = HeadlessVaht::start_with_environment(
        Path::new(EXECUTABLE),
        &[(
            "settings.toml",
            format!(
                "[session]\nupdate_activation_environment = \"always\"\n\
                 autostart = [\"cat {calls} > {seen}.part; mv {seen}.part {seen}\"]\n",
                calls = calls.display(),
                seen = seen.display(),
            )
            .as_bytes(),
        )],
        &[("PATH", &tools.path())],
    );
    let autostarted = vaht_test_support::wait_for_file(&seen, |_| true);
    assert!(
        autostarted.contains("WAYLAND_DISPLAY"),
        "autostart runs once services can find the session: {autostarted:?}"
    );
    let exported =
        vaht_test_support::wait_for_file(&calls, |text| text.contains("WAYLAND_DISPLAY"));
    assert!(
        exported.starts_with("--systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP"),
        "{exported}"
    );
    assert!(
        exported.contains(
            "systemctl --user stop vaht-session.target\n\
             systemctl --user start vaht-session.target\n"
        ),
        "the session target starts afresh after the export: {exported}"
    );
    let _report = vaht.stop();
    let all = vaht_test_support::wait_for_file(&calls, |text| text.contains("WAYLAND_DISPLAY="));
    assert!(all.contains("XDG_CURRENT_DESKTOP= "), "{all}");
    let ending = &all[all.rfind("systemctl --user stop").expect("stopped at exit")..];
    assert!(
        ending.contains("WAYLAND_DISPLAY="),
        "the target stops before the environment is emptied: {all}"
    );
}

#[test]
fn the_activation_environment_follows_its_setting_while_vaht_runs() {
    let tools = FakeSessionTools::new("0");
    let vaht = HeadlessVaht::start_with_environment(
        Path::new(EXECUTABLE),
        &[(
            "settings.toml",
            b"[session]\nupdate_activation_environment = \"never\"\n",
        )],
        &[("PATH", &tools.path())],
    );
    let rewrite = |value: &str| {
        let reloads = vaht.logged_count("configuration reloaded");
        std::fs::write(
            vaht.config_directory().join("settings.toml"),
            format!("[session]\nupdate_activation_environment = \"{value}\"\n"),
        )
        .expect("the settings are written");
        vaht.wait_for_logged("configuration reloaded", reloads + 1);
    };
    rewrite("always");
    let told = vaht_test_support::wait_for_file(&tools.calls, |text| {
        text.contains("systemctl --user start vaht-session.target")
    });
    assert!(told.contains("WAYLAND_DISPLAY"), "{told}");
    rewrite("never");
    let forgotten =
        vaht_test_support::wait_for_file(&tools.calls, |text| text.contains("WAYLAND_DISPLAY="));
    assert!(
        forgotten
            .rfind("systemctl --user stop")
            .is_some_and(|stop| { forgotten[stop..].contains("WAYLAND_DISPLAY=") }),
        "the session target stops, then the environment is emptied: {forgotten}"
    );
}

#[test]
fn a_headless_vaht_leaves_the_activation_environment_alone() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let marker = directory.path().join("started");
    let settings = format!("[session]\nautostart = [\"touch {}\"]\n", marker.display());
    let vaht =
        HeadlessVaht::start_with_config(Path::new(EXECUTABLE), &[("settings.toml", &settings)]);
    // Autostart runs after the export would have been logged.
    let _ = vaht_test_support::wait_for_file(&marker, |_| true);
    assert_eq!(vaht.logged_count("activation environment updated"), 0);
}
