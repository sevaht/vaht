//! What a widget's background looks like: a fill and a bevel.
//!
//! These are Fluxbox's textures without the X11 baggage: a solid color or a
//! two-color gradient, optionally bevelled to look raised or sunken, or
//! nothing at all, so what is behind shows (Fluxbox's `ParentRelative`).

use crate::color::Color;

/// A widget's background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Texture {
    /// What fills the area.
    pub fill: Fill,
    /// The edge shading that makes it look raised or sunken.
    pub bevel: Bevel,
}

impl Texture {
    /// Returns a flat area of one color.
    #[must_use]
    pub const fn solid(color: Color) -> Self {
        Self {
            fill: Fill::Solid(color),
            bevel: Bevel::Flat,
        }
    }

    /// Returns a flat gradient from `from` to `to` in `direction`.
    #[must_use]
    pub const fn gradient(direction: GradientDirection, from: Color, to: Color) -> Self {
        Self {
            fill: Fill::Gradient {
                direction,
                from,
                to,
            },
            bevel: Bevel::Flat,
        }
    }

    /// Returns no texture: what is behind the widget shows.
    #[must_use]
    pub const fn parent() -> Self {
        Self {
            fill: Fill::Parent,
            bevel: Bevel::Flat,
        }
    }

    /// Returns the texture with `bevel`.
    #[must_use]
    pub const fn with_bevel(mut self, bevel: Bevel) -> Self {
        self.bevel = bevel;
        self
    }
}

/// What fills an area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fill {
    /// One color.
    Solid(Color),
    /// A blend between two colors.
    Gradient {
        /// Which way the blend runs.
        direction: GradientDirection,
        /// The starting color.
        from: Color,
        /// The ending color.
        to: Color,
    },
    /// Nothing: what is behind shows.
    Parent,
}

/// Which way a gradient runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GradientDirection {
    /// Top to bottom.
    Vertical,
    /// Left to right.
    Horizontal,
    /// Top-left to bottom-right.
    Diagonal,
    /// Top-right to bottom-left.
    CrossDiagonal,
}

/// Edge shading that makes an area look raised, sunken, or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Bevel {
    /// No shading.
    #[default]
    Flat,
    /// Lit from the top left: light top and left edges, dark bottom and right.
    Raised,
    /// The reverse: the area looks pressed in.
    Sunken,
}
