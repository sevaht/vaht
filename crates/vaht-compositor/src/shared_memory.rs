//! Writing pixels into a client's shared-memory buffer: vaht's only `unsafe`.
//!
//! Screenshots and screen recording hand vaht a buffer that lives in memory
//! the client also maps. Smithay exposes it only as a raw pointer, and there
//! is no safe way to write through one (R-X3): the client may change that
//! memory at any moment, so even a Rust reference into it would be undefined
//! behavior. [`write_frame`] therefore never creates one. It validates every
//! bound first, then copies each row with `ptr::copy_nonoverlapping`, which
//! reads only from vaht's own memory and writes only inside the buffer.

use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_shm::Format;
use smithay::wayland::shm::{BufferAccessError, BufferData, with_buffer_contents_mut};

/// Bytes per pixel of the 32-bit formats vaht writes.
const BYTES_PER_PIXEL: usize = 4;

/// Copies a `width` × `height` image of 32-bit BGRA pixels (ARGB8888 in
/// memory), rows packed, into the client's shared-memory `buffer`.
///
/// # Errors
///
/// Returns an error, writing nothing, if the buffer is not shared memory,
/// is not a 32-bit RGB format, has another size, or does not fit in its pool.
pub(crate) fn write_frame(
    buffer: &WlBuffer,
    pixels: &[u8],
    width: usize,
    height: usize,
) -> Result<(), SharedMemoryError> {
    let row_length = width
        .checked_mul(BYTES_PER_PIXEL)
        .ok_or(SharedMemoryError::OutOfBounds)?;
    if row_length.checked_mul(height) != Some(pixels.len()) {
        return Err(SharedMemoryError::SizeMismatch);
    }
    with_buffer_contents_mut(buffer, |pool, pool_length, data| {
        let layout = RowLayout::of(data, width, height, pool_length)?;
        for (row_index, row) in pixels.chunks_exact(row_length).enumerate() {
            let start = layout.offset + row_index * layout.stride;
            #[expect(unsafe_code, reason = "the only way to write a client's shared memory")]
            // SAFETY: `RowLayout::of` checked that every row's range,
            // `start..start + row_length`, lies inside the pool's
            // `pool_length` bytes, so `pool.add(start)` stays in the mapping
            // and the copy writes nowhere else. `row` is vaht's own memory and
            // cannot overlap the client's mapping. No Rust reference into the
            // shared memory is created, so the client changing it meanwhile is
            // not undefined behavior.
            unsafe {
                std::ptr::copy_nonoverlapping(row.as_ptr(), pool.add(start), row_length);
            }
        }
        Ok(())
    })
    .map_err(SharedMemoryError::Access)?
}

/// Where a buffer's rows lie in its pool, checked against the pool's length.
#[derive(Debug, Clone, Copy)]
struct RowLayout {
    /// The first row's offset into the pool, in bytes.
    offset: usize,
    /// The distance between rows, in bytes.
    stride: usize,
}

impl RowLayout {
    /// Checks that a buffer described by `data` holds `height` rows of
    /// `width` 32-bit pixels entirely inside a pool of `pool_length` bytes.
    fn of(
        data: BufferData,
        width: usize,
        height: usize,
        pool_length: usize,
    ) -> Result<Self, SharedMemoryError> {
        if !matches!(data.format, Format::Argb8888 | Format::Xrgb8888) {
            return Err(SharedMemoryError::UnsupportedFormat(data.format));
        }
        let to_size =
            |value: i32| usize::try_from(value).map_err(|_negative| SharedMemoryError::OutOfBounds);
        let (offset, stride) = (to_size(data.offset)?, to_size(data.stride)?);
        if to_size(data.width)? != width || to_size(data.height)? != height {
            return Err(SharedMemoryError::SizeMismatch);
        }
        let row_length = width * BYTES_PER_PIXEL;
        if stride < row_length {
            return Err(SharedMemoryError::OutOfBounds);
        }
        // The last row ends at offset + (height - 1) × stride + row length.
        let end = height
            .checked_sub(1)
            .map_or(Some(offset), |last_row| {
                last_row
                    .checked_mul(stride)
                    .and_then(|start| start.checked_add(offset))
                    .and_then(|start| start.checked_add(row_length))
            })
            .ok_or(SharedMemoryError::OutOfBounds)?;
        if end > pool_length {
            return Err(SharedMemoryError::OutOfBounds);
        }
        Ok(Self { offset, stride })
    }
}

/// Why pixels could not be written into a client's buffer.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SharedMemoryError {
    /// The buffer is not shared memory, or its memory could not be mapped.
    #[error("could not access the client's buffer")]
    Access(#[source] BufferAccessError),

    /// The buffer is not 32-bit RGB.
    #[error("unsupported buffer format {0:?}")]
    UnsupportedFormat(Format),

    /// The buffer is not the size of the image.
    #[error("the buffer is not the size of the image")]
    SizeMismatch,

    /// The buffer's rows would lie outside its pool.
    #[error("the buffer lies outside its memory pool")]
    OutOfBounds,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10 × 4 ARGB8888 buffer with rows 48 bytes apart, starting at 16.
    const DATA: BufferData = BufferData {
        offset: 16,
        width: 10,
        height: 4,
        stride: 48,
        format: Format::Argb8888,
    };

    /// The pool length that exactly fits [`DATA`]: 16 + 3 × 48 + 40.
    const EXACT_POOL: usize = 200;

    #[test]
    fn a_buffer_that_fits_its_pool_exactly_is_accepted() {
        let layout = RowLayout::of(DATA, 10, 4, EXACT_POOL).expect("fits");
        assert_eq!((layout.offset, layout.stride), (16, 48));
    }

    #[test]
    fn a_buffer_one_byte_past_its_pool_is_rejected() {
        assert!(matches!(
            RowLayout::of(DATA, 10, 4, EXACT_POOL - 1),
            Err(SharedMemoryError::OutOfBounds)
        ));
    }

    #[test]
    fn rows_narrower_than_the_image_are_rejected() {
        let narrow = BufferData { stride: 36, ..DATA };
        assert!(matches!(
            RowLayout::of(narrow, 10, 4, 10_000),
            Err(SharedMemoryError::OutOfBounds)
        ));
    }

    #[test]
    fn negative_offsets_and_other_formats_are_rejected() {
        let negative = BufferData { offset: -4, ..DATA };
        assert!(RowLayout::of(negative, 10, 4, 10_000).is_err());
        let rgb565 = BufferData {
            format: Format::Rgb565,
            ..DATA
        };
        assert!(matches!(
            RowLayout::of(rgb565, 10, 4, 10_000),
            Err(SharedMemoryError::UnsupportedFormat(Format::Rgb565))
        ));
    }

    #[test]
    fn a_size_other_than_the_image_is_rejected() {
        assert!(matches!(
            RowLayout::of(DATA, 11, 4, 10_000),
            Err(SharedMemoryError::SizeMismatch)
        ));
    }
}
