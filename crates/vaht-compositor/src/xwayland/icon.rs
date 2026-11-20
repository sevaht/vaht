//! X11 windows' icons (`_NET_WM_ICON`, else the icon bitmap of `WM_HINTS`),
//! as Fluxbox shows them.
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
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, ImageFormat, ImageOrder,
};
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
                Ok(Some(Event::PropertyNotify(notify)))
                    if notify.atom == self.net_wm_icon
                        || notify.atom == u32::from(AtomEnum::WM_HINTS) =>
                {
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
            // A program that sets no `_NET_WM_ICON` may still hand over an
            // icon bitmap in its `WM_HINTS` (xterm does).
            Ok(values) => decode(&values).or_else(|| self.hinted_icon(window)),
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

impl IconTracker {
    /// The `WM_HINTS` flag that says `icon_pixmap` is set.
    const ICON_PIXMAP_HINT: u32 = 1 << 2;
    /// The `WM_HINTS` flag that says `icon_mask` is set.
    const ICON_MASK_HINT: u32 = 1 << 5;

    /// Returns the icon in the `WM_HINTS` of the X window `window`: the
    /// bitmap (or picture) it names, shaped by its mask if it has one.
    fn hinted_icon(&self, window: u32) -> Option<vaht_ui::Icon> {
        let reply = self
            .connection
            .get_property(false, window, AtomEnum::WM_HINTS, AtomEnum::WM_HINTS, 0, 9)
            .ok()?
            .reply()
            .ok()?;
        // flags, input, initial_state, icon_pixmap, icon_window, icon_x,
        // icon_y, icon_mask, window_group.
        let hints: Vec<u32> = reply.value32()?.collect();
        let (flags, pixmap, mask) = (*hints.first()?, *hints.get(3)?, *hints.get(7)?);
        if flags & Self::ICON_PIXMAP_HINT == 0 || pixmap == 0 {
            return None;
        }
        let picture = self.read_pixmap(pixmap)?;
        let mask = (flags & Self::ICON_MASK_HINT != 0 && mask != 0)
            .then(|| self.read_pixmap(mask))
            .flatten();
        hinted_pixels(&picture, mask.as_ref())
    }

    /// Reads the pixmap `pixmap`: a bitmap (depth 1) or a picture.
    fn read_pixmap(&self, pixmap: u32) -> Option<Bitmap> {
        let geometry = self.connection.get_geometry(pixmap).ok()?.reply().ok()?;
        let (width, height) = (u32::from(geometry.width), u32::from(geometry.height));
        if !(1..=Self::MAX_SIDE).contains(&width) || !(1..=Self::MAX_SIDE).contains(&height) {
            return None;
        }
        let image = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                pixmap,
                0,
                0,
                geometry.width,
                geometry.height,
                !0,
            )
            .ok()?
            .reply()
            .ok()?;
        let setup = self.connection.setup();
        let format = setup
            .pixmap_formats
            .iter()
            .find(|format| format.depth == image.depth)?;
        Some(Bitmap {
            width,
            height,
            depth: image.depth,
            bits_per_pixel: u32::from(format.bits_per_pixel),
            row_padding: u32::from(format.scanline_pad),
            is_most_significant_first: if image.depth == 1 {
                setup.bitmap_format_bit_order == ImageOrder::MSB_FIRST
            } else {
                setup.image_byte_order == ImageOrder::MSB_FIRST
            },
            data: image.data,
        })
    }
}

/// A pixmap read from the X server.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Bitmap {
    /// Pixels across.
    width: u32,
    /// Pixels down.
    height: u32,
    /// The pixmap's depth: 1 for a bitmap.
    depth: u8,
    /// Bits to a pixel in the data.
    bits_per_pixel: u32,
    /// Every row is padded to a multiple of this many bits.
    row_padding: u32,
    /// For a bitmap, whether the first pixel of a byte is its high bit; for
    /// a picture, whether a pixel's bytes are most significant first.
    is_most_significant_first: bool,
    /// The rows.
    data: Vec<u8>,
}

impl Bitmap {
    /// Returns the bytes in a row.
    const fn stride(&self) -> usize {
        let bits = self.width * self.bits_per_pixel;
        let padded = bits.div_ceil(self.row_padding) * self.row_padding;
        (padded / 8) as usize
    }

    /// Returns whether the pixel at (`x`, `y`) of a bitmap is set.
    fn bit(&self, x: u32, y: u32) -> bool {
        let row = y as usize * self.stride();
        let byte = self.data.get(row + (x / 8) as usize).copied().unwrap_or(0);
        let place = x % 8;
        let shift = if self.is_most_significant_first {
            7 - place
        } else {
            place
        };
        byte >> shift & 1 == 1
    }

    /// Returns the red, green and blue of the pixel at (`x`, `y`) of a
    /// picture of 32 bits to a pixel (24 bits of color), if it is one.
    fn rgb(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        if self.bits_per_pixel != 32 {
            return None;
        }
        let at = y as usize * self.stride() + x as usize * 4;
        let bytes: [u8; 4] = self.data.get(at..at + 4)?.try_into().ok()?;
        // The pixel is 0x00RRGGBB, stored in the server's byte order.
        let pixel = if self.is_most_significant_first {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        };
        let [_, red, green, blue] = pixel.to_be_bytes();
        Some([red, green, blue])
    }
}

/// Returns the icon of `picture`, the icon pixmap of a window's `WM_HINTS`,
/// shaped by `mask` where the program gave one (a set bit is a pixel that
/// shows). A bitmap is black where it is set and white where it is not, so
/// it is seen against any taskbar; a picture keeps its colors.
fn hinted_pixels(picture: &Bitmap, mask: Option<&Bitmap>) -> Option<vaht_ui::Icon> {
    let mut bytes = Vec::with_capacity((picture.width * picture.height * 4) as usize);
    for y in 0..picture.height {
        for x in 0..picture.width {
            let shows = mask.is_none_or(|mask| {
                x < mask.width && y < mask.height && mask.depth == 1 && mask.bit(x, y)
            });
            let [red, green, blue] = if picture.depth == 1 {
                let value = if picture.bit(x, y) { 0 } else { 255 };
                [value; 3]
            } else {
                picture.rgb(x, y)?
            };
            bytes.extend_from_slice(&[if shows { 255 } else { 0 }, red, green, blue]);
        }
    }
    vaht_ui::Icon::from_argb_bytes(picture.width, picture.height, &bytes)
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

    /// A `width` × `height` bitmap whose rows are `rows` (a `#` is a set
    /// bit), packed as the X server sends them: 32-bit padded rows.
    fn bitmap(rows: &[&str], is_most_significant_first: bool) -> Bitmap {
        let width = u32::try_from(rows[0].len()).expect("small");
        let mut data = Vec::new();
        for row in rows {
            let mut bytes = vec![0_u8; 4];
            for (x, character) in row.chars().enumerate() {
                if character == '#' {
                    let place = u32::try_from(x % 8).expect("small");
                    let shift = if is_most_significant_first {
                        7 - place
                    } else {
                        place
                    };
                    bytes[x / 8] |= 1 << shift;
                }
            }
            data.extend(bytes);
        }
        Bitmap {
            width,
            height: u32::try_from(rows.len()).expect("small"),
            depth: 1,
            bits_per_pixel: 1,
            row_padding: 32,
            is_most_significant_first,
            data,
        }
    }

    /// Returns the red, green, blue and alpha of `icon`'s pixel at (`x`, `y`),
    /// with `icon` the size of a 4 by 2 bitmap (unscaled).
    fn at(icon: &vaht_ui::Icon, x: u32, y: u32) -> [u8; 4] {
        let color = icon.pixels().pixel(x, y).expect("inside").demultiply();
        [color.red(), color.green(), color.blue(), color.alpha()]
    }

    #[test]
    fn a_hinted_bitmap_is_black_ink_on_white_whichever_way_its_bits_are_ordered() {
        for most_significant_first in [false, true] {
            let ink = bitmap(&["#..#", ".##."], most_significant_first);
            let icon = hinted_pixels(&ink, None).expect("an icon");
            // Fitted into a square 4 wide, 2 rows centered: rows 1 and 2.
            assert_eq!(at(&icon, 0, 1), [0, 0, 0, 255], "set: black");
            assert_eq!(at(&icon, 1, 1), [255, 255, 255, 255], "clear: white");
            assert_eq!(at(&icon, 1, 2), [0, 0, 0, 255]);
            assert_eq!(at(&icon, 0, 2), [255, 255, 255, 255]);
        }
    }

    #[test]
    fn a_hinted_mask_leaves_the_pixels_outside_it_clear() {
        let ink = bitmap(&["####", "####"], true);
        let mask = bitmap(&[".##.", ".##."], true);
        let icon = hinted_pixels(&ink, Some(&mask)).expect("an icon");
        assert_eq!(at(&icon, 0, 1)[3], 0, "outside the mask");
        assert_eq!(at(&icon, 1, 1), [0, 0, 0, 255], "inside it");
    }

    #[test]
    fn a_hinted_picture_keeps_its_colors() {
        // 32 bits to a pixel, least significant byte first: blue, green, red.
        let picture = Bitmap {
            width: 2,
            height: 1,
            depth: 24,
            bits_per_pixel: 32,
            row_padding: 32,
            is_most_significant_first: false,
            data: vec![0x00, 0x00, 0xff, 0x00, 0xff, 0x00, 0x00, 0x00],
        };
        let icon = hinted_pixels(&picture, None).expect("an icon");
        assert_eq!(at(&icon, 0, 0), [255, 0, 0, 255], "red");
        assert_eq!(at(&icon, 1, 0), [0, 0, 255, 255], "blue");
    }
}
