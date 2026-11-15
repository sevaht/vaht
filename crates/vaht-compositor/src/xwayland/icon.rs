//! X11 windows' icons (`_NET_WM_ICON`), as Fluxbox shows them.
//!
//! An X program hands its icon over as pixels in a property of its window,
//! where a Wayland program names it or draws it over xdg-toplevel-icon.
//! [`IconTracker`] reads the property when a window maps, and again
//! whenever the program changes it, over a connection of vaht's own to
//! Xwayland; the window then shows the icon as it shows a Wayland
//! program's drawn one.

use std::collections::HashMap;
use std::os::fd::{AsFd as _, OwnedFd};

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask};
use x11rb::rust_connection::RustConnection;

use crate::Compositor;
use crate::event_source::EventSource;
use crate::resources::ResourceLedger;

/// Each X11 window's icon, followed over vaht's own X connection.
#[derive(Debug)]
pub(crate) struct IconTracker {
    /// vaht's connection to Xwayland, for icons alone.
    connection: RustConnection,
    /// The `_NET_WM_ICON` atom.
    net_wm_icon: u32,
    /// Each window's icon, by X window id; a window without one has none.
    icons: HashMap<u32, vaht_ui::Icon>,
    /// The connection's events, registered while the tracker lives.
    _source: EventSource<Compositor>,
}

impl IconTracker {
    /// The widest and tallest icon read, in pixels: larger ones are
    /// skipped (a property that big is more likely broken than an icon).
    const MAX_SIDE: u32 = 1024;

    /// Connects to Xwayland on `display` (such as `:1`) for icons.
    ///
    /// # Errors
    ///
    /// Returns why not: the display cannot be reached.
    pub(crate) fn connect(
        display: &str,
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
    ) -> Result<Self, String> {
        let (connection, _) = x11rb::connect(Some(display))
            .map_err(|error| format!("cannot reach {display}: {error}"))?;
        let net_wm_icon = connection
            .intern_atom(false, b"_NET_WM_ICON")
            .map_err(|error| error.to_string())?
            .reply()
            .map_err(|error| error.to_string())?
            .atom;
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
                compositor.xwayland_icons_changed();
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| format!("cannot register the connection: {}", error.error))?;
        Ok(Self {
            connection,
            net_wm_icon,
            icons: HashMap::new(),
            _source: source,
        })
    }

    /// Starts following the icon of the X window `window`, just mapped.
    pub(crate) fn track(&mut self, window: u32) {
        let attributes = ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE);
        let selected = self
            .connection
            .change_window_attributes(window, &attributes)
            .map(|_| ())
            .and_then(|()| self.connection.flush());
        if let Err(error) = selected {
            tracing::debug!(%error, window, "a window's icon is not followed");
            return;
        }
        self.refresh(window);
    }

    /// Stops following the icon of the X window `window`, which is gone.
    pub(crate) fn untrack(&mut self, window: u32) {
        self.icons.remove(&window);
    }

    /// Returns the icon of the X window `window`, if it has one.
    pub(crate) fn icon_of(&self, window: u32) -> Option<&vaht_ui::Icon> {
        self.icons.get(&window)
    }

    /// Reads what the X server sent, and reads again the icon of each
    /// window whose icon changed; returns those windows.
    pub(crate) fn follow_changes(&mut self) -> Vec<u32> {
        let mut changed = Vec::new();
        loop {
            match self.connection.poll_for_event() {
                Ok(Some(Event::PropertyNotify(notify))) if notify.atom == self.net_wm_icon => {
                    if !changed.contains(&notify.window) {
                        changed.push(notify.window);
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(%error, "vaht's X connection for icons failed");
                    break;
                }
            }
        }
        changed.retain(|&window| self.refresh(window));
        changed
    }

    /// Reads the icon of the X window `window` again; returns whether it
    /// changed.
    fn refresh(&mut self, window: u32) -> bool {
        let values = self
            .connection
            .get_property(
                false,
                window,
                self.net_wm_icon,
                AtomEnum::CARDINAL,
                0,
                u32::MAX,
            )
            .map_err(|error| error.to_string())
            .and_then(|cookie| cookie.reply().map_err(|error| error.to_string()))
            .map(|reply| {
                reply
                    .value32()
                    .map(Iterator::collect::<Vec<u32>>)
                    .unwrap_or_default()
            });
        let icon = match values {
            Ok(values) => decode(&values),
            Err(error) => {
                tracing::debug!(error, window, "a window's icon could not be read");
                return false;
            }
        };
        let Some(icon) = icon else {
            return self.icons.remove(&window).is_some();
        };
        tracing::debug!(window, size = icon.size(), "window icon read");
        self.icons.insert(window, icon);
        true
    }
}

/// Returns the largest picture in a `_NET_WM_ICON` property's `values`:
/// pictures one after another, each its width, its height, then its pixels
/// row by row, each `0xAARRGGBB` (not premultiplied). Truncated pictures,
/// and those empty or wider or taller than [`IconTracker::MAX_SIDE`], are
/// skipped.
fn decode(values: &[u32]) -> Option<vaht_ui::Icon> {
    let mut largest: Option<(u32, u32, &[u32])> = None;
    let mut rest = values;
    while let [width, height, after @ ..] = rest {
        let (width, height) = (*width, *height);
        let Some(count) = usize::try_from(u64::from(width) * u64::from(height))
            .ok()
            .filter(|&count| count <= after.len())
        else {
            break;
        };
        let (pixels, next) = after.split_at(count);
        let is_usable = (1..=IconTracker::MAX_SIDE).contains(&width)
            && (1..=IconTracker::MAX_SIDE).contains(&height);
        let is_larger = largest.is_none_or(|(known_width, known_height, _)| {
            u64::from(width) * u64::from(height) > u64::from(known_width) * u64::from(known_height)
        });
        if is_usable && is_larger {
            largest = Some((width, height, pixels));
        }
        rest = next;
    }
    let (width, height, pixels) = largest?;
    let bytes: Vec<u8> = pixels
        .iter()
        .flat_map(|pixel| pixel.to_be_bytes())
        .collect();
    vaht_ui::Icon::from_argb_bytes(width, height, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width` × `height` picture of `pixel`, as the property holds it.
    fn picture(width: u32, height: u32, pixel: u32) -> Vec<u32> {
        let count = usize::try_from(width * height).expect("small");
        [width, height]
            .into_iter()
            .chain(std::iter::repeat_n(pixel, count))
            .collect()
    }

    /// Returns the red, green, blue and alpha of `icon`'s middle pixel.
    fn middle(icon: &vaht_ui::Icon) -> [u8; 4] {
        let pixels = icon.pixels();
        let color = pixels
            .pixel(pixels.width() / 2, pixels.height() / 2)
            .expect("inside")
            .demultiply();
        [color.red(), color.green(), color.blue(), color.alpha()]
    }

    #[test]
    fn the_largest_picture_is_the_icon() {
        let values = [picture(16, 16, 0xff00_00ff), picture(32, 32, 0xffff_0000)].concat();
        let icon = decode(&values).expect("an icon");
        assert_eq!(icon.size(), 32);
        assert_eq!(middle(&icon), [0xff, 0, 0, 0xff], "the 32-pixel red one");
    }

    #[test]
    fn a_translucent_pixel_keeps_its_color() {
        let icon = decode(&picture(4, 4, 0x8000_ff00)).expect("an icon");
        let [red, green, blue, alpha] = middle(&icon);
        assert_eq!((red, blue, alpha), (0, 0, 0x80));
        assert!(green >= 0xfe, "green, not premultiplied twice: {green}");
    }

    #[test]
    fn a_truncated_picture_is_skipped_but_whole_ones_before_it_count() {
        let mut values = picture(8, 8, 0xff00_ff00);
        values.extend([64, 64, 0xffff_ffff]);
        assert_eq!(decode(&values).map(|icon| icon.size()), Some(8));
    }

    #[test]
    fn an_empty_or_absurd_property_gives_no_icon() {
        assert!(decode(&[]).is_none());
        assert!(decode(&[0, 0]).is_none(), "an empty picture");
        assert!(decode(&[5000, 1, 0]).is_none(), "truncated and too wide");
    }
}
