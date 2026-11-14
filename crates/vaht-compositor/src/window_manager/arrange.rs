//! Arranging the windows on an output (`arrange`, Fluxbox's
//! `ArrangeWindows` and kin), decided as tile cells from a count alone.
//!
//! A grid has as many columns as the square root of the count, rounded
//! up, and the rows it takes; the last row's windows share its whole
//! width. Columns put the windows side by side, rows one above another.
//! Cells meet exactly: each edge is where the next cell starts.

use super::TileCell;

/// How windows are arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString, strum::Display)]
#[strum(serialize_all = "kebab-case")]
pub(crate) enum ArrangeStyle {
    /// In a grid.
    Grid,
    /// Side by side, each the full height.
    Columns,
    /// One above another, each the full width.
    Rows,
}

/// The whole of an axis, in the thousandths cells are measured in.
const WHOLE: u32 = 1000;

/// Returns the cells of `count` windows arranged as `style`, in reading
/// order.
pub(crate) fn cells(count: usize, style: ArrangeStyle) -> Vec<TileCell> {
    if count == 0 {
        return Vec::new();
    }
    let count = u32::try_from(count).unwrap_or(u32::MAX).min(WHOLE);
    let rows: Vec<u32> = match style {
        ArrangeStyle::Columns => vec![count],
        ArrangeStyle::Rows => vec![1; count as usize],
        ArrangeStyle::Grid => {
            let columns = (1..=count)
                .find(|columns| columns * columns >= count)
                .unwrap_or(count);
            let full = count / columns;
            let rest = count % columns;
            let mut rows = vec![columns; full as usize];
            if rest > 0 {
                rows.push(rest);
            }
            rows
        }
    };
    let row_count = u32::try_from(rows.len()).unwrap_or(1);
    let edge = |index: u32, parts: u32| index * WHOLE / parts;
    let narrow = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
    rows.iter()
        .zip(0..)
        .flat_map(|(&columns, row)| {
            (0..columns).filter_map(move |column| {
                let (left, right) = (edge(column, columns), edge(column + 1, columns));
                let (top, bottom) = (edge(row, row_count), edge(row + 1, row_count));
                TileCell::new(
                    narrow(left),
                    narrow(top),
                    narrow(right - left),
                    narrow(bottom - top),
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell from its edges in thousandths.
    fn cell(x: u16, y: u16, width: u16, height: u16) -> TileCell {
        TileCell::new(x, y, width, height).expect("a valid cell")
    }

    #[test]
    fn five_windows_make_a_grid_whose_last_row_shares_its_width() {
        assert_eq!(
            cells(5, ArrangeStyle::Grid),
            [
                cell(0, 0, 333, 500),
                cell(333, 0, 333, 500),
                cell(666, 0, 334, 500),
                cell(0, 500, 500, 500),
                cell(500, 500, 500, 500),
            ],
            "three columns, two rows, the last row sharing its width"
        );
    }

    #[test]
    fn columns_and_rows_split_one_axis() {
        assert_eq!(
            cells(3, ArrangeStyle::Columns),
            [
                cell(0, 0, 333, 1000),
                cell(333, 0, 333, 1000),
                cell(666, 0, 334, 1000)
            ]
        );
        assert_eq!(
            cells(2, ArrangeStyle::Rows),
            [cell(0, 0, 1000, 500), cell(0, 500, 1000, 500)]
        );
        assert_eq!(cells(1, ArrangeStyle::Grid), [cell(0, 0, 1000, 1000)]);
        assert_eq!(cells(0, ArrangeStyle::Grid), []);
    }
}
