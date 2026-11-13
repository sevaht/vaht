//! The sizes of frames and the taskbar as vaht's built-in configuration
//! draws them, which tests measure windows by.
//!
//! [`BUILT_IN_METRICS`] states them once for every test; a unit test checks
//! them against the built-in theme itself, so a theme change that moves
//! them fails here first, with the new values.

use crate::Bounds;

/// Sizes, in pixels, that window frames and the taskbar take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeMetrics {
    /// A titlebar's height.
    pub titlebar_height: u32,
    /// A frame line's width.
    pub border_width: u32,
    /// The handle's height, along a window's bottom; 0 for none.
    pub handle_height: u32,
    /// The taskbar's height; it runs along the bottom.
    pub taskbar_height: u32,
}

/// The built-in configuration's sizes: frames have no handle, as the
/// built-in settings leave it out.
pub const BUILT_IN_METRICS: ThemeMetrics = ThemeMetrics {
    titlebar_height: 20,
    border_width: 1,
    handle_height: 0,
    taskbar_height: 20,
};

impl ThemeMetrics {
    /// Returns how much frame is above a window's content: a line, the
    /// titlebar and the line under it.
    #[must_use]
    pub const fn above_content(&self) -> u32 {
        2 * self.border_width + self.titlebar_height
    }

    /// Returns how much frame is below a window's content: a line, and
    /// with a handle, the handle and the line under it.
    #[must_use]
    pub const fn below_content(&self) -> u32 {
        if self.handle_height > 0 {
            2 * self.border_width + self.handle_height
        } else {
            self.border_width
        }
    }

    /// Returns the content of a window whose frame fills `outer`.
    #[must_use]
    pub const fn content_within(&self, outer: Bounds) -> Bounds {
        Bounds {
            left: outer.left + self.border_width,
            top: outer.top + self.above_content(),
            right: outer.right - self.border_width,
            bottom: outer.bottom - self.below_content(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_metrics_are_the_built_in_theme_s() {
        let theme = vaht_ui::Theme::built_in().metrics();
        assert_eq!(
            BUILT_IN_METRICS,
            ThemeMetrics {
                titlebar_height: theme.titlebar_height,
                border_width: theme.border_width,
                // The built-in settings show no handle, whatever its height.
                handle_height: 0,
                taskbar_height: theme.taskbar_height,
            }
        );
    }
}
