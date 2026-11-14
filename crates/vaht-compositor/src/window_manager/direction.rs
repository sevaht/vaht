//! Which window lies in a direction from another (`focus-direction`),
//! decided from plain rectangles.
//!
//! A window is in a direction when its middle lies that way from the
//! focused window's middle, within 45 degrees of it: a window far to the
//! right and a little lower is to the right, not below. Of those, the
//! nearest wins, distance across the direction counting double, so a
//! window straight ahead beats one a little nearer but off to the side.

use smithay::utils::{Logical, Rectangle};

use super::Direction;

/// Returns the key of the nearest of `candidates` (each a key and its
/// area) in `direction` from `from`; none if none lies that way.
pub(crate) fn nearest_in<Key: Copy>(
    from: Rectangle<i32, Logical>,
    direction: Direction,
    candidates: impl IntoIterator<Item = (Key, Rectangle<i32, Logical>)>,
) -> Option<Key> {
    let middle = |area: Rectangle<i32, Logical>| {
        (
            i64::from(area.loc.x) * 2 + i64::from(area.size.w),
            i64::from(area.loc.y) * 2 + i64::from(area.size.h),
        )
    };
    let (from_x, from_y) = middle(from);
    candidates
        .into_iter()
        .filter_map(|(key, area)| {
            let (x, y) = middle(area);
            let (dx, dy) = (x - from_x, y - from_y);
            let (ahead, across) = match direction {
                Direction::Left => (-dx, dy),
                Direction::Right => (dx, dy),
                Direction::Up => (-dy, dx),
                Direction::Down => (dy, dx),
            };
            (ahead > 0 && ahead >= across.abs()).then_some((ahead + across.abs() * 2, key))
        })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, key)| key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 100×100 square at `(x, y)`.
    fn square(x: i32, y: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (100, 100).into())
    }

    #[test]
    fn the_nearest_window_that_way_wins() {
        let from = square(500, 500);
        let windows = [
            ("left", square(200, 500)),
            ("far left", square(0, 500)),
            ("right", square(800, 520)),
            ("above", square(500, 100)),
        ];
        assert_eq!(nearest_in(from, Direction::Left, windows), Some("left"));
        assert_eq!(nearest_in(from, Direction::Right, windows), Some("right"));
        assert_eq!(nearest_in(from, Direction::Up, windows), Some("above"));
        assert_eq!(
            nearest_in(from, Direction::Down, windows),
            None,
            "none below"
        );
    }

    #[test]
    fn a_window_straight_ahead_beats_a_nearer_one_off_to_the_side() {
        let from = square(500, 500);
        let windows = [("ahead", square(800, 500)), ("aside", square(650, 900))];
        assert_eq!(nearest_in(from, Direction::Right, windows), Some("ahead"));
    }
}
