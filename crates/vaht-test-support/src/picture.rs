//! Pictures tests give vaht, such as application icons.

/// Returns a PNG file `size` pixels square, all `color`.
///
/// # Panics
///
/// Panics if `size` is 0.
#[must_use]
pub fn solid_png(size: u32, color: [u8; 3]) -> Vec<u8> {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("a picture has an area");
    let [red, green, blue] = color;
    pixmap.fill(tiny_skia::Color::from_rgba8(red, green, blue, 255));
    pixmap.encode_png().expect("a pixmap encodes as PNG")
}
