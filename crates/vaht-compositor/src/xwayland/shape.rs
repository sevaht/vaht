//! X11 windows' shapes (the X Shape extension), clipped exactly (04 §8.1).
//!
//! Xwayland does not show a window's bounding shape, so vaht must: Wine's
//! layered windows and Proton's focus overlays are rings, and drawn whole
//! they cover what should show through. An X shape is a set of rectangles,
//! so clipping to them is exact, with no limit on how many (fluxland gave up
//! past 32). [`ShapeTracker`] follows each window's shape over a connection
//! of vaht's own to Xwayland; [`WindowShape`] is the shape as plain values,
//! which drawing ([`crate::scene::ShapedElement`]) and hit-testing use.

use std::collections::HashMap;
use std::os::fd::{AsFd as _, OwnedFd};

use smithay::backend::renderer::element::Id;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size};
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::shape::{ConnectionExt as _, SK};
use x11rb::rust_connection::RustConnection;

use crate::Compositor;
use crate::event_source::EventSource;
use crate::resources::ResourceLedger;

/// The part of a window that shows: rectangles relative to its top left
/// corner. Under each shape, the window's surfaces are new elements to the
/// renderer, so a change in shape redraws the window whole.
#[derive(Debug, Clone)]
pub(crate) struct WindowShape {
    /// The rectangles that show, relative to the window.
    rectangles: Vec<Rectangle<i32, Logical>>,
    /// What the renderer knows each of the window's surfaces by under this
    /// shape, by the id it has unshaped.
    ids: std::cell::RefCell<HashMap<Id, Id>>,
}

/// Each X11 window's shape, followed over vaht's own X connection.
#[derive(Debug)]
pub(crate) struct ShapeTracker {
    /// vaht's connection to Xwayland, for shapes alone.
    connection: RustConnection,
    /// Each shaped window's shape, by X window id; an unshaped window has
    /// none.
    shapes: HashMap<u32, WindowShape>,
    /// The connection's events, registered while the tracker lives.
    _source: EventSource<Compositor>,
}

impl WindowShape {
    /// The shape `rectangles` make of a window of `size`; none if they
    /// leave it whole (it is not shaped).
    pub(crate) fn new(
        rectangles: Vec<Rectangle<i32, Logical>>,
        size: Size<i32, Logical>,
    ) -> Option<Self> {
        let whole = Rectangle::from_size(size);
        let is_whole = rectangles
            .iter()
            .any(|rectangle| rectangle.contains_rect(whole));
        (!is_whole).then(|| Self {
            rectangles,
            ids: std::cell::RefCell::default(),
        })
    }

    /// Returns whether `point` (relative to the window) is in the shape.
    pub(crate) fn contains(&self, point: Point<f64, Logical>) -> bool {
        self.rectangles
            .iter()
            .any(|rectangle| rectangle.to_f64().contains(point))
    }

    /// Returns the shape at `scale`, relative to the window, as the
    /// renderer clips in.
    pub(crate) fn clip(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        self.rectangles
            .iter()
            .map(|rectangle| rectangle.to_physical_precise_round(scale))
            .collect()
    }

    /// Returns what the renderer knows the surface element `unshaped` by
    /// under this shape: an id of its own, the same for as long as the
    /// shape lasts.
    pub(crate) fn element_id(&self, unshaped: &Id) -> Id {
        self.ids
            .borrow_mut()
            .entry(unshaped.clone())
            .or_insert_with(Id::new)
            .clone()
    }

    /// Returns whether it is the shape `other` is.
    fn is_same_as(&self, other: &[Rectangle<i32, Logical>]) -> bool {
        self.rectangles == other
    }
}

impl ShapeTracker {
    /// Connects to Xwayland on `display` (such as `:1`) for shapes.
    ///
    /// # Errors
    ///
    /// Returns why not: the display cannot be reached, or it has no Shape
    /// extension.
    pub(crate) fn connect(
        display: &str,
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
    ) -> Result<Self, String> {
        let (connection, _) = x11rb::connect(Some(display))
            .map_err(|error| format!("cannot reach {display}: {error}"))?;
        connection
            .shape_query_version()
            .map_err(|error| error.to_string())?
            .reply()
            .map_err(|error| format!("{display} has no Shape extension: {error}"))?;
        let readable: OwnedFd = connection
            .stream()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|error| error.to_string())?;
        let source = EventSource::insert(
            handle,
            ledger,
            Generic::new(readable, Interest::READ, Mode::Level),
            |_, _, compositor| {
                compositor.xwayland_shapes_changed();
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| format!("cannot register the connection: {}", error.error))?;
        Ok(Self {
            connection,
            shapes: HashMap::new(),
            _source: source,
        })
    }

    /// Starts following the shape of the X window `window`, of `size`.
    pub(crate) fn track(&mut self, window: u32, size: Size<i32, Logical>) {
        let selected = self
            .connection
            .shape_select_input(window, true)
            .map(|_| ())
            .and_then(|()| self.connection.flush());
        if let Err(error) = selected {
            tracing::debug!(%error, window, "a window's shape is not followed");
            return;
        }
        self.refresh(window, size);
    }

    /// Stops following the shape of the X window `window`, which is gone.
    pub(crate) fn untrack(&mut self, window: u32) {
        self.shapes.remove(&window);
    }

    /// Returns the shape of the X window `window`, if it is shaped.
    pub(crate) fn shape_of(&self, window: u32) -> Option<&WindowShape> {
        self.shapes.get(&window)
    }

    /// Reads what the X server sent, and reads again the shape of each
    /// window whose shape changed; returns whether any did. `size_of`
    /// gives a window's size.
    pub(crate) fn follow_changes(
        &mut self,
        size_of: impl Fn(u32) -> Option<Size<i32, Logical>>,
    ) -> bool {
        let mut changed = Vec::new();
        loop {
            match self.connection.poll_for_event() {
                Ok(Some(Event::ShapeNotify(notify))) if notify.shape_kind == SK::BOUNDING => {
                    changed.push(notify.affected_window);
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(%error, "vaht's X connection for shapes failed");
                    break;
                }
            }
        }
        let mut is_any_changed = false;
        for window in changed {
            if let Some(size) = size_of(window) {
                is_any_changed |= self.refresh(window, size);
            }
        }
        is_any_changed
    }

    /// Reads the shape of the X window `window`, of `size`, again; returns
    /// whether it changed.
    fn refresh(&mut self, window: u32, size: Size<i32, Logical>) -> bool {
        let rectangles = match self
            .connection
            .shape_get_rectangles(window, SK::BOUNDING)
            .map_err(|error| error.to_string())
            .and_then(|cookie| cookie.reply().map_err(|error| error.to_string()))
        {
            Ok(reply) => reply
                .rectangles
                .iter()
                .map(|rectangle| {
                    Rectangle::new(
                        (i32::from(rectangle.x), i32::from(rectangle.y)).into(),
                        (i32::from(rectangle.width), i32::from(rectangle.height)).into(),
                    )
                })
                .collect::<Vec<_>>(),
            Err(error) => {
                tracing::debug!(error, window, "a window's shape could not be read");
                return false;
            }
        };
        let is_same = match self.shapes.get(&window) {
            Some(known) => known.is_same_as(&rectangles),
            None => WindowShape::new(rectangles.clone(), size).is_none(),
        };
        if is_same {
            return false;
        }
        match WindowShape::new(rectangles, size) {
            Some(shape) => {
                tracing::debug!(window, "window shaped");
                self.shapes.insert(window, shape);
            }
            None => {
                self.shapes.remove(&window);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rectangle.
    fn area(x: i32, y: i32, width: i32, height: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (width, height).into())
    }

    /// A 100×100 ring, 10 pixels wide, as four rectangles.
    fn ring() -> WindowShape {
        WindowShape::new(
            vec![
                area(0, 0, 100, 10),
                area(0, 90, 100, 10),
                area(0, 10, 10, 80),
                area(90, 10, 10, 80),
            ],
            (100, 100).into(),
        )
        .expect("a ring is shaped")
    }

    #[test]
    fn a_shape_covering_the_window_is_no_shape() {
        assert!(WindowShape::new(vec![area(0, 0, 100, 100)], (100, 100).into()).is_none());
        assert!(WindowShape::new(vec![area(-5, -5, 200, 200)], (100, 100).into()).is_none());
    }

    #[test]
    fn a_ring_holds_its_band_but_not_its_middle() {
        let ring = ring();
        assert!(ring.contains((5.0, 50.0).into()));
        assert!(ring.contains((95.0, 95.0).into()));
        assert!(
            !ring.contains((50.0, 50.0).into()),
            "the middle shows through"
        );
        assert!(!ring.contains((150.0, 5.0).into()));
    }

    #[test]
    fn a_shape_clips_at_the_outputs_scale() {
        let clip = ring().clip(Scale::from(2.0));
        assert_eq!(clip[0], Rectangle::new((0, 0).into(), (200, 20).into()));
        assert_eq!(clip.len(), 4);
    }

    #[test]
    fn each_shape_makes_new_elements_that_last_as_long_as_it() {
        let surface = Id::new();
        let (first, second) = (ring(), ring());
        assert_eq!(first.element_id(&surface), first.element_id(&surface));
        assert_ne!(first.element_id(&surface), second.element_id(&surface));
    }
}
