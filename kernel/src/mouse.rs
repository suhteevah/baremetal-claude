//! USB HID Mouse Support
//!
//! Provides mouse state tracking, boot protocol report parsing, cursor
//! rendering on the GOP framebuffer, and a mouse event system. The mouse
//! uses the USB HID boot protocol (3-4 byte reports) and renders an XOR'd
//! crosshair cursor for visibility on any background.
//!
//! ## Boot Protocol Mouse Report (3-4 bytes)
//!
//! - Byte 0: Button bitmap (bit 0 = left, bit 1 = right, bit 2 = middle)
//! - Byte 1: X displacement (signed i8, positive = right)
//! - Byte 2: Y displacement (signed i8, positive = down)
//! - Byte 3: Scroll wheel (signed i8, optional — positive = scroll up)
//!
//! ## Integration
//!
//! The xHCI crate currently only exposes keyboard-specific enumeration and
//! polling. To fully wire up mouse support, `XhciController` needs:
//!
//! 1. `find_hid_mouse()` on `ParsedConfiguration` (class=3, subclass=1, protocol=2)
//! 2. `setup_mouse()` analogous to `setup_keyboard()` (SET_PROTOCOL Boot, SET_IDLE)
//! 3. `poll_mouse()` returning raw report bytes
//!
//! Until then, `poll_usb_mouse()` in `usb.rs` is a no-op stub, and this module
//! can be exercised via `process_report()` for testing or future integration.

extern crate alloc;

use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Mouse buttons
// ---------------------------------------------------------------------------

/// Mouse button identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Button bits in boot protocol report byte 0.
const BTN_LEFT: u8 = 1 << 0;
const BTN_RIGHT: u8 = 1 << 1;
const BTN_MIDDLE: u8 = 1 << 2;

// ---------------------------------------------------------------------------
// Mouse events
// ---------------------------------------------------------------------------

/// Events produced by the mouse driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEvent {
    /// Cursor moved to absolute position (x, y).
    Move(i32, i32),
    /// A button was pressed.
    ButtonDown(MouseButton),
    /// A button was released.
    ButtonUp(MouseButton),
    /// Scroll wheel moved (positive = up, negative = down).
    Scroll(i8),
}

// ---------------------------------------------------------------------------
// Mouse state
// ---------------------------------------------------------------------------

/// Current mouse state: position, buttons, and screen bounds.
#[derive(Debug)]
pub struct MouseState {
    /// Current X position (pixels from left).
    pub x: i32,
    /// Current Y position (pixels from top).
    pub y: i32,
    /// Left button held.
    pub left: bool,
    /// Right button held.
    pub right: bool,
    /// Middle button held.
    pub middle: bool,
    /// Accumulated scroll delta (cleared on read).
    pub scroll: i8,
    /// Screen width bound.
    pub screen_w: i32,
    /// Screen height bound.
    pub screen_h: i32,
    /// Previous button state for edge detection.
    prev_buttons: u8,
    /// Event queue.
    event_queue: VecDeque<MouseEvent>,
}

impl MouseState {
    /// Create a new mouse state with the given screen bounds.
    pub fn new(screen_w: i32, screen_h: i32) -> Self {
        log::debug!(
            "[mouse] state initialized: screen={}x{}",
            screen_w, screen_h
        );
        Self {
            x: screen_w / 2,
            y: screen_h / 2,
            left: false,
            right: false,
            middle: false,
            scroll: 0,
            screen_w,
            screen_h,
            prev_buttons: 0,
            event_queue: VecDeque::new(),
        }
    }

    /// Process a USB HID boot protocol mouse report (3 or 4 bytes).
    ///
    /// Generates `MouseEvent`s for movement, button changes, and scroll.
    pub fn process_report(&mut self, report: &[u8]) {
        if report.len() < 3 {
            log::warn!(
                "[mouse] report too short ({} bytes, need >= 3)",
                report.len()
            );
            return;
        }

        let buttons = report[0];
        let dx = report[1] as i8;
        let dy = report[2] as i8;
        let scroll = if report.len() >= 4 {
            report[3] as i8
        } else {
            0
        };

        log::trace!(
            "[mouse] report: buttons={:#04x} dx={} dy={} scroll={}",
            buttons, dx, dy, scroll
        );

        // --- Movement ---
        if dx != 0 || dy != 0 {
            self.x = (self.x + dx as i32).clamp(0, self.screen_w - 1);
            self.y = (self.y + dy as i32).clamp(0, self.screen_h - 1);
            self.event_queue.push_back(MouseEvent::Move(self.x, self.y));
        }

        // --- Button edge detection ---
        let changed = buttons ^ self.prev_buttons;

        if changed & BTN_LEFT != 0 {
            self.left = buttons & BTN_LEFT != 0;
            self.event_queue.push_back(if self.left {
                MouseEvent::ButtonDown(MouseButton::Left)
            } else {
                MouseEvent::ButtonUp(MouseButton::Left)
            });
        }

        if changed & BTN_RIGHT != 0 {
            self.right = buttons & BTN_RIGHT != 0;
            self.event_queue.push_back(if self.right {
                MouseEvent::ButtonDown(MouseButton::Right)
            } else {
                MouseEvent::ButtonUp(MouseButton::Right)
            });
        }

        if changed & BTN_MIDDLE != 0 {
            self.middle = buttons & BTN_MIDDLE != 0;
            self.event_queue.push_back(if self.middle {
                MouseEvent::ButtonDown(MouseButton::Middle)
            } else {
                MouseEvent::ButtonUp(MouseButton::Middle)
            });
        }

        self.prev_buttons = buttons;

        // --- Scroll ---
        if scroll != 0 {
            self.scroll = scroll;
            self.event_queue.push_back(MouseEvent::Scroll(scroll));
        }
    }

    /// Dequeue the next mouse event, if any.
    pub fn next_event(&mut self) -> Option<MouseEvent> {
        self.event_queue.pop_front()
    }

    /// Check if there are pending mouse events.
    pub fn has_events(&self) -> bool {
        !self.event_queue.is_empty()
    }

    /// Update screen bounds (e.g. on resolution change).
    pub fn set_bounds(&mut self, w: i32, h: i32) {
        self.screen_w = w;
        self.screen_h = h;
        self.x = self.x.clamp(0, w - 1);
        self.y = self.y.clamp(0, h - 1);
    }
}

// ---------------------------------------------------------------------------
// Global mouse state
// ---------------------------------------------------------------------------

/// Global mouse state, protected by a spinlock.
static MOUSE: Mutex<Option<MouseState>> = Mutex::new(None);

/// Whether a USB mouse was detected during enumeration.
static USB_MOUSE_PRESENT: AtomicBool = AtomicBool::new(false);

/// Whether the cursor is currently visible.
static CURSOR_VISIBLE: AtomicBool = AtomicBool::new(false);

/// Cached cursor position for the renderer (avoids locking MOUSE during draw).
static CURSOR_X: AtomicI32 = AtomicI32::new(0);
static CURSOR_Y: AtomicI32 = AtomicI32::new(0);

/// Initialize the global mouse state.
///
/// Call this after the framebuffer is initialized so we know the screen bounds.
pub fn init() {
    let w = crate::framebuffer::width() as i32;
    let h = crate::framebuffer::height() as i32;

    if w == 0 || h == 0 {
        log::warn!("[mouse] framebuffer not available, mouse disabled");
        return;
    }

    let state = MouseState::new(w, h);
    *MOUSE.lock() = Some(state);

    log::info!("[mouse] initialized: screen={}x{}", w, h);
}

/// Mark that a USB mouse was detected.
pub fn set_mouse_present(present: bool) {
    USB_MOUSE_PRESENT.store(present, Ordering::Relaxed);
    if present {
        log::info!("[mouse] USB HID mouse detected");
    }
}

/// Check if a USB mouse is present.
pub fn is_present() -> bool {
    USB_MOUSE_PRESENT.load(Ordering::Relaxed)
}

/// Feed a raw USB HID boot protocol report into the mouse state machine.
///
/// Called from `usb::poll_usb_mouse()` when report data arrives from xHCI.
pub fn feed_report(report: &[u8]) {
    let mut guard = MOUSE.lock();
    if let Some(ref mut state) = *guard {
        state.process_report(report);

        // Update cached cursor position for rendering
        CURSOR_X.store(state.x, Ordering::Relaxed);
        CURSOR_Y.store(state.y, Ordering::Relaxed);
    }
}

/// Drain all pending mouse events, calling the provided callback for each.
pub fn drain_events(mut callback: impl FnMut(MouseEvent)) {
    let mut guard = MOUSE.lock();
    if let Some(ref mut state) = *guard {
        while let Some(evt) = state.next_event() {
            callback(evt);
        }
    }
}

/// Get the current mouse position.
pub fn position() -> (i32, i32) {
    (
        CURSOR_X.load(Ordering::Relaxed),
        CURSOR_Y.load(Ordering::Relaxed),
    )
}

/// Get the current button state (left, right, middle).
pub fn buttons() -> (bool, bool, bool) {
    let guard = MOUSE.lock();
    match *guard {
        Some(ref state) => (state.left, state.right, state.middle),
        None => (false, false, false),
    }
}

// ---------------------------------------------------------------------------
// Cursor rendering — XOR crosshair
// ---------------------------------------------------------------------------

/// Crosshair cursor size (arm length from center, in pixels).
const CURSOR_ARM: i32 = 6;

/// Show the mouse cursor on the framebuffer.
pub fn show_cursor() {
    CURSOR_VISIBLE.store(true, Ordering::Relaxed);
    draw_cursor();
    log::debug!("[mouse] cursor shown");
}

/// Hide the mouse cursor.
pub fn hide_cursor() {
    if CURSOR_VISIBLE.load(Ordering::Relaxed) {
        // Erase cursor by drawing it again (XOR is self-inverse)
        draw_cursor();
        CURSOR_VISIBLE.store(false, Ordering::Relaxed);
        log::debug!("[mouse] cursor hidden");
    }
}

/// Erase the cursor at its current position, update position, redraw.
///
/// Call this after `feed_report()` to animate the cursor.
pub fn update_cursor() {
    if !CURSOR_VISIBLE.load(Ordering::Relaxed) {
        return;
    }

    // The cursor is drawn with XOR, so drawing it again erases it.
    // We rely on the caller having already called `draw_cursor()` to show it,
    // so we erase, then redraw at the new position.
    //
    // Note: since we draw into the back buffer and blit happens separately,
    // we just need to XOR the pixels at old and new positions.
    draw_crosshair(
        CURSOR_X.load(Ordering::Relaxed),
        CURSOR_Y.load(Ordering::Relaxed),
    );
}

/// Draw (or erase via XOR) the crosshair cursor at the current cached position.
fn draw_cursor() {
    let cx = CURSOR_X.load(Ordering::Relaxed);
    let cy = CURSOR_Y.load(Ordering::Relaxed);
    draw_crosshair(cx, cy);
}

/// Draw an XOR crosshair at (cx, cy).
///
/// The crosshair consists of a horizontal and vertical line, each `CURSOR_ARM`
/// pixels long from center. The center pixel is drawn once (intersection).
/// Each pixel is XOR'd with 0xFFFFFF so the cursor is visible on any background.
fn draw_crosshair(cx: i32, cy: i32) {
    let w = crate::framebuffer::width() as i32;
    let h = crate::framebuffer::height() as i32;
    let stride = crate::framebuffer::stride();
    let bpp = crate::framebuffer::bytes_per_pixel();

    if w == 0 || h == 0 {
        return;
    }

    // We need direct back-buffer access for XOR. Use put_pixel_xor helper.
    // Horizontal arm
    for dx in -CURSOR_ARM..=CURSOR_ARM {
        let px = cx + dx;
        if px >= 0 && px < w && cy >= 0 && cy < h {
            xor_pixel(px as usize, cy as usize, stride, bpp);
        }
    }

    // Vertical arm (skip center to avoid double-XOR)
    for dy in -CURSOR_ARM..=CURSOR_ARM {
        if dy == 0 {
            continue; // Already drawn in horizontal pass
        }
        let py = cy + dy;
        if cx >= 0 && cx < w && py >= 0 && py < h {
            xor_pixel(cx as usize, py as usize, stride, bpp);
        }
    }
}

/// XOR a single pixel in the framebuffer back buffer with white (0xFFFFFF).
///
/// This gives maximum visibility: white pixels become black, black become white,
/// and colored pixels become their inverse.
fn xor_pixel(x: usize, y: usize, stride: usize, bpp: usize) {
    // Access the framebuffer back buffer directly via the FB lock.
    // This is safe because we only XOR within validated bounds.
    crate::framebuffer::xor_pixel_backbuf(x, y, stride, bpp);
}

// ---------------------------------------------------------------------------
// Stub: USB HID mouse detection constants
// ---------------------------------------------------------------------------

/// USB HID Boot Interface Subclass.
pub const HID_SUBCLASS_BOOT: u8 = 1;

/// USB HID Boot Protocol: Mouse.
pub const HID_PROTOCOL_MOUSE: u8 = 2;
