//! Where a locked or confined pointer may go (`pointer-constraints`),
//! decided from plain values.
//!
//! A game locks the pointer (it stays put; the game reads relative motion)
//! or confines it to its window or a region of it. The compositor reads
//! which constraint holds, if any, and asks [`constrain`] where a motion
//! takes the pointer.

use smithay::utils::{Logical, Point};

/// What holds the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointerHold {
    /// It does not move at all.
    Locked,
    /// It moves only within an area.
    Confined,
}

/// Returns where the pointer at `from` goes when moved by `delta` under
/// `hold`: locked, nowhere; confined, along each axis only as far as it
/// stays where `is_allowed` says it may be, so it slides along an edge
/// rather than sticking to it.
pub(crate) fn constrain(
    hold: PointerHold,
    from: Point<f64, Logical>,
    delta: Point<f64, Logical>,
    is_allowed: impl Fn(Point<f64, Logical>) -> bool,
) -> Point<f64, Logical> {
    match hold {
        PointerHold::Locked => from,
        PointerHold::Confined => {
            let across = Point::from((from.x + delta.x, from.y));
            let x = if is_allowed(across) { across.x } else { from.x };
            let down = Point::from((x, from.y + delta.y));
            let y = if is_allowed(down) { down.y } else { from.y };
            Point::from((x, y))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `point` is within the square from 0 to 100.
    fn in_square(point: Point<f64, Logical>) -> bool {
        (0.0..100.0).contains(&point.x) && (0.0..100.0).contains(&point.y)
    }

    #[test]
    fn a_locked_pointer_stays_put() {
        let from = Point::from((50.0, 50.0));
        assert_eq!(
            constrain(PointerHold::Locked, from, (30.0, -10.0).into(), in_square),
            from
        );
    }

    #[test]
    fn a_confined_pointer_moves_freely_inside() {
        let moved = constrain(
            PointerHold::Confined,
            (50.0, 50.0).into(),
            (10.0, 20.0).into(),
            in_square,
        );
        assert_eq!(moved, Point::from((60.0, 70.0)));
    }

    #[test]
    fn a_confined_pointer_slides_along_the_edge_it_meets() {
        let moved = constrain(
            PointerHold::Confined,
            (95.0, 50.0).into(),
            (20.0, 10.0).into(),
            in_square,
        );
        assert_eq!(
            moved,
            Point::from((95.0, 60.0)),
            "held at the right edge, free down it"
        );
    }
}
