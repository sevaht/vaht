//! Where a window dragged to the usable area's edge goes (fluxland's snap
//! zones), and the layout picker shown at the top edge.
//!
//! [`SnapTarget::at`] finds the zone the pointer is in: within the zone
//! distance of the left or right edge, the window takes that half; of the
//! bottom, the bottom half; of a corner, that quarter; of the top, it is
//! maximized. [`LayoutPicker`] lays out the picker's thumbnails, one per
//! layout, and says which layout's cell the pointer is on: the window
//! then takes that cell.

use smithay::utils::{Logical, Point, Rectangle};

use super::{Area, TileCell};

/// Where a dragged window goes when the drag ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapTarget {
    /// Maximized.
    Maximize,
    /// On a cell of the usable area.
    Cell(TileCell),
}

impl SnapTarget {
    /// Returns the zone the pointer at `point` is in, within `distance`
    /// pixels of `usable`'s edges; corners before edges.
    pub(crate) fn at(point: Point<i32, Logical>, usable: Area, distance: i32) -> Option<Self> {
        let is_left = point.x - usable.loc.x < distance;
        let is_right = usable.loc.x + usable.size.w - point.x <= distance;
        let is_top = point.y - usable.loc.y < distance;
        let is_bottom = usable.loc.y + usable.size.h - point.y <= distance;
        let cell = |x, y, width, height| TileCell::new(x, y, width, height).map(Self::Cell);
        match (is_left, is_right, is_top, is_bottom) {
            (true, _, true, _) => cell(0, 0, 500, 500),
            (_, true, true, _) => cell(500, 0, 500, 500),
            (true, _, _, true) => cell(0, 500, 500, 500),
            (_, true, _, true) => cell(500, 500, 500, 500),
            (true, ..) => cell(0, 0, 500, 1000),
            (_, true, ..) => cell(500, 0, 500, 1000),
            (_, _, true, _) => Some(Self::Maximize),
            (_, _, _, true) => cell(0, 500, 1000, 500),
            _ => None,
        }
    }

    /// Returns the area the window would cover in `usable`.
    pub(crate) fn area_in(self, usable: Area) -> Area {
        match self {
            Self::Maximize => usable,
            Self::Cell(cell) => cell.area_in(usable),
        }
    }
}

/// The layout picker's place on an output: its panel and a thumbnail per
/// layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayoutPicker {
    /// The panel the thumbnails are on.
    panel: Area,
    /// Each layout's thumbnail, in order.
    thumbs: Vec<Area>,
}

impl LayoutPicker {
    /// The space around the thumbnails, and between them, in pixels.
    const PAD: i32 = 10;

    /// Lays out the picker of `layouts` on `usable`: thumbnails `size`
    /// pixels wide (at 16:10), at most `columns` to a row (0: as many as
    /// fit), centered just below the zone `distance` from the top edge.
    /// None without layouts.
    pub(crate) fn new(
        count: usize,
        usable: Area,
        size: u16,
        columns: u16,
        distance: i32,
    ) -> Option<Self> {
        if count == 0 {
            return None;
        }
        let count = i32::try_from(count).ok()?;
        let room = usable.size.w - 4 * Self::PAD;
        let width = i32::from(size).min(room).max(20);
        let height = width * 10 / 16;
        let fit = ((room + Self::PAD) / (width + Self::PAD)).max(1);
        let columns = match i32::from(columns) {
            0 => fit,
            asked => asked.min(fit),
        }
        .clamp(1, count);
        let rows = (count + columns - 1) / columns;
        let panel_width = columns * width + (columns + 1) * Self::PAD;
        let panel_height = rows * height + (rows + 1) * Self::PAD;
        let panel = Rectangle::new(
            (
                usable.loc.x + (usable.size.w - panel_width) / 2,
                usable.loc.y + distance + 4,
            )
                .into(),
            (panel_width, panel_height).into(),
        );
        let thumbs = (0..count)
            .map(|index| {
                Rectangle::new(
                    (
                        panel.loc.x + Self::PAD + (index % columns) * (width + Self::PAD),
                        panel.loc.y + Self::PAD + (index / columns) * (height + Self::PAD),
                    )
                        .into(),
                    (width, height).into(),
                )
            })
            .collect();
        Some(Self { panel, thumbs })
    }

    /// Returns the panel.
    pub(crate) const fn panel(&self) -> Area {
        self.panel
    }

    /// Returns each layout's thumbnail.
    pub(crate) fn thumbs(&self) -> &[Area] {
        &self.thumbs
    }

    /// Returns whether the picker keeps showing with the pointer at
    /// `point`: over it, or between it and `usable`'s top edge.
    pub(crate) fn holds(&self, point: Point<i32, Logical>, usable: Area) -> bool {
        point.x >= self.panel.loc.x
            && point.x < self.panel.loc.x + self.panel.size.w
            && point.y >= usable.loc.y
            && point.y < self.panel.loc.y + self.panel.size.h
    }

    /// Returns the layout and cell, of `layouts`, whose part of a
    /// thumbnail is under `point`.
    pub(crate) fn hit(
        &self,
        layouts: &[Vec<TileCell>],
        point: Point<i32, Logical>,
    ) -> Option<(usize, usize)> {
        self.thumbs
            .iter()
            .zip(layouts)
            .enumerate()
            .find_map(|(index, (thumb, cells))| {
                cells
                    .iter()
                    .position(|cell| cell.area_in(*thumb).contains(point))
                    .map(|cell| (index, cell))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1000 by 800 usable area.
    fn usable() -> Area {
        Rectangle::new((0, 0).into(), (1000, 800).into())
    }

    /// The target with the pointer at `x`, `y`.
    fn at(x: i32, y: i32) -> Option<SnapTarget> {
        SnapTarget::at((x, y).into(), usable(), 10)
    }

    #[test]
    fn edges_take_halves_corners_quarters_and_the_top_maximizes() {
        let area = |x: i32, y: i32| at(x, y).map(|target| target.area_in(usable()));
        let rect = |left, top, width, height| {
            Some(Rectangle::new((left, top).into(), (width, height).into()))
        };
        assert_eq!(area(2, 400), rect(0, 0, 500, 800), "left half");
        assert_eq!(area(995, 400), rect(500, 0, 500, 800), "right half");
        assert_eq!(area(500, 795), rect(0, 400, 1000, 400), "bottom half");
        assert_eq!(area(2, 2), rect(0, 0, 500, 400), "top-left quarter");
        assert_eq!(area(995, 795), rect(500, 400, 500, 400), "bottom-right");
        assert_eq!(at(500, 2), Some(SnapTarget::Maximize));
        assert_eq!(at(500, 400), None, "the middle is no zone");
    }

    #[test]
    fn the_picker_is_centered_below_the_top_zone_and_finds_cells() {
        let layouts = vec![
            vec![
                TileCell::new(0, 0, 500, 1000).expect("a cell"),
                TileCell::new(500, 0, 500, 1000).expect("a cell"),
            ];
            8
        ];
        let picker = LayoutPicker::new(layouts.len(), usable(), 120, 4, 10).expect("laid out");
        let panel = picker.panel();
        assert_eq!(panel.size.w, 4 * 120 + 5 * 10, "four to a row");
        assert_eq!(panel.size.h, 2 * 75 + 3 * 10, "two rows of 16:10");
        assert_eq!(panel.loc.x, (1000 - panel.size.w) / 2);
        assert_eq!(panel.loc.y, 14);
        let thumb = picker.thumbs()[5];
        let right_half = thumb.loc + Point::from((thumb.size.w * 3 / 4, thumb.size.h / 2));
        assert_eq!(picker.hit(&layouts, right_half), Some((5, 1)));
        assert_eq!(picker.hit(&layouts, panel.loc), None, "the padding");
        assert!(
            picker.holds((500, 0).into(), usable()),
            "above it, at the edge"
        );
        assert!(!picker.holds((500, 400).into(), usable()));
    }
}
