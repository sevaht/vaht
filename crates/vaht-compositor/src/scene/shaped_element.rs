//! A window's surface clipped to its shape (an X11 window's X Shape).
//!
//! [`ShapedElement`] wraps the element that draws a surface and lets it
//! draw only inside the shape's rectangles: the damage it is asked to draw
//! is cut to them, and so are the regions it says are opaque, so what is
//! below shows through the holes. It is exact for any number of
//! rectangles. It offers no buffer for direct scan-out, since a plane
//! would show the surface whole.

use smithay::backend::renderer::Renderer;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Transform};

/// An element drawn only inside a shape.
#[derive(Debug)]
pub(crate) struct ShapedElement<E> {
    /// What draws the surface.
    inner: E,
    /// The shape's rectangles, relative to the element.
    clip: Vec<Rectangle<i32, Physical>>,
    /// What the renderer knows this element by: the shape's, so a new
    /// shape redraws it whole.
    id: Id,
}

impl<E: Element> ShapedElement<E> {
    /// `inner`, drawn only inside `clip` (relative to the window, whose
    /// top left corner is at `window_corner`), known by `id`.
    pub(crate) fn new(
        inner: E,
        clip: &[Rectangle<i32, Physical>],
        window_corner: Point<i32, Physical>,
        (id, scale): (Id, Scale<f64>),
    ) -> Self {
        // The clip is the window's; the element may be one of its
        // subsurfaces, elsewhere in it.
        let offset = inner.geometry(scale).loc - window_corner;
        let clip = clip
            .iter()
            .map(|rectangle| Rectangle::new(rectangle.loc - offset, rectangle.size))
            .collect();
        Self { inner, clip, id }
    }

    /// Returns the parts of `regions` (relative to the element) inside the
    /// shape.
    fn within<'regions>(
        &'regions self,
        regions: impl IntoIterator<Item = &'regions Rectangle<i32, Physical>>,
    ) -> impl Iterator<Item = Rectangle<i32, Physical>> {
        regions.into_iter().flat_map(|region| {
            self.clip
                .iter()
                .filter_map(|rectangle| rectangle.intersection(*region))
        })
    }
}

impl<E: Element> Element for ShapedElement<E> {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        let damage = self.inner.damage_since(scale, commit);
        self.within(damage.iter()).collect()
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        let opaque = self.inner.opaque_regions(scale);
        self.within(opaque.iter()).collect()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<R: Renderer, E: RenderElement<R>> RenderElement<R> for ShapedElement<E> {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&smithay::utils::user_data::UserDataMap>,
    ) -> Result<(), R::Error> {
        let damage: Vec<_> = self.within(damage).collect();
        if damage.is_empty() {
            return Ok(());
        }
        let opaque: Vec<_> = self.within(opaque_regions).collect();
        self.inner.draw(frame, src, dst, &damage, &opaque, cache)
    }

    fn underlying_storage(&self, _renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        // A plane would show the surface whole, holes and all.
        None
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::renderer::element::solid::SolidColorRenderElement;

    use super::*;

    /// A ring 100 pixels square, 10 wide, as four rectangles.
    fn ring() -> Vec<Rectangle<i32, Physical>> {
        vec![
            Rectangle::new((0, 0).into(), (100, 10).into()),
            Rectangle::new((0, 90).into(), (100, 10).into()),
            Rectangle::new((0, 10).into(), (10, 80).into()),
            Rectangle::new((90, 10).into(), (10, 80).into()),
        ]
    }

    /// A 100×100 solid element at `corner`.
    fn solid(corner: (i32, i32)) -> SolidColorRenderElement {
        SolidColorRenderElement::new(
            Id::new(),
            Rectangle::new(corner.into(), (100, 100).into()),
            CommitCounter::default(),
            [1.0, 0.0, 0.0, 1.0],
            Kind::Unspecified,
        )
    }

    #[test]
    fn damage_and_opaque_regions_stay_inside_the_shape() {
        let shaped = ShapedElement::new(
            solid((50, 50)),
            &ring(),
            (50, 50).into(),
            (Id::new(), Scale::from(1.0)),
        );
        let damage = shaped.damage_since(Scale::from(1.0), None);
        let area: i32 = damage
            .iter()
            .map(|rectangle| rectangle.size.w * rectangle.size.h)
            .sum();
        assert_eq!(
            area,
            100 * 100 - 80 * 80,
            "the ring's area, not the square's"
        );
        let opaque = shaped.opaque_regions(Scale::from(1.0));
        assert!(
            opaque
                .iter()
                .all(|rectangle| ring().iter().any(|band| band.contains_rect(*rectangle)))
        );
    }

    #[test]
    fn a_subsurface_is_clipped_where_it_is_in_the_window() {
        // An element 20 pixels right of the window's corner: the window's
        // left band is out of it.
        let shaped = ShapedElement::new(
            solid((70, 50)),
            &ring(),
            (50, 50).into(),
            (Id::new(), Scale::from(1.0)),
        );
        let damage = shaped.damage_since(Scale::from(1.0), None);
        assert!(
            damage.iter().all(|rectangle| rectangle.loc.x >= 0),
            "nothing left of the element"
        );
    }
}
