//! Wallpaper tests: the image the settings name under everything, and
//! pseudo-transparent windows showing only it through.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, TestClient, WINDOW_COLOR, keys, solid_png};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// The wallpaper's color.
const SKY: [u8; 3] = [0x30, 0xa0, 0x50];

/// The middle of the headless output, where the test windows overlap.
const MIDDLE: (u32, u32) = (960, 540);

/// Starts vaht with a solid wallpaper, tiled, and `rules`.
fn start(rules: &str) -> (HeadlessVaht, TestClient) {
    start_with_bindings(rules, "")
}

/// Starts vaht with a solid wallpaper, tiled, `rules` and `bindings`.
fn start_with_bindings(rules: &str, bindings: &str) -> (HeadlessVaht, TestClient) {
    let picture = solid_png(16, SKY);
    let settings = "[wallpaper]\nimage = \"sky.png\"\nfit = \"tile\"\n";
    let vaht = HeadlessVaht::start_with_files(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", settings.as_bytes()),
            ("rules.toml", rules.as_bytes()),
            ("bindings.toml", bindings.as_bytes()),
            ("sky.png", &picture),
        ],
        &[],
    );
    let client = TestClient::connect(vaht.socket());
    (vaht, client)
}

/// Rules that put every test window in the middle, so they overlap, and
/// `extra` keys.
fn centered(extra: &str) -> String {
    format!(
        "[[rule]]\nmatch = {{ app_id = \"vaht.test\" }}\nposition = {{ anchor = \"center\" }}\n{extra}"
    )
}

#[test]
fn the_wallpaper_covers_the_desktop() {
    let (_vaht, mut client) = start("");
    let screenshot = client.screenshot();
    assert_eq!(screenshot.rgb(5, 300), SKY);
    assert_eq!(screenshot.rgb(1900, 1000), SKY);
}

#[test]
fn a_clear_window_shows_the_window_below_it() {
    let (_vaht, mut client) = start(&centered(""));
    client.open_windows(2);
    client.clear_window(1);
    assert_eq!(client.screenshot().rgb(MIDDLE.0, MIDDLE.1), WINDOW_COLOR);
}

#[test]
fn a_pseudo_transparent_window_shows_only_the_wallpaper() {
    let (vaht, mut client) = start(&centered("pseudo_transparent = true\n"));
    client.open_windows(2);
    client.clear_window(1);
    assert_eq!(client.screenshot().rgb(MIDDLE.0, MIDDLE.1), SKY);
    client.close_windows();
    drop(client);
    vaht.wait_for_disconnections(1);
    vaht.stop().assert_nothing_left_behind();
}

#[test]
fn set_alpha_makes_a_window_translucent_and_nothing_restores_it() {
    let (_vaht, mut client) = start_with_bindings(
        &centered(""),
        "[keys]\nF1 = \"set-alpha 0\"\nF2 = \"set-alpha\"\n",
    );
    client.open_windows(1);
    client.type_chord(&[keys::F1]);
    assert_eq!(client.screenshot().rgb(MIDDLE.0, MIDDLE.1), SKY);
    client.type_chord(&[keys::F2]);
    assert_eq!(client.screenshot().rgb(MIDDLE.0, MIDDLE.1), WINDOW_COLOR);
}

#[test]
fn a_workspace_with_its_own_image_shows_it_while_shown() {
    const BEACH: [u8; 3] = [0xe0, 0xc0, 0x80];
    let settings = "[wallpaper]\nimage = \"sky.png\"\nfit = \"tile\"\n\
                    [wallpaper.workspaces]\n2 = \"beach.png\"\n";
    let vaht = HeadlessVaht::start_with_files(
        Path::new(EXECUTABLE),
        &[
            ("settings.toml", settings.as_bytes()),
            ("sky.png", &solid_png(16, SKY)),
            ("beach.png", &solid_png(16, BEACH)),
        ],
        &[],
    );
    let mut client = TestClient::connect(vaht.socket());
    let run = |command: &str| {
        let output = std::process::Command::new(EXECUTABLE)
            .args(["run", command])
            .env(vaht_ipc::SOCKET_VARIABLE, vaht.ipc_socket())
            .output()
            .expect("vaht runs");
        assert!(output.status.success(), "{output:?}");
    };
    assert_eq!(client.screenshot().rgb(5, 300), SKY);
    run("workspace 2");
    assert_eq!(client.screenshot().rgb(5, 300), BEACH, "its own image");
    run("workspace 3");
    assert_eq!(client.screenshot().rgb(5, 300), SKY, "the settings' image");
}

#[test]
fn a_translucent_taskbar_shows_the_wallpaper_through() {
    let taskbar_at = |alpha: u8| -> [u8; 3] {
        let style = format!("[taskbar]\ntexture = {{ solid = \"#000000\" }}\nalpha = {alpha}\n");
        let settings = "style = \"see-through\"\n[wallpaper]\nimage = \"sky.png\"\nfit = \"tile\"\n\
                        [taskbar]\nwidgets = []\n";
        let vaht = HeadlessVaht::start_with_files(
            Path::new(EXECUTABLE),
            &[
                ("settings.toml", settings.as_bytes()),
                ("styles/see-through.toml", style.as_bytes()),
                ("sky.png", &solid_png(16, SKY)),
            ],
            &[],
        );
        let mut client = TestClient::connect(vaht.socket());
        let screenshot = client.screenshot();
        screenshot.rgb(100, screenshot.height() - 2)
    };
    assert_eq!(taskbar_at(255), [0, 0, 0], "opaque");
    assert_eq!(taskbar_at(0), SKY, "clear");
    let half = taskbar_at(128);
    for (blended, sky) in half.iter().zip(SKY) {
        assert!(
            (i32::from(*blended) - i32::from(sky) / 2).abs() <= 2,
            "half of the wallpaper: {half:?}"
        );
    }
}
