//! Rounded corners: how round each corner of a rectangle is, and drawing
//! with them.
//!
//! [`CornerRadii`] says how round each [`Corner`] is; a theme says it as a
//! radius and a [`CornerSet`] of the corners it rounds. A [`Panel`] cuts its
//! corners away by them (anti-aliased), and hit-testing asks
//! [`CornerRadii::cuts`] whether a pixel was cut. A line drawn around a
//! rounded rectangle is straight along its sides and a [`CornerArc`] at
//! each rounded corner: the arc is painted on its own small square, so the
//! straight parts stay plain colored rectangles.
//!
//! [`Panel`]: crate::Panel

use tiny_skia::{BlendMode, FillRule, Mask, Paint, PathBuilder, Pixmap, Transform};

use crate::color::Color;
use crate::geometry::{Point, Size};

/// One corner of a rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumIter)]
pub enum Corner {
    /// The top-left corner.
    TopLeft,
    /// The top-right corner.
    TopRight,
    /// The bottom-left corner.
    BottomLeft,
    /// The bottom-right corner.
    BottomRight,
}

impl Corner {
    /// Returns the corner's name in theme files.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::TopLeft => "top-left",
            Self::TopRight => "top-right",
            Self::BottomLeft => "bottom-left",
            Self::BottomRight => "bottom-right",
        }
    }

    /// Returns the corner named `name` in theme files.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        <Self as strum::IntoEnumIterator>::iter().find(|corner| corner.name() == name)
    }

    /// Returns whether it is one of the top corners.
    #[must_use]
    pub const fn is_top(self) -> bool {
        matches!(self, Self::TopLeft | Self::TopRight)
    }

    /// Returns whether it is one of the left corners.
    #[must_use]
    pub const fn is_left(self) -> bool {
        matches!(self, Self::TopLeft | Self::BottomLeft)
    }

    /// Returns the center of the circle a corner `radius` round follows,
    /// on a square `radius` wide at that corner.
    fn arc_center(self, radius: f32) -> (f32, f32) {
        (
            if self.is_left() { radius } else { 0.0 },
            if self.is_top() { radius } else { 0.0 },
        )
    }
}

/// Some of a rectangle's corners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CornerSet {
    /// Whether each corner is in the set, in [`Corner`]'s order.
    members: [bool; 4],
}

impl CornerSet {
    /// Every corner.
    pub const ALL: Self = Self { members: [true; 4] };

    /// Returns whether `corner` is in the set.
    #[must_use]
    pub const fn contains(self, corner: Corner) -> bool {
        self.members[corner as usize]
    }

    /// Returns the corners in the set.
    pub fn iter(self) -> impl Iterator<Item = Corner> {
        <Corner as strum::IntoEnumIterator>::iter().filter(move |corner| self.contains(*corner))
    }
}

impl Default for CornerSet {
    /// Every corner: a radius alone rounds them all.
    fn default() -> Self {
        Self::ALL
    }
}

impl FromIterator<Corner> for CornerSet {
    fn from_iter<Corners: IntoIterator<Item = Corner>>(iter: Corners) -> Self {
        let mut set = Self {
            members: [false; 4],
        };
        for corner in iter {
            set.members[corner as usize] = true;
        }
        set
    }
}

/// How round each corner of a rectangle is, in pixels; 0 is square.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CornerRadii {
    /// Each corner's radius, in [`Corner`]'s order.
    radii: [u32; 4],
}

impl CornerRadii {
    /// Every corner square.
    pub const SQUARE: Self = Self { radii: [0; 4] };

    /// How far along a quarter circle's ends its cubic Bézier's control
    /// points sit, as a fraction of the radius: 4/3 · (√2 − 1), the closest
    /// a cubic comes to a circle.
    const KAPPA: f32 = 0.552_284_8;

    /// Returns `corners` rounded by `radius`, the others square.
    #[must_use]
    pub fn of(radius: u32, corners: impl IntoIterator<Item = Corner>) -> Self {
        corners
            .into_iter()
            .fold(Self::SQUARE, |radii, corner| radii.with(corner, radius))
    }

    /// Returns how round `corner` is.
    #[must_use]
    pub const fn radius(self, corner: Corner) -> u32 {
        self.radii[corner as usize]
    }

    /// Returns whether every corner is square.
    #[must_use]
    pub fn is_square(self) -> bool {
        self.radii.iter().all(|radius| *radius == 0)
    }

    /// Returns whether the pixel at `point` of a rectangle of `size` lies
    /// outside its rounded corners, and so was cut away: whether the
    /// pixel's center is further from a corner's circle than its radius.
    #[must_use]
    pub fn cuts(self, size: Size, point: Point) -> bool {
        <Corner as strum::IntoEnumIterator>::iter().any(|corner| {
            let radius = self.radius(corner);
            if radius == 0 {
                return false;
            }
            let radius = f64::from(radius);
            let (width, height) = (f64::from(size.width), f64::from(size.height));
            let (x, y) = (f64::from(point.x) + 0.5, f64::from(point.y) + 0.5);
            let center_x = if corner.is_left() {
                radius
            } else {
                width - radius
            };
            let center_y = if corner.is_top() {
                radius
            } else {
                height - radius
            };
            let is_in_square = if corner.is_left() {
                x < radius
            } else {
                x > center_x
            } && if corner.is_top() {
                y < radius
            } else {
                y > center_y
            };
            is_in_square && (x - center_x).hypot(y - center_y) > radius
        })
    }

    /// Returns these radii with `corner` rounded by `radius`.
    #[must_use]
    pub const fn with(mut self, corner: Corner, radius: u32) -> Self {
        self.radii[corner as usize] = radius;
        self
    }

    /// Returns each radius less `width`, as the corners just inside a line
    /// that wide around the rectangle are round.
    #[must_use]
    pub fn inset(self, width: u32) -> Self {
        Self {
            radii: self.radii.map(|radius| radius.saturating_sub(width)),
        }
    }

    /// Clears what lies outside these corners of `pixmap`, anti-aliased.
    pub(crate) fn cut(self, pixmap: &mut Pixmap) {
        if self.is_square() {
            return;
        }
        let Some(mask) = self.mask(pixmap.width(), pixmap.height()) else {
            return;
        };
        pixmap.apply_mask(&mask);
    }

    /// Returns a mask covering a rectangle `width` by `height` with these
    /// corners, if it has an area.
    fn mask(self, width: u32, height: u32) -> Option<Mask> {
        let mut mask = Mask::new(width, height)?;
        let (width, height) = (pixels(width), pixels(height));
        let radius = |corner| {
            pixels(self.radius(corner))
                .min(width / 2.0)
                .min(height / 2.0)
        };
        let mut path = PathBuilder::new();
        let top_left = radius(Corner::TopLeft);
        let top_right = radius(Corner::TopRight);
        let bottom_right = radius(Corner::BottomRight);
        let bottom_left = radius(Corner::BottomLeft);
        path.move_to(top_left, 0.0);
        path.line_to(width - top_right, 0.0);
        path.cubic_to(
            width - top_right * (1.0 - Self::KAPPA),
            0.0,
            width,
            top_right * (1.0 - Self::KAPPA),
            width,
            top_right,
        );
        path.line_to(width, height - bottom_right);
        path.cubic_to(
            width,
            height - bottom_right * (1.0 - Self::KAPPA),
            width - bottom_right * (1.0 - Self::KAPPA),
            height,
            width - bottom_right,
            height,
        );
        path.line_to(bottom_left, height);
        path.cubic_to(
            bottom_left * (1.0 - Self::KAPPA),
            height,
            0.0,
            height - bottom_left * (1.0 - Self::KAPPA),
            0.0,
            height - bottom_left,
        );
        path.line_to(0.0, top_left);
        path.cubic_to(
            0.0,
            top_left * (1.0 - Self::KAPPA),
            top_left * (1.0 - Self::KAPPA),
            0.0,
            top_left,
            0.0,
        );
        path.close();
        mask.fill_path(
            &path.finish()?,
            FillRule::Winding,
            true,
            Transform::identity(),
        );
        Some(mask)
    }
}

/// The part of a line around a rounded rectangle at one corner: an arc
/// `line_width` thick along the corner's circle, on a square as wide as
/// the radius. Outside the arc the square is clear, so what lies there
/// (cut away) and inside it (the rectangle's own corner) both show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CornerArc {
    /// The corner.
    pub corner: Corner,
    /// How round it is.
    pub radius: u32,
    /// How thick the line is.
    pub line_width: u32,
    /// The line's color.
    pub color: Color,
}

impl CornerArc {
    /// Paints the arc on its square; none for a square corner.
    #[must_use]
    pub fn paint(&self) -> Option<Pixmap> {
        let mut pixmap = Pixmap::new(self.radius, self.radius)?;
        let radius = pixels(self.radius);
        let (center_x, center_y) = self.corner.arc_center(radius);
        let outer = PathBuilder::from_circle(center_x, center_y, radius)?;
        let mut paint = Paint::default();
        paint.set_color(self.color.to_skia());
        paint.anti_alias = true;
        pixmap.fill_path(
            &outer,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let inner_radius = pixels(self.radius.saturating_sub(self.line_width));
        if let Some(inner) = PathBuilder::from_circle(center_x, center_y, inner_radius) {
            paint.blend_mode = BlendMode::Clear;
            pixmap.fill_path(
                &inner,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        Some(pixmap)
    }
}

/// Converts a pixel count to drawing coordinates.
fn pixels(count: u32) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "panel extents are far below 2^24, where f32 stays exact"
    )]
    let exact = count as f32;
    exact
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 40 by 30 rectangle.
    const SIZE: Size = Size::new(40, 30);

    #[test]
    fn only_rounded_corners_cut_pixels_outside_their_circle() {
        let radii = CornerRadii::of(8, [Corner::TopLeft, Corner::BottomRight]);
        assert!(radii.cuts(SIZE, Point::new(0, 0)), "the very corner");
        assert!(radii.cuts(SIZE, Point::new(1, 1)));
        assert!(
            !radii.cuts(SIZE, Point::new(3, 3)),
            "(3.5, 3.5) is 6.4 from (8, 8): inside"
        );
        assert!(
            !radii.cuts(SIZE, Point::new(8, 0)),
            "past the corner's square"
        );
        assert!(radii.cuts(SIZE, Point::new(39, 29)), "bottom right");
        assert!(!radii.cuts(SIZE, Point::new(39, 0)), "top right is square");
        assert!(
            !radii.cuts(SIZE, Point::new(0, 29)),
            "bottom left is square"
        );
    }

    #[test]
    fn cutting_clears_outside_the_corners_and_keeps_inside() {
        let mut pixmap = Pixmap::new(40, 30).expect("an area");
        pixmap.fill(Color::rgb(255, 0, 0).to_skia());
        CornerRadii::of(8, [Corner::TopRight]).cut(&mut pixmap);
        let alpha = |x, y| pixmap.pixel(x, y).expect("in the pixmap").alpha();
        assert_eq!(alpha(39, 0), 0, "cut away");
        assert_eq!(alpha(34, 5), 255, "inside the circle");
        assert_eq!(alpha(0, 0), 255, "other corners stay");
    }

    #[test]
    fn an_arc_is_drawn_along_the_circle_and_clear_off_it() {
        let arc = CornerArc {
            corner: Corner::TopLeft,
            radius: 8,
            line_width: 1,
            color: Color::rgb(0, 0, 255),
        };
        let pixmap = arc.paint().expect("a round corner");
        let alpha = |x, y| pixmap.pixel(x, y).expect("in the square").alpha();
        assert_eq!(alpha(0, 0), 0, "outside the circle");
        assert_eq!(alpha(7, 7), 0, "inside the line: the rectangle shows");
        assert!(alpha(0, 7) > 128, "on the line at the left edge");
        assert!(alpha(7, 0) > 128, "on the line at the top edge");
    }

    #[test]
    fn insetting_shrinks_every_radius_and_stops_at_square() {
        let radii = CornerRadii::of(8, [Corner::TopLeft]).with(Corner::BottomLeft, 1);
        assert_eq!(radii.inset(2), CornerRadii::of(6, [Corner::TopLeft]));
    }

    #[test]
    fn a_set_holds_the_corners_it_was_made_of() {
        let set: CornerSet = [Corner::TopRight, Corner::BottomLeft].into_iter().collect();
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            [Corner::TopRight, Corner::BottomLeft]
        );
        assert_eq!(CornerSet::default().iter().count(), 4);
    }

    #[test]
    fn corners_are_named_as_theme_files_write_them() {
        assert_eq!(Corner::named("bottom-right"), Some(Corner::BottomRight));
        assert_eq!(Corner::named("bottomright"), None);
    }
}
