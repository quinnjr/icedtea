//! The Skia -> `wl_shm` seam.
//!
//! `skia_rs_canvas::Surface` cannot wrap external pixel memory:
//! `Surface::new_raster` always allocates its own `PixelBuffer`, which is
//! always physically RGBA with premultiplied alpha and a `width * 4` stride.
//! `wl_shm`'s `Argb8888` is little-endian `0xAARRGGBB`, i.e. `B, G, R, A` in
//! memory, also premultiplied. So the seam is one copy with an R/B swap and
//! no alpha arithmetic at all.
//!
//! # Double buffering
//!
//! A `wl_buffer` belongs to the compositor from the `wl_surface.commit` that
//! attaches it until it sends `wl_buffer.release`; writing into it before
//! then can tear or show a half-drawn frame. [`BufferPool`] therefore keeps
//! [`POOL_INITIAL_BUFFERS`] buffers, hands out only ones the compositor is
//! not holding, and grows to [`POOL_MAX_BUFFERS`] if every one is busy. The
//! bookkeeping is [`SlotPool`], which has no Wayland objects in it and is
//! unit-tested on its own.

use std::fs::File;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::FileExt;

use skia_rs_safe::canvas::Surface;
use wayland_client::protocol::{wl_buffer, wl_shm, wl_shm_pool};
use wayland_client::{Dispatch, QueueHandle};

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

/// Which [`BufferPool`] slot a `wl_buffer` belongs to.
///
/// This is the `wl_buffer`'s dispatch user data, so a `wl_buffer.release`
/// event names the slot to mark free without any reverse lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferSlot(pub usize);

/// How many buffers a fresh [`BufferPool`] allocates.
pub const POOL_INITIAL_BUFFERS: usize = 2;
/// The most buffers a [`BufferPool`] will ever hold.
pub const POOL_MAX_BUFFERS: usize = 3;

/// What an [`SlotPool::acquire`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// Reuse the already-allocated buffer at this index.
    Existing(usize),
    /// Allocate a buffer; it belongs at this index.
    New(usize),
}

/// Release bookkeeping for a pool of `wl_buffer`s.
///
/// Deliberately free of Wayland objects: the invariant that matters -- never
/// hand out a buffer the compositor is still holding -- is a state machine,
/// and this is it.
#[derive(Clone, Debug)]
pub struct SlotPool {
    /// Whether the compositor still holds the buffer at each index.
    busy: Vec<bool>,
    /// The most slots this pool may ever have.
    max: usize,
}

impl SlotPool {
    /// A pool of `initial` free slots that may grow to `max`.
    #[must_use]
    pub fn new(initial: usize, max: usize) -> Self {
        Self {
            busy: vec![false; initial.min(max)],
            max,
        }
    }

    /// Take a slot the compositor is not holding, growing the pool if every
    /// existing slot is busy, or `None` if it is already at `max`.
    ///
    /// The slot is marked busy: a caller that acquires is about to attach
    /// and commit, and the release that clears the flag comes back from the
    /// compositor.
    pub fn acquire(&mut self) -> Option<Slot> {
        if let Some(index) = self.busy.iter().position(|busy| !busy) {
            self.busy[index] = true;
            return Some(Slot::Existing(index));
        }
        if self.busy.len() < self.max {
            self.busy.push(true);
            return Some(Slot::New(self.busy.len() - 1));
        }
        None
    }

    /// Mark the slot free again, ignoring an index this pool never handed
    /// out (a compositor may release a buffer more than once in theory, and
    /// a stale release must not panic a client).
    pub fn release(&mut self, index: usize) {
        if let Some(busy) = self.busy.get_mut(index) {
            *busy = false;
        } else {
            tracing::debug!(index, "release for an unknown buffer slot; ignoring");
        }
    }

    /// Whether the compositor still holds the buffer at `index`.
    #[must_use]
    pub fn is_busy(&self, index: usize) -> bool {
        self.busy.get(index).copied().unwrap_or(false)
    }

    /// How many slots exist.
    #[must_use]
    pub fn len(&self) -> usize {
        self.busy.len()
    }

    /// Whether no slot exists yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.busy.is_empty()
    }

    /// How many slots the compositor is holding.
    #[must_use]
    pub fn busy_count(&self) -> usize {
        self.busy.iter().filter(|busy| **busy).count()
    }
}

/// Read one pixel out of a `wl_shm`-formatted byte buffer as `(r, g, b)`.
///
/// The one definition of `wl_shm`'s byte orders in this crate: `Xrgb8888`
/// and `Argb8888` are little-endian `0xAARRGGBB`, i.e. `B, G, R, A` in
/// memory, while `Bgr888` is `R, G, B` in memory despite the name. Anything
/// else, or an out-of-range coordinate, is `None`.
///
/// Screencopy captures come back in whichever of these the compositor
/// chose, so tests that read pixels back share this rather than each
/// carrying their own copy of the table.
#[must_use]
pub fn pixel_rgb(
    format: wl_shm::Format,
    bytes: &[u8],
    stride: u32,
    x: u32,
    y: u32,
) -> Option<(u8, u8, u8)> {
    let (bpp, order): (usize, [usize; 3]) = match format {
        wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888 => (4, [2, 1, 0]),
        wl_shm::Format::Bgr888 => (3, [0, 1, 2]),
        _ => return None,
    };
    let offset = (y as usize)
        .checked_mul(stride as usize)?
        .checked_add((x as usize).checked_mul(bpp)?)?;
    let pixel = bytes.get(offset..offset.checked_add(bpp)?)?;
    Some((pixel[order[0]], pixel[order[1]], pixel[order[2]]))
}

/// A memfd-backed `wl_shm` buffer in `Argb8888`.
pub struct ShmBuffer {
    file: File,
    pool: wl_shm_pool::WlShmPool,
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
    pub fn new<D>(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<D>,
        width: i32,
        height: i32,
        slot: BufferSlot,
    ) -> io::Result<Self>
    where
        D: Dispatch<wl_shm_pool::WlShmPool, ()>
            + Dispatch<wl_buffer::WlBuffer, BufferSlot>
            + 'static,
    {
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
        let buffer =
            pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, qh, slot);
        Ok(Self {
            file,
            pool,
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

impl Drop for ShmBuffer {
    /// Give the compositor back everything this buffer holds: the
    /// `wl_buffer`, then its `wl_shm_pool`, then the memfd (dropping `file`
    /// closes it). Without this a client that reopens its window leaks a
    /// pool and an fd per buffer for the compositor's whole lifetime.
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
    }
}

/// A double-buffered set of same-sized `wl_shm` buffers.
///
/// Generic over the dispatch state because M3 has four client states that all
/// want one of these (the M1 `LayerWindow`, `window::Window`, and each popup's
/// own pool); the M1 signatures named `AppState` outright, which no other
/// client could satisfy.
pub struct BufferPool {
    buffers: Vec<ShmBuffer>,
    slots: SlotPool,
    width: i32,
    height: i32,
}

impl BufferPool {
    /// Allocate [`POOL_INITIAL_BUFFERS`] buffers of `width` x `height`.
    ///
    /// # Errors
    ///
    /// Whatever [`ShmBuffer::new`] reports.
    pub fn new<D>(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<D>,
        width: i32,
        height: i32,
    ) -> io::Result<Self>
    where
        D: Dispatch<wl_shm_pool::WlShmPool, ()>
            + Dispatch<wl_buffer::WlBuffer, BufferSlot>
            + 'static,
    {
        let mut pool = Self {
            buffers: Vec::with_capacity(POOL_MAX_BUFFERS),
            // `SlotPool::new` already starts every slot free; growing
            // `buffers` to match keeps that invariant in one place instead
            // of reaching into the pool's private bookkeeping here too.
            slots: SlotPool::new(POOL_INITIAL_BUFFERS, POOL_MAX_BUFFERS),
            width,
            height,
        };
        for _ in 0..POOL_INITIAL_BUFFERS {
            let slot = BufferSlot(pool.buffers.len());
            pool.buffers
                .push(ShmBuffer::new(shm, qh, width, height, slot)?);
        }
        Ok(pool)
    }

    /// A buffer the compositor is not holding, allocating one if every
    /// existing buffer is busy and the pool is below [`POOL_MAX_BUFFERS`].
    ///
    /// `None` means every buffer is still held: the caller must wait for a
    /// `wl_buffer.release` rather than paint over one.
    ///
    /// # Errors
    ///
    /// Whatever [`ShmBuffer::new`] reports, when the pool has to grow.
    pub fn acquire<D>(
        &mut self,
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<D>,
    ) -> io::Result<Option<usize>>
    where
        D: Dispatch<wl_shm_pool::WlShmPool, ()>
            + Dispatch<wl_buffer::WlBuffer, BufferSlot>
            + 'static,
    {
        match self.slots.acquire() {
            Some(Slot::Existing(index)) => Ok(Some(index)),
            Some(Slot::New(index)) => {
                tracing::debug!(index, "every shm buffer was busy; growing the pool");
                let buffer = ShmBuffer::new(shm, qh, self.width, self.height, BufferSlot(index))?;
                self.buffers.push(buffer);
                debug_assert_eq!(self.buffers.len(), index + 1);
                Ok(Some(index))
            }
            None => Ok(None),
        }
    }

    /// Mark the buffer the compositor just released as reusable.
    pub fn release(&mut self, slot: BufferSlot) {
        self.slots.release(slot.0);
    }

    /// The `wl_buffer` to attach for `index`.
    ///
    /// # Panics
    ///
    /// If `index` was not handed out by [`Self::acquire`].
    #[must_use]
    pub fn wl_buffer(&self, index: usize) -> &wl_buffer::WlBuffer {
        self.buffers[index].wl_buffer()
    }

    /// Copy `surface`'s pixels into the buffer at `index`.
    ///
    /// # Errors
    ///
    /// Whatever [`ShmBuffer::upload`] reports.
    ///
    /// # Panics
    ///
    /// If `index` was not handed out by [`Self::acquire`].
    pub fn upload(&mut self, index: usize, surface: &Surface) -> io::Result<()> {
        self.buffers[index].upload(surface)
    }

    /// Buffer dimensions in pixels.
    #[must_use]
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// The release bookkeeping, for assertions and diagnostics.
    #[must_use]
    pub fn slots(&self) -> &SlotPool {
        &self.slots
    }

    /// Mutable access to the release/acquire bookkeeping.
    ///
    /// `wayland::repaint` drains its queued releases and decides whether a
    /// frame can proceed through this, rather than [`Self::acquire`],
    /// specifically because that decision needs no `shm`/`qh` and so is
    /// unit-testable without a live Wayland connection; see
    /// `wayland::select_paint_slot`.
    pub(crate) fn slots_mut(&mut self) -> &mut SlotPool {
        &mut self.slots
    }

    /// Install a buffer freshly allocated for a [`Slot::New`] decision
    /// already taken from [`Self::slots_mut`].
    ///
    /// # Panics
    ///
    /// If `index` is not the next index the pool would grow into.
    pub(crate) fn install(&mut self, index: usize, buffer: ShmBuffer) {
        debug_assert_eq!(self.buffers.len(), index, "buffer installed out of order");
        self.buffers.push(buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        POOL_INITIAL_BUFFERS, POOL_MAX_BUFFERS, Slot, SlotPool, pixel_rgb, skia_rgba_to_shm_argb,
    };
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::{Color, Rect};
    use skia_rs_safe::paint::{Paint, Style};
    use wayland_client::protocol::wl_shm::Format;

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
    fn shm_pixels_are_read_back_in_the_right_byte_order() {
        // Two rows of two pixels, with padding in the stride: reading row 1
        // proves the stride is honoured, not just the width.
        let argb = [
            0xE4u8, 0x84, 0x35, 0xFF, 0x00, 0x00, 0x00, 0xFF, 0xAA, 0xBB, 0xCC, //
            0x11, 0x22, 0x33, 0xFF, 0x44, 0x55, 0x66, 0xFF, 0x00, 0x00, 0x00,
        ];
        assert_eq!(
            pixel_rgb(Format::Argb8888, &argb, 11, 0, 0),
            Some((0x35, 0x84, 0xE4))
        );
        assert_eq!(
            pixel_rgb(Format::Xrgb8888, &argb, 11, 1, 1),
            Some((0x66, 0x55, 0x44))
        );

        // Bgr888 is R, G, B in memory, three bytes per pixel.
        let bgr = [0x35u8, 0x84, 0xE4, 0x01, 0x02, 0x03];
        assert_eq!(
            pixel_rgb(Format::Bgr888, &bgr, 6, 1, 0),
            Some((0x01, 0x02, 0x03))
        );
    }

    #[test]
    fn an_unknown_format_or_an_out_of_range_pixel_reads_none() {
        let bytes = [0u8; 16];
        assert_eq!(pixel_rgb(Format::Rgb565, &bytes, 8, 0, 0), None);
        assert_eq!(pixel_rgb(Format::Argb8888, &bytes, 8, 0, 9), None);
        assert_eq!(pixel_rgb(Format::Argb8888, &bytes, 8, 4, 0), None);
    }

    fn pool() -> SlotPool {
        SlotPool::new(POOL_INITIAL_BUFFERS, POOL_MAX_BUFFERS)
    }

    #[test]
    fn a_fresh_pool_hands_out_each_buffer_once_before_reusing_any() {
        // A3: the single buffer was rewritten while it was still attached.
        // Back-to-back frames must land in different buffers.
        let mut pool = pool();
        assert_eq!(pool.acquire(), Some(Slot::Existing(0)));
        assert_eq!(pool.acquire(), Some(Slot::Existing(1)));
        assert_eq!(pool.busy_count(), 2);
    }

    #[test]
    fn a_busy_buffer_is_never_handed_out_again() {
        let mut pool = pool();
        let first = pool.acquire().expect("first");
        for _ in 0..8 {
            let next = pool.acquire();
            assert_ne!(
                next,
                Some(first),
                "handed out a buffer the compositor is still holding"
            );
            if next.is_none() {
                break;
            }
        }
        assert!(pool.is_busy(0));
    }

    #[test]
    fn all_buffers_busy_grows_the_pool_once_and_then_refuses() {
        let mut pool = pool();
        assert_eq!(pool.acquire(), Some(Slot::Existing(0)));
        assert_eq!(pool.acquire(), Some(Slot::Existing(1)));
        assert_eq!(
            pool.acquire(),
            Some(Slot::New(2)),
            "both buffers busy should allocate a third"
        );
        assert_eq!(pool.len(), POOL_MAX_BUFFERS);
        assert_eq!(
            pool.acquire(),
            None,
            "the pool must refuse rather than reuse a held buffer"
        );
    }

    #[test]
    fn a_released_buffer_becomes_reusable() {
        let mut pool = pool();
        pool.acquire().expect("first");
        pool.acquire().expect("second");
        assert_eq!(pool.acquire(), Some(Slot::New(2)));
        assert_eq!(pool.acquire(), None);

        pool.release(1);
        assert!(!pool.is_busy(1));
        assert_eq!(pool.acquire(), Some(Slot::Existing(1)));
        assert_eq!(pool.busy_count(), 3);
    }

    #[test]
    fn a_release_for_an_unknown_slot_is_ignored() {
        let mut pool = pool();
        pool.release(99);
        assert_eq!(pool.busy_count(), 0);
        assert!(!pool.is_busy(99));
    }

    #[test]
    fn a_short_destination_is_filled_as_far_as_it_goes() {
        let src = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut dst = [0u8; 4];
        skia_rgba_to_shm_argb(&src, &mut dst);
        assert_eq!(dst, [3, 2, 1, 4]);
    }
}
