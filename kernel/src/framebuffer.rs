//! GOP framebuffer management — double-buffered, memory-mapped pixel access.
//!
//! ## Architecture (TempleOS-inspired)
//!
//! The framebuffer is our only display output. Rather than calling `put_pixel`
//! for every pixel (which acquires a mutex lock 1M+ times per frame), we use:
//!
//! 1. **Back buffer** — a heap-allocated `Vec<u8>` the same size as the
//!    hardware framebuffer. All rendering happens here, lock-free.
//! 2. **Front buffer** — the actual hardware framebuffer, memory-mapped via
//!    the physical memory offset. We blast the back buffer here in one
//!    `copy_nonoverlapping` call ("page flip").
//! 3. **Dirty region tracking** — the dashboard tells us which pixel rows
//!    changed, so we only copy those rows to the front buffer.
//!
//! ## Framebuffer address mapping
//!
//! Limine already maps the framebuffer into the higher-half direct map (HHDM).
//! The `addr()` pointer returned by the Limine framebuffer struct is a valid
//! writable virtual address — no extra page-table walking required (unlike
//! the `bootloader` 0.11 crate, which mapped the framebuffer at a separate
//! address that could lack the WRITABLE flag).

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

static FB: Mutex<Option<FrameBufferState>> = Mutex::new(None);

/// Thin adapter struct so the rest of the kernel doesn't have to know about
/// the Limine framebuffer type directly. Populated in `init()` from the
/// `limine::framebuffer::Framebuffer` returned by the bootloader.
pub struct LimineFramebufferInfo {
    /// Pointer to the first byte of the framebuffer.
    pub addr: *mut u8,
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Pitch in bytes (distance between two rows — may exceed `width*bpp/8`).
    pub pitch: usize,
    /// Bits per pixel (typically 32 for BGR/RGB8888).
    pub bpp: u16,
}

/// Core state for the double-buffered framebuffer.
pub struct FrameBufferState {
    /// Hardware framebuffer (front buffer) — memory-mapped via the Limine
    /// HHDM.  Writes to this are immediately visible on screen.
    pub front: &'static mut [u8],
    /// Off-screen back buffer — heap-allocated.  All rendering targets this.
    pub back: Vec<u8>,
    /// Display width in pixels.
    pub width: usize,
    /// Display height in pixels.
    pub height: usize,
    /// Stride in pixels (may be > width due to GPU alignment requirements).
    pub stride: usize,
    /// Bytes per pixel (typically 4 for BGR32 UEFI GOP format).
    pub bytes_per_pixel: usize,
}

/// Initialize the framebuffer state from a Limine framebuffer description.
///
/// `info.addr` must be a valid writable pointer to `info.pitch * info.height`
/// bytes for the lifetime of the kernel. Limine maps it into the HHDM, so this
/// precondition is satisfied as long as the bootloader's page tables remain in
/// effect (which they do for the entire ClaudioOS lifetime).
pub fn init(info: LimineFramebufferInfo) {
    let bytes_per_pixel = ((info.bpp as usize) + 7) / 8;
    let buf_len = info.pitch * info.height;
    let stride_pixels = if bytes_per_pixel > 0 {
        info.pitch / bytes_per_pixel
    } else {
        info.width
    };

    log::info!(
        "[fb] limine framebuffer: addr={:p} {}x{} pitch={} bpp={}",
        info.addr,
        info.width,
        info.height,
        info.pitch,
        info.bpp,
    );

    // SAFETY: the caller guarantees the pointer is valid for `buf_len` bytes
    // and remains mapped + writable for the kernel's lifetime.
    let front = unsafe {
        core::slice::from_raw_parts_mut(info.addr, buf_len)
    };

    // Allocate the back buffer on the heap — same size as the front buffer.
    let back = vec![0u8; buf_len];

    log::info!(
        "[fb] double buffer allocated: {} bytes back buffer + {} bytes front buffer",
        buf_len,
        buf_len
    );

    let state = FrameBufferState {
        front,
        back,
        width: info.width,
        height: info.height,
        stride: stride_pixels,
        bytes_per_pixel,
    };

    // Intentionally NOT clearing the front buffer. Phase -2's RGB proof-of-
    // life bars must stay visible until a higher layer (splash, vconsole)
    // draws real content over them. A black-after-bars transition on real
    // hardware is indistinguishable from "kernel died silently."
    log::info!(
        "[fb] front buffer ready — {} bytes visible, proof-of-life bars preserved",
        buf_len,
    );

    *FB.lock() = Some(state);
    log::info!(
        "[fb] framebuffer ready: {}x{} stride={} bpp={} double-buffered",
        info.width,
        info.height,
        stride_pixels,
        bytes_per_pixel
    );
}

/// Return the framebuffer width in pixels, or 0 if not initialised.
pub fn width() -> usize {
    FB.lock().as_ref().map_or(0, |fb| fb.width)
}

/// Return the framebuffer height in pixels, or 0 if not initialised.
pub fn height() -> usize {
    FB.lock().as_ref().map_or(0, |fb| fb.height)
}

/// Return stride in pixels.
pub fn stride() -> usize {
    FB.lock().as_ref().map_or(0, |fb| fb.stride)
}

/// Return bytes per pixel.
pub fn bytes_per_pixel() -> usize {
    FB.lock().as_ref().map_or(4, |fb| fb.bytes_per_pixel)
}

/// Draw a single pixel. Legacy API — still used by the panic handler.
/// For normal rendering, use the back-buffer `DrawTarget` instead.
#[inline]
pub fn put_pixel(x: usize, y: usize, r: u8, g: u8, b: u8) {
    if let Some(ref mut fb) = *FB.lock() {
        if x >= fb.width || y >= fb.height {
            return;
        }
        let offset = (y * fb.stride + x) * fb.bytes_per_pixel;
        if offset + 2 < fb.back.len() {
            fb.back[offset] = b;
            fb.back[offset + 1] = g;
            fb.back[offset + 2] = r;
        }
    }
}

/// Return (width, height) in pixels, or (0, 0) if not initialised.
pub fn dimensions() -> (usize, usize) {
    FB.lock().as_ref().map_or((0, 0), |fb| (fb.width, fb.height))
}

/// Fill an axis-aligned rectangle on the back buffer with a solid color. Takes
/// the framebuffer mutex once per call, so it's dramatically faster than a
/// `put_pixel` loop for large areas (as used by the QR renderer). Clips to
/// framebuffer bounds.
pub fn fill_rect(x: usize, y: usize, w: usize, h: usize, (r, g, b): (u8, u8, u8)) {
    if let Some(ref mut fb) = *FB.lock() {
        let x_end = (x + w).min(fb.width);
        let y_end = (y + h).min(fb.height);
        if x >= fb.width || y >= fb.height {
            return;
        }
        let bpp = fb.bytes_per_pixel;
        let stride_bytes = fb.stride * bpp;
        for yy in y..y_end {
            let row_off = yy * stride_bytes;
            for xx in x..x_end {
                let off = row_off + xx * bpp;
                if off + 2 < fb.back.len() {
                    fb.back[off] = b;
                    fb.back[off + 1] = g;
                    fb.back[off + 2] = r;
                }
            }
        }
    }
}

/// Blit the entire back buffer to the front (hardware) buffer.
pub fn blit_full() {
    if let Some(ref mut fb) = *FB.lock() {
        let len = fb.back.len().min(fb.front.len());
        unsafe {
            core::ptr::copy_nonoverlapping(
                fb.back.as_ptr(),
                fb.front.as_mut_ptr(),
                len,
            );
        }
        log::trace!("[fb] blit_full: {} bytes copied to front buffer", len);
    }
}

/// Blit only the specified pixel rows from the back buffer to the front buffer.
pub fn blit_rows(y_start: usize, y_end: usize) {
    if let Some(ref mut fb) = *FB.lock() {
        let y_start = y_start.min(fb.height);
        let y_end = y_end.min(fb.height);
        if y_start >= y_end {
            return;
        }
        let bpp = fb.bytes_per_pixel;
        let row_bytes = fb.stride * bpp;
        let start = y_start * row_bytes;
        let end = y_end * row_bytes;
        let len = (end - start).min(fb.back.len() - start).min(fb.front.len() - start);
        unsafe {
            core::ptr::copy_nonoverlapping(
                fb.back.as_ptr().add(start),
                fb.front.as_mut_ptr().add(start),
                len,
            );
        }
        log::trace!(
            "[fb] blit_rows: y={}..{} ({} bytes)",
            y_start,
            y_end,
            len
        );
    }
}

/// Acquire the back buffer for direct rendering.
pub fn with_back_buffer<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut [u8], usize, usize, usize, usize) -> R,
{
    if let Some(ref mut fb) = *FB.lock() {
        Some(f(
            &mut fb.back,
            fb.width,
            fb.height,
            fb.stride,
            fb.bytes_per_pixel,
        ))
    } else {
        None
    }
}

/// XOR a single pixel in the back buffer with white (0xFFFFFF).
#[inline]
pub fn xor_pixel_backbuf(x: usize, y: usize, stride: usize, bpp: usize) {
    if let Some(ref mut fb) = *FB.lock() {
        let offset = (y * stride + x) * bpp;
        if offset + 2 < fb.back.len() {
            fb.back[offset] ^= 0xFF;
            fb.back[offset + 1] ^= 0xFF;
            fb.back[offset + 2] ^= 0xFF;
        }
    }
}
