//! Clients told when their frames showed (`wp_presentation`), and their
//! frames paced (`wp_fifo_v1`) and timed (`wp_commit_timing_v1`), as video
//! players and games use them.

use std::path::Path;
use std::time::Duration;

use vaht_test_support::{HeadlessVaht, Presentation, TestClient, monotonic_now};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// `CLOCK_MONOTONIC`, as `clock_gettime` numbers it.
const CLOCK_MONOTONIC: u32 = 1;

/// The headless outputs' refresh interval: 60 Hz.
const HEADLESS_REFRESH: Duration = Duration::from_nanos(16_666_666);

#[test]
fn a_frame_is_reported_shown_by_the_monotonic_clock() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let before = monotonic_now();
    let reported = client.redraw_reported(0, [0x20, 0x40, 0x80]);
    let after = monotonic_now();
    let Presentation::Shown {
        clock,
        time,
        refresh,
        ..
    } = reported
    else {
        panic!("the frame was not shown: {reported:?}");
    };
    assert_eq!(clock, CLOCK_MONOTONIC);
    assert!(
        (before..=after).contains(&time),
        "{time:?} is not between {before:?} and {after:?}"
    );
    assert_eq!(refresh, HEADLESS_REFRESH);
}

#[test]
fn each_frame_shown_comes_later_in_the_outputs_count() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let sequence = |reported| match reported {
        Presentation::Shown { sequence, .. } => sequence,
        Presentation::Discarded => panic!("the frame was not shown"),
    };
    let first = sequence(client.redraw_reported(0, [0x20, 0x40, 0x80]));
    let second = sequence(client.redraw_reported(0, [0x80, 0x40, 0x20]));
    assert!(second > first, "{second} follows {first}");
}

#[test]
fn paced_frames_each_show_in_turn() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let colors = [
        [0x10, 0x20, 0x30],
        [0x20, 0x30, 0x40],
        [0x30, 0x40, 0x50],
        [0x40, 0x50, 0x60],
    ];
    let reported = client.redraw_paced(0, &colors);
    assert!(
        matches!(reported, Presentation::Shown { .. }),
        "the last frame showed: {reported:?}"
    );
    assert!(
        client.screenshot().bounds_of([0x40, 0x50, 0x60]).is_some(),
        "the window shows its last frame"
    );
}

#[test]
fn a_timed_frame_shows_no_sooner_than_asked() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let at = monotonic_now() + Duration::from_millis(300);
    let reported = client.redraw_at(0, [0x60, 0x20, 0x20], at);
    let Presentation::Shown { time, .. } = reported else {
        panic!("the frame was not shown: {reported:?}");
    };
    // It is let in by the frame that shows within a refresh of the moment.
    assert!(
        time + HEADLESS_REFRESH >= at,
        "shown at {time:?}, more than a refresh before {at:?}"
    );
    assert!(
        time < at + Duration::from_secs(1),
        "shown at {time:?}, long after {at:?}"
    );
}
