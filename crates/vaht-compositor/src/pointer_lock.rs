//! Where a locked or confined pointer may go (`pointer-constraints`), and
//! whether a client may move it (`pointer-warp`), decided from plain
//! values.
//!
//! A game locks the pointer (it stays put; the game reads relative motion)
//! or confines it to its window or a region of it. The compositor reads
//! which constraint holds, if any, and asks [`constrain`] where a motion
//! takes the pointer. A client may also move the pointer within its own
//! surface while it has the pointer, as an X game's warp through Xwayland
//! does; [`may_warp`] says whether a request may.

use smithay::utils::{Logical, Point, Serial, Size};

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

/// Returns whether a client may move the pointer to `position` on its
/// surface of `size`: the surface has the pointer, the client names the
/// serial of the pointer's last entering one of its surfaces
/// (`entered`), and the position is on the surface.
pub(crate) fn may_warp(
    is_focused: bool,
    (serial, entered): (Serial, Option<Serial>),
    position: Point<f64, Logical>,
    size: Size<i32, Logical>,
) -> bool {
    let size = size.to_f64();
    is_focused
        && entered == Some(serial)
        && (0.0..size.w).contains(&position.x)
        && (0.0..size.h).contains(&position.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_moves_the_pointer_only_on_its_surface_while_it_has_it() {
        let size = Size::from((100, 50));
        let serial = Serial::from(7);
        let inside = Point::from((10.0, 20.0));
        assert!(may_warp(true, (serial, Some(serial)), inside, size));
        assert!(
            !may_warp(false, (serial, Some(serial)), inside, size),
            "not focused"
        );
        assert!(
            !may_warp(true, (serial, Some(Serial::from(8))), inside, size),
            "a stale serial"
        );
        assert!(
            !may_warp(true, (serial, None), inside, size),
            "never entered"
        );
        assert!(
            !may_warp(true, (serial, Some(serial)), (100.0, 20.0).into(), size),
            "off the surface's right edge"
        );
        assert!(
            !may_warp(true, (serial, Some(serial)), (-1.0, 20.0).into(), size),
            "off its left edge"
        );
    }

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
