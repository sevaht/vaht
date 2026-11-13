//! `rules.toml`: what happens to particular windows when they open (02 §7).
//!
//! ```toml
//! [[rule]]
//! match = { app_id = "vaht-dialog" }
//! position = { anchor = "center" }
//!
//! [[rule]]
//! match = { app_id = "steam" }
//! workspace = 2
//! ```
//!
//! Rules apply in file order when a window first gets its place; a later
//! matching rule overrides an earlier one key by key. Rule keys for features
//! vaht does not have yet are refused, so a rule never silently does nothing.

use std::num::NonZeroUsize;

use smithay::utils::{Logical, Point, Rectangle, Size};

use super::matcher::WindowMatcher;
use crate::window_manager::{Axes, Placement, StackLayer, TileCell, Window, Workspace};

/// Every rule, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rules {
    /// The rules.
    #[serde(default)]
    rule: Vec<Rule>,
}

impl Rules {
    /// Returns what the rules matching `window` say, while `current` is
    /// shown; later rules override earlier ones key by key.
    pub(crate) fn for_window(&self, window: &Window, current: Workspace) -> RuleSettings {
        self.rule
            .iter()
            .filter(|rule| rule.matcher.matches(window, current))
            .fold(RuleSettings::default(), |settings, rule| {
                settings.overridden_by(&rule.settings())
            })
    }
}

/// One rule: which windows, and what for them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    /// The windows it is for.
    #[serde(rename = "match")]
    matcher: WindowMatcher,
    /// The workspace they open on, from 1.
    workspace: Option<NonZeroUsize>,
    /// Whether they show on every workspace.
    sticky: Option<bool>,
    /// Whether they open minimized.
    minimized: Option<bool>,
    /// How they are placed: `maximized`, `maximized horizontal`,
    /// `maximized vertical`, `fullscreen`, or `tile <x> <y> <width>
    /// <height>`.
    placement: Option<RulePlacement>,
    /// Where their frame goes.
    position: Option<Position>,
    /// Their frame's size, in pixels.
    size: Option<[u16; 2]>,
    /// Whether what shows through them, where they are translucent, is
    /// only the wallpaper, not the windows below.
    pseudo_transparent: Option<bool>,
    /// The stacking layer they open in.
    #[serde(default, deserialize_with = "deserialize_layer")]
    layer: Option<StackLayer>,
}

/// Reads a layer's name.
fn deserialize_layer<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<StackLayer>, D::Error> {
    let name: String = serde::Deserialize::deserialize(deserializer)?;
    name.parse().map(Some).map_err(serde::de::Error::custom)
}

impl Rule {
    /// Returns the rule's settings.
    const fn settings(&self) -> RuleSettings {
        RuleSettings {
            workspace: self.workspace,
            sticky: self.sticky,
            minimized: self.minimized,
            placement: self.placement,
            position: self.position,
            size: self.size,
            pseudo_transparent: self.pseudo_transparent,
            layer: self.layer,
        }
    }
}

/// What rules say about a window; `None` where they say nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RuleSettings {
    /// The workspace it opens on, from 1.
    pub(crate) workspace: Option<NonZeroUsize>,
    /// Whether it shows on every workspace.
    pub(crate) sticky: Option<bool>,
    /// Whether it opens minimized.
    pub(crate) minimized: Option<bool>,
    /// How it is placed.
    pub(crate) placement: Option<RulePlacement>,
    /// Where its frame goes.
    pub(crate) position: Option<Position>,
    /// Its frame's size.
    size: Option<[u16; 2]>,
    /// Whether only the wallpaper shows through it.
    pub(crate) pseudo_transparent: Option<bool>,
    /// The stacking layer it opens in.
    pub(crate) layer: Option<StackLayer>,
}

impl RuleSettings {
    /// Returns the frame size the rules set, if any.
    pub(crate) fn size(&self) -> Option<Size<i32, Logical>> {
        self.size
            .map(|[width, height]| Size::from((i32::from(width).max(1), i32::from(height).max(1))))
    }

    /// Returns these settings with `later`'s keys replacing theirs.
    #[must_use]
    fn overridden_by(self, later: &Self) -> Self {
        Self {
            workspace: later.workspace.or(self.workspace),
            sticky: later.sticky.or(self.sticky),
            minimized: later.minimized.or(self.minimized),
            placement: later.placement.or(self.placement),
            position: later.position.or(self.position),
            size: later.size.or(self.size),
            pseudo_transparent: later.pseudo_transparent.or(self.pseudo_transparent),
            layer: later.layer.or(self.layer),
        }
    }
}

/// How a rule places a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RulePlacement {
    /// Maximized along the axes.
    Maximized(Axes),
    /// Fullscreen.
    Fullscreen,
    /// On a cell of the usable area.
    Tiled(TileCell),
}

impl RulePlacement {
    /// Returns the placement for a window floating at `current`.
    pub(crate) fn placement(self, current: Rectangle<i32, Logical>) -> Placement {
        match self {
            Self::Maximized(axes) => Placement::Floating.with_maximized_toggled(axes, current),
            Self::Fullscreen => Placement::Floating.with_fullscreen_toggled(current),
            Self::Tiled(cell) => Placement::Tiled {
                cell,
                restore: current,
            },
        }
    }

    /// Parses the string form.
    fn parse(text: &str) -> Result<Self, String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        match words.as_slice() {
            ["maximized"] => Ok(Self::Maximized(Axes::Both)),
            ["maximized", "horizontal"] => Ok(Self::Maximized(Axes::Horizontal)),
            ["maximized", "vertical"] => Ok(Self::Maximized(Axes::Vertical)),
            ["fullscreen"] => Ok(Self::Fullscreen),
            ["tile", x, y, width, height] => {
                let number = |text: &str| text.parse::<u16>().ok();
                number(x)
                    .zip(number(y))
                    .zip(number(width).zip(number(height)))
                    .and_then(|((x, y), (width, height))| TileCell::new(x, y, width, height))
                    .map(Self::Tiled)
                    .ok_or_else(|| "a tile cell is four thousandths within 0 to 1000".to_owned())
            }
            _ => Err(format!(
                "placement is maximized [horizontal|vertical], fullscreen, or tile <x> <y> \
                 <width> <height>, not `{text}`"
            )),
        }
    }
}

impl<'de> serde::Deserialize<'de> for RulePlacement {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Where a rule puts a window's frame: at an anchor of the usable area,
/// moved in from it by an offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Position {
    /// The point of the area the frame is put against.
    anchor: Anchor,
    /// How far in from the anchor, in pixels: right and down from a left or
    /// top anchor, left and up from a right or bottom one.
    #[serde(default)]
    offset: [i32; 2],
}

impl Position {
    /// Returns where the corner of a frame of `size` goes in `usable`.
    pub(crate) fn corner(
        self,
        usable: Rectangle<i32, Logical>,
        size: Size<i32, Logical>,
    ) -> Point<i32, Logical> {
        let [offset_x, offset_y] = self.offset;
        let (horizontal, vertical) = self.anchor.sides();
        let along = |side: Side, start: i32, span: i32, length: i32, offset: i32| match side {
            Side::Start => start + offset,
            Side::Middle => start + (span - length) / 2 + offset,
            Side::End => start + span - length - offset,
        };
        Point::from((
            along(horizontal, usable.loc.x, usable.size.w, size.w, offset_x),
            along(vertical, usable.loc.y, usable.size.h, size.h, offset_y),
        ))
    }
}

/// A point of an area a frame is put against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Anchor {
    /// The top-left corner.
    TopLeft,
    /// The middle of the top edge.
    Top,
    /// The top-right corner.
    TopRight,
    /// The middle of the left edge.
    Left,
    /// The middle.
    Center,
    /// The middle of the right edge.
    Right,
    /// The bottom-left corner.
    BottomLeft,
    /// The middle of the bottom edge.
    Bottom,
    /// The bottom-right corner.
    BottomRight,
}

impl Anchor {
    /// Returns where along each axis the anchor is.
    const fn sides(self) -> (Side, Side) {
        match self {
            Self::TopLeft => (Side::Start, Side::Start),
            Self::Top => (Side::Middle, Side::Start),
            Self::TopRight => (Side::End, Side::Start),
            Self::Left => (Side::Start, Side::Middle),
            Self::Center => (Side::Middle, Side::Middle),
            Self::Right => (Side::End, Side::Middle),
            Self::BottomLeft => (Side::Start, Side::End),
            Self::Bottom => (Side::Middle, Side::End),
            Self::BottomRight => (Side::End, Side::End),
        }
    }
}

/// Where along an axis an anchor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// At its start (left or top).
    Start,
    /// In its middle.
    Middle,
    /// At its end (right or bottom).
    End,
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Parses a rules file.
    fn rules(text: &str) -> Result<Rules, String> {
        toml::from_str(text).map_err(|error| error.message().to_owned())
    }

    #[rstest]
    #[case::centered(Anchor::Center, [0, 0], (300, 200))]
    #[case::bottom_right_moved_in(Anchor::BottomRight, [10, 20], (590, 380))]
    #[case::top_moved_right(Anchor::Top, [5, 0], (305, 0))]
    fn a_position_anchors_the_frame(
        #[case] anchor: Anchor,
        #[case] offset: [i32; 2],
        #[case] expected: (i32, i32),
    ) {
        let usable = Rectangle::new((0, 0).into(), (1000, 600).into());
        let corner = Position { anchor, offset }.corner(usable, (400, 200).into());
        assert_eq!((corner.x, corner.y), expected);
    }

    #[test]
    fn later_rules_override_key_by_key() {
        let earlier = RuleSettings {
            workspace: NonZeroUsize::new(2),
            sticky: Some(true),
            ..RuleSettings::default()
        };
        let later = RuleSettings {
            workspace: NonZeroUsize::new(3),
            ..RuleSettings::default()
        };
        let merged = earlier.overridden_by(&later);
        assert_eq!(merged.workspace, NonZeroUsize::new(3));
        assert_eq!(merged.sticky, Some(true));
    }

    #[rstest]
    #[case::no_match("[[rule]]\nworkspace = 2\n", "missing field `match`")]
    #[case::not_yet("[[rule]]\nmatch = {}\nfocus_stealing = \"allow\"\n", "unknown field")]
    #[case::bad_placement("[[rule]]\nmatch = {}\nplacement = \"huge\"\n", "placement is")]
    #[case::workspace_zero("[[rule]]\nmatch = {}\nworkspace = 0\n", "nonzero")]
    fn mistakes_are_explained(#[case] text: &str, #[case] expected: &str) {
        let error = rules(text).expect_err(text);
        assert!(error.contains(expected), "{error}");
    }
}
