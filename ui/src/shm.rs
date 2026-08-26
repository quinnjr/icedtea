//! The Skia -> `wl_shm` seam.
//!
//! `skia_rs_canvas::Surface` cannot wrap external pixel memory:
//! `Surface::new_raster` always allocates its own `PixelBuffer`, which is
//! always physically RGBA with premultiplied alpha and a `width * 4` stride.
//! `wl_shm`'s `Argb8888` is little-endian `0xAARRGGBB`, i.e. `B, G, R, A` in
//! memory, also premultiplied. So the seam is one copy with an R/B swap and
//! no alpha arithmetic at all.

use std::fs::File;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::FileExt;

use skia_rs_safe::canvas::Surface;
use wayland_client::QueueHandle;
use wayland_client::protocol::{wl_buffer, wl_shm, wl_shm_pool};

use crate::wayland::AppState;

/// Convert Skia's physical RGBA-premultiplied bytes into `Argb8888` bytes.
///
/// Copies `min(src.len(), dst.len()) / 4` whole pixels; a partial trailing
/// pixel is ignored rather than half-written.
pub fn skia_rgba_to_shm_argb(src: &[u8], dst: &mut [u8]) {
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        d[0] = s[2]; // B
        d[1] = s[1]; // G
        d[2] = s[0]; // R
        d[3] = s[3]; // A
    }
}

/// A memfd-backed `wl_shm` buffer in `Argb8888`.
pub struct ShmBuffer {
    file: File,
    _pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
    width: i32,
    height: i32,
    scratch: Vec<u8>,
}

impl ShmBuffer {
    /// Allocate a `width` x `height` `Argb8888` buffer.
    ///
    /// # Errors
    ///
    /// Returns the underlying `memfd_create`/`ftruncate` error, or an
    /// `InvalidInput` error if `width`/`height` do not describe a positive
    /// buffer size.
    pub fn new(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<AppState>,
        width: i32,
        height: i32,
    ) -> io::Result<Self> {
        let stride = width
            .checked_mul(4)
            .ok_or_else(|| io::Error::other("shm buffer stride overflows"))?;
        let len = stride
            .checked_mul(height)
            .and_then(|len| usize::try_from(len).ok())
            .filter(|len| *len > 0)
            .ok_or_else(|| io::Error::other("shm buffer size is not positive"))?;
        let fd: OwnedFd =
            rustix::fs::memfd_create("icedtea-ui-shm", rustix::fs::MemfdFlags::CLOEXEC)?;
        rustix::fs::ftruncate(&fd, len as u64)?;
        let file = File::from(fd);
        let pool = shm.create_pool(file.as_fd(), stride * height, qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, qh, ());
        Ok(Self {
            file,
            _pool: pool,
            buffer,
            width,
            height,
            scratch: vec![0u8; len],
        })
    }

    /// The `wl_buffer` to attach.
    #[must_use]
    pub fn wl_buffer(&self) -> &wl_buffer::WlBuffer {
        &self.buffer
    }

    /// Buffer dimensions in pixels.
    #[must_use]
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// Copy `surface`'s pixels into this buffer.
    ///
    /// Writes through the file rather than an mmap: the pool is `MAP_SHARED`
    /// on the compositor side, so a `pwrite` at offset 0 is visible there,
    /// and this keeps the whole path in safe Rust.
    ///
    /// # Errors
    ///
    /// Returns the `pwrite` error if the backing memfd cannot be written.
    pub fn upload(&mut self, surface: &Surface) -> io::Result<()> {
        skia_rgba_to_shm_argb(surface.pixels(), &mut self.scratch);
        self.file.write_all_at(&self.scratch, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::skia_rgba_to_shm_argb;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::{Color, Rect};
    use skia_rs_safe::paint::{Paint, Style};

    #[test]
    fn rgba_becomes_argb8888_byte_order() {
        // One opaque pixel: R=0x35 G=0x84 B=0xE4 A=0xFF in Skia's physical
        // RGBA order. wl_shm's Argb8888 is little-endian 0xAARRGGBB, i.e.
        // B, G, R, A in memory.
        let src = [0x35u8, 0x84, 0xE4, 0xFF];
        let mut dst = [0u8; 4];
        skia_rgba_to_shm_argb(&src, &mut dst);
        assert_eq!(dst, [0xE4, 0x84, 0x35, 0xFF]);
    }

    #[test]
    fn premultiplied_alpha_passes_through_untouched() {
        // Skia's buffer is already premultiplied and so is Argb8888, so the
        // conversion must not divide or multiply anything.
        let src = [0x40u8, 0x20, 0x10, 0x80];
        let mut dst = [0u8; 4];
        skia_rgba_to_shm_argb(&src, &mut dst);
        assert_eq!(dst, [0x10, 0x20, 0x40, 0x80]);
    }

    #[test]
    fn a_painted_surface_round_trips_to_the_expected_shm_bytes() {
        let mut surface = Surface::new_raster_n32_premul(4, 4).expect("surface");
        surface.canvas().clear(Color::TRANSPARENT);
        let mut paint = Paint::new();
        paint.set_color32(Color(0xFF35_84E4));
        paint.set_style(Style::Fill);
        surface
            .canvas()
            .draw_rect(&Rect::new(0.0, 0.0, 4.0, 4.0), &paint);

        let mut dst = vec![0u8; 4 * 4 * 4];
        skia_rgba_to_shm_argb(surface.pixels(), &mut dst);
        for pixel in dst.chunks_exact(4) {
            assert_eq!(pixel, [0xE4, 0x84, 0x35, 0xFF]);
        }
    }

    #[test]
    fn a_short_destination_is_filled_as_far_as_it_goes() {
        let src = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut dst = [0u8; 4];
        skia_rgba_to_shm_argb(&src, &mut dst);
        assert_eq!(dst, [3, 2, 1, 4]);
    }
}
