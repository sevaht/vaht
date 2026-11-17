//! Fluxbox's texture renderer (`FbTk::TextureRender`), so gradients and
//! bevels look as they did in Fluxbox, to the pixel.
//!
//! A gradient is a table of colors stepped from the first color toward the
//! second, one entry per row or column, the last entry one step short of
//! the second color; a diagonal one adds two tables at half strength. A
//! bevel then lightens or darkens the edge pixels of what was drawn
//! ([`lighter`], [`darker`]): raised lights the top and left. Sunken draws
//! the gradient with its colors swapped, bevels it raised, and turns the
//! result half a turn, which is what puts its light edges at the bottom and
//! right.

use super::{Bevel, GradientDirection};
use crate::color::Color;

/// A channel made lighter, as Fluxbox lights a bevel's bright edge: an
/// eighth of the way to white.
pub(crate) const fn lighter(value: u8) -> u8 {
    value + ((255 - value) >> 3)
}

/// A channel made darker, as Fluxbox shades a bevel's dark edge: three
/// quarters of it.
pub(crate) const fn darker(value: u8) -> u8 {
    // At most 255 * 3 / 4, which fits.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "three quarters of a byte is a byte"
    )]
    let shaded = (value as u16 * 3 / 4) as u8;
    shaded
}

/// Returns `color` made lighter ([`lighter`]), its alpha kept.
pub(crate) const fn lightened(color: Color) -> Color {
    Color::rgba(
        lighter(color.red),
        lighter(color.green),
        lighter(color.blue),
        color.alpha,
    )
}

/// Returns `color` made darker ([`darker`]), its alpha kept.
pub(crate) const fn darkened(color: Color) -> Color {
    Color::rgba(
        darker(color.red),
        darker(color.green),
        darker(color.blue),
        color.alpha,
    )
}

/// A gradient `width` × `height`, from `from` to `to` along `direction`,
/// bevelled as `bevel` says: its colors row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GradientImage {
    /// How wide it is.
    pub(crate) width: u32,
    /// How tall it is.
    pub(crate) height: u32,
    /// Its colors, row by row.
    pub(crate) pixels: Vec<Color>,
}

impl GradientImage {
    /// Renders the gradient from `from` to `to` along `direction`,
    /// `width` × `height`, with `bevel`.
    pub(crate) fn render(
        direction: GradientDirection,
        (from, to): (Color, Color),
        bevel: Bevel,
        (width, height): (u32, u32),
    ) -> Self {
        let is_sunken = bevel == Bevel::Sunken;
        let (first, second) = if is_sunken { (to, from) } else { (from, to) };
        let mut image = Self::gradient(direction, first, second, width, height);
        if bevel != Bevel::Flat {
            image.bevel_raised();
        }
        if is_sunken {
            image.pixels.reverse();
        }
        image
    }

    /// Returns the color at column `x`, row `y`.
    #[cfg(test)]
    pub(crate) fn at(&self, x: u32, y: u32) -> Color {
        self.pixels[self.index(x, y)]
    }

    /// Returns where column `x`, row `y` is in `pixels`.
    fn index(&self, x: u32, y: u32) -> usize {
        usize::try_from(y * self.width + x).expect("image sizes fit in usize")
    }

    /// Renders the gradient, unbevelled.
    fn gradient(
        direction: GradientDirection,
        from: Color,
        to: Color,
        width: u32,
        height: u32,
    ) -> Self {
        let columns = |scale| table(width, from, to, scale);
        let rows = |scale| table(height, from, to, scale);
        let pixel: Box<dyn Fn(usize, usize) -> Color> = match direction {
            GradientDirection::Horizontal => {
                let columns = columns(1.0);
                Box::new(move |x, _| columns[x])
            }
            GradientDirection::Vertical => {
                let rows = rows(1.0);
                Box::new(move |_, y| rows[y])
            }
            GradientDirection::Diagonal => {
                let (columns, rows) = (columns(0.5), rows(0.5));
                Box::new(move |x, y| sum(columns[x], rows[y]))
            }
            GradientDirection::CrossDiagonal => {
                // The columns run the other way: the corner that starts is
                // the top right.
                let columns = table(width, to, from, 0.5);
                let rows = rows(0.5);
                Box::new(move |x, y| sum(columns[x], rows[y]))
            }
        };
        let to_index = |value: u32| usize::try_from(value).expect("image sizes fit in usize");
        let pixels = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| pixel(to_index(x), to_index(y)))
            .collect();
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Lightens the top and left edges and darkens the bottom and right,
    /// as Fluxbox's `bevel1` does, the bottom right corner and the pixel
    /// above it darkened twice.
    fn bevel_raised(&mut self) {
        let (width, height) = (self.width, self.height);
        if width <= 2 || height <= 2 {
            return;
        }
        let mut change = |x: u32, y: u32, shade: fn(Color) -> Color| {
            let index = self.index(x, y);
            self.pixels[index] = shade(self.pixels[index]);
        };
        for x in 0..width {
            change(x, 0, lightened);
        }
        for y in 1..height - 1 {
            change(0, y, lightened);
            change(width - 1, y, darkened);
        }
        for x in 1..width {
            change(x, height - 1, darkened);
        }
        change(width - 1, height - 1, darkened);
        change(width - 1, height - 2, darkened);
    }
}

/// Returns `count` colors stepped from `from` toward `to`, each channel
/// times `scale` and cut to a whole number, as Fluxbox's tables are: the
/// last is one step short of `to`.
fn table(count: u32, from: Color, to: Color, scale: f64) -> Vec<Color> {
    let channel = |start: u8, end: u8, index: u32| {
        let step = (f64::from(end) - f64::from(start)) / f64::from(count);
        let value = scale * f64::from(index).mul_add(step, f64::from(start));
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a blend of two channels, cut as Fluxbox cuts it, is a channel"
        )]
        let channel = value as u8;
        channel
    };
    (0..count)
        .map(|index| {
            Color::rgba(
                channel(from.red, to.red, index),
                channel(from.green, to.green, index),
                channel(from.blue, to.blue, index),
                channel(from.alpha, to.alpha, index),
            )
        })
        .collect()
}

/// Returns the sum of two half-strength colors, channel by channel.
const fn sum(first: Color, second: Color) -> Color {
    Color::rgba(
        first.red.wrapping_add(second.red),
        first.green.wrapping_add(second.green),
        first.blue.wrapping_add(second.blue),
        first.alpha.wrapping_add(second.alpha),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// X11's `greyN`: N percent of white.
    fn grey(percent: u8) -> Color {
        let value = u8::try_from((u16::from(percent) * 255 + 50) / 100).expect("a channel");
        Color::rgb(value, value, value)
    }

    /// A taskbar entry's texture, 160 × 20, as the user's style has it.
    fn entry(from: Color, to: Color, bevel: Bevel) -> GradientImage {
        GradientImage::render(GradientDirection::Vertical, (from, to), bevel, (160, 20))
    }

    /// The red channel down column `x`.
    fn column(image: &GradientImage, x: u32) -> Vec<u8> {
        (0..image.height).map(|y| image.at(x, y).red).collect()
    }

    #[test]
    fn a_raised_gradient_matches_fluxbox_to_the_pixel() {
        // Read from fluxland's taskbar: the focused entry, grey55 to grey5,
        // raised. Its top row is grey55 lightened, its bottom row the last
        // step darkened.
        let image = entry(grey(55), grey(5), Bevel::Raised);
        let middle = column(&image, 80);
        assert_eq!(middle[0], 154, "lightened");
        assert_eq!(&middle[1..3], [133, 127]);
        assert_eq!(middle[19], 14, "darkened");
        assert_eq!(
            image.at(0, 10).red,
            lighter(image.at(1, 10).red),
            "left lit"
        );
    }

    #[test]
    fn a_sunken_gradient_matches_fluxbox_to_the_pixel() {
        // The unfocused entry, grey30 to grey5, sunken: darkened at the top,
        // lightened at the bottom.
        let image = entry(grey(30), grey(5), Bevel::Sunken);
        let middle = column(&image, 80);
        assert_eq!(middle[0], 54, "darkened");
        assert_eq!(middle[19], 43, "lightened");
        assert!(middle[1] > middle[18], "still lighter at the top");
    }

    #[test]
    fn a_flat_gradient_steps_one_short_of_its_end() {
        let image = entry(Color::rgb(200, 0, 0), Color::rgb(0, 0, 0), Bevel::Flat);
        let middle = column(&image, 0);
        assert_eq!(middle[0], 200);
        assert_eq!(middle[19], 10, "200 - 19 * 10");
    }

    #[test]
    fn a_diagonal_runs_from_the_top_left_and_a_cross_diagonal_from_the_top_right() {
        let (white, black) = (Color::rgb(255, 255, 255), Color::rgb(0, 0, 0));
        let render =
            |direction| GradientImage::render(direction, (white, black), Bevel::Flat, (10, 10));
        let diagonal = render(GradientDirection::Diagonal);
        assert!(diagonal.at(0, 0).red > diagonal.at(9, 9).red);
        let cross = render(GradientDirection::CrossDiagonal);
        assert!(cross.at(9, 0).red > cross.at(0, 9).red);
    }

    #[test]
    fn bevel_shading_is_fluxbox_s() {
        assert_eq!(lighter(0), 31);
        assert_eq!(lighter(255), 255);
        assert_eq!(darker(255), 191);
        assert_eq!(darker(0), 0);
    }
}
