//! GOP framebuffer management — pixel drawing, viewport clipping.
//!
//! The framebuffer is our only display output. Terminal panes are viewports
//! into this buffer. The panic handler can force-render here.

use bootloader_api::info::FrameBuffer;
use spin::Mutex;

static FB: Mutex<Option<FrameBufferState>> = Mutex::new(None);

pub struct FrameBufferState {
    pub buffer: &'static mut [u8],
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
}

pub fn init(fb: &'static mut FrameBuffer) {
    let info = fb.info();
    let state = FrameBufferState {
        buffer: fb.buffer_mut(),
        width: info.width,
        height: info.height,
        stride: info.stride,
        bytes_per_pixel: info.bytes_per_pixel,
    };

    // NOTE: Framebuffer clearing is deferred — the bootloader's virtual address
    // for the FB buffer (0x20000000000) may require page table fixup before writes
    // work. For now, skip the clear; the screen starts with whatever the bootloader left.
    // TODO: Fix framebuffer page table mapping in Phase 2
    log::info!("[fb] buffer at {:p}, {} bytes (clear deferred)", state.buffer.as_ptr(), state.buffer.len());

    *FB.lock() = Some(state);
    log::info!("[fb] framebuffer ready");
}

/// Draw a single pixel. Used by terminal renderer.
#[inline]
pub fn put_pixel(x: usize, y: usize, r: u8, g: u8, b: u8) {
    // TODO: lock-free fast path using atomic framebuffer access
    if let Some(ref mut fb) = *FB.lock() {
        if x >= fb.width || y >= fb.height {
            return;
        }
        let offset = (y * fb.stride + x) * fb.bytes_per_pixel;
        // Assume BGR pixel format (common for UEFI GOP)
        fb.buffer[offset] = b;
        fb.buffer[offset + 1] = g;
        fb.buffer[offset + 2] = r;
    }
}
