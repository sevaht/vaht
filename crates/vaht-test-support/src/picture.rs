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

/// Returns an xcursor file holding one picture `size` pixels square, all
/// `color`, whose hotspot is its middle.
#[must_use]
pub fn solid_xcursor(size: u32, color: [u8; 3]) -> Vec<u8> {
    /// The table-of-contents type of an image chunk.
    const IMAGE_TYPE: u32 = 0xfffd_0002;
    /// The bytes of the file header: magic, header size, version, entries.
    const FILE_HEADER: u32 = 16;
    /// The bytes of one table-of-contents entry.
    const TOC_ENTRY: u32 = 12;
    /// The bytes of an image chunk's header.
    const IMAGE_HEADER: u32 = 36;
    let mut file = Vec::new();
    let mut word = |value: u32| file.extend_from_slice(&value.to_le_bytes());
    // The file header, then its one table-of-contents entry.
    for value in [FILE_HEADER, 0x0001_0000, 1] {
        word(value);
    }
    for value in [IMAGE_TYPE, size, FILE_HEADER + TOC_ENTRY] {
        word(value);
    }
    // The image: its header, then premultiplied ARGB pixels.
    let middle = size / 2;
    for value in [
        IMAGE_HEADER,
        IMAGE_TYPE,
        size,
        1,
        size,
        size,
        middle,
        middle,
        0,
    ] {
        word(value);
    }
    let [red, green, blue] = color;
    let pixel = u32::from_be_bytes([255, red, green, blue]);
    for _ in 0..size * size {
        word(pixel);
    }
    let mut bytes = b"Xcur".to_vec();
    bytes.extend_from_slice(&file);
    bytes
}
