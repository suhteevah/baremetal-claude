//! HID (Human Interface Device) Keyboard Support
//!
//! Implements the USB HID boot protocol for keyboards. The boot protocol is a
//! simplified 8-byte report format that does not require parsing HID report
//! descriptors, making it ideal for early-boot keyboard support.
//!
//! Boot protocol keyboard report (8 bytes):
//! - Byte 0: Modifier keys (bitmap)
//! - Byte 1: Reserved (always 0)
//! - Bytes 2-7: Up to 6 simultaneous keycodes (USB HID Usage IDs)

use alloc::collections::VecDeque;

// ---------------------------------------------------------------------------
// HID Class-specific requests
// ---------------------------------------------------------------------------

/// HID class request: GET_REPORT
pub const HID_REQ_GET_REPORT: u8 = 0x01;
/// HID class request: GET_IDLE
pub const HID_REQ_GET_IDLE: u8 = 0x02;
/// HID class request: GET_PROTOCOL
pub const HID_REQ_GET_PROTOCOL: u8 = 0x03;
/// HID class request: SET_REPORT
pub const HID_REQ_SET_REPORT: u8 = 0x09;
/// HID class request: SET_IDLE
pub const HID_REQ_SET_IDLE: u8 = 0x0A;
/// HID class request: SET_PROTOCOL
pub const HID_REQ_SET_PROTOCOL: u8 = 0x0B;

/// Protocol values for SET_PROTOCOL
pub const HID_PROTOCOL_BOOT: u16 = 0;
pub const HID_PROTOCOL_REPORT: u16 = 1;

// ---------------------------------------------------------------------------
// Modifier key bits (byte 0 of boot protocol report)
// ---------------------------------------------------------------------------

pub const MOD_LEFT_CTRL: u8 = 1 << 0;
pub const MOD_LEFT_SHIFT: u8 = 1 << 1;
pub const MOD_LEFT_ALT: u8 = 1 << 2;
pub const MOD_LEFT_GUI: u8 = 1 << 3;
pub const MOD_RIGHT_CTRL: u8 = 1 << 4;
pub const MOD_RIGHT_SHIFT: u8 = 1 << 5;
pub const MOD_RIGHT_ALT: u8 = 1 << 6;
pub const MOD_RIGHT_GUI: u8 = 1 << 7;

// ---------------------------------------------------------------------------
// USB HID Usage ID to PS/2-style scancode mapping
// ---------------------------------------------------------------------------

/// A keyboard event produced by parsing a HID boot protocol report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// The USB HID Usage ID (keycode)
    pub usage_id: u8,
    /// PS/2 scancode equivalent (for compatibility with pc-keyboard crate)
    pub scancode: u8,
    /// true = key pressed, false = key released
    pub pressed: bool,
    /// Modifier state at the time of the event
    pub modifiers: u8,
}

/// USB HID Usage ID to PS/2 Set 1 scancode lookup table.
/// Index = USB HID Usage ID, Value = PS/2 Make code (0 = unmapped).
///
/// Based on USB HID Usage Tables 1.4, Section 10 "Keyboard/Keypad Page (0x07)"
/// and PS/2 Scan Code Set 1.
static HID_TO_SCANCODE: [u8; 128] = [
    // 0x00-0x03: No Event, Error Roll Over, POST Fail, Error Undefined
    0x00, 0x00, 0x00, 0x00,
    // 0x04: A, 0x05: B, 0x06: C, 0x07: D
    0x1E, 0x30, 0x2E, 0x20,
    // 0x08: E, 0x09: F, 0x0A: G, 0x0B: H
    0x12, 0x21, 0x22, 0x23,
    // 0x0C: I, 0x0D: J, 0x0E: K, 0x0F: L
    0x17, 0x24, 0x25, 0x26,
    // 0x10: M, 0x11: N, 0x12: O, 0x13: P
    0x32, 0x31, 0x18, 0x19,
    // 0x14: Q, 0x15: R, 0x16: S, 0x17: T
    0x10, 0x13, 0x1F, 0x14,
    // 0x18: U, 0x19: V, 0x1A: W, 0x1B: X
    0x16, 0x2F, 0x11, 0x2D,
    // 0x1C: Y, 0x1D: Z, 0x1E: 1, 0x1F: 2
    0x15, 0x2C, 0x02, 0x03,
    // 0x20: 3, 0x21: 4, 0x22: 5, 0x23: 6
    0x04, 0x05, 0x06, 0x07,
    // 0x24: 7, 0x25: 8, 0x26: 9, 0x27: 0
    0x08, 0x09, 0x0A, 0x0B,
    // 0x28: Enter, 0x29: Escape, 0x2A: Backspace, 0x2B: Tab
    0x1C, 0x01, 0x0E, 0x0F,
    // 0x2C: Space, 0x2D: Minus, 0x2E: Equal, 0x2F: Left Bracket
    0x39, 0x0C, 0x0D, 0x1A,
    // 0x30: Right Bracket, 0x31: Backslash, 0x32: Non-US #, 0x33: Semicolon
    0x1B, 0x2B, 0x2B, 0x27,
    // 0x34: Apostrophe, 0x35: Grave Accent, 0x36: Comma, 0x37: Period
    0x28, 0x29, 0x33, 0x34,
    // 0x38: Slash, 0x39: Caps Lock, 0x3A: F1, 0x3B: F2
    0x35, 0x3A, 0x3B, 0x3C,
    // 0x3C: F3, 0x3D: F4, 0x3E: F5, 0x3F: F6
    0x3D, 0x3E, 0x3F, 0x40,
    // 0x40: F7, 0x41: F8, 0x42: F9, 0x43: F10
    0x41, 0x42, 0x43, 0x44,
    // 0x44: F11, 0x45: F12, 0x46: Print Screen, 0x47: Scroll Lock
    0x57, 0x58, 0x00, 0x46,
    // 0x48: Pause, 0x49: Insert, 0x4A: Home, 0x4B: Page Up
    0x00, 0x52, 0x47, 0x49,
    // 0x4C: Delete, 0x4D: End, 0x4E: Page Down, 0x4F: Right Arrow
    0x53, 0x4F, 0x51, 0x4D,
    // 0x50: Left Arrow, 0x51: Down Arrow, 0x52: Up Arrow, 0x53: Num Lock
    0x4B, 0x50, 0x48, 0x45,
    // 0x54: KP /, 0x55: KP *, 0x56: KP -, 0x57: KP +
    0x35, 0x37, 0x4A, 0x4E,
    // 0x58: KP Enter, 0x59: KP 1, 0x5A: KP 2, 0x5B: KP 3
    0x1C, 0x4F, 0x50, 0x51,
    // 0x5C: KP 4, 0x5D: KP 5, 0x5E: KP 6, 0x5F: KP 7
    0x4B, 0x4C, 0x4D, 0x47,
    // 0x60: KP 8, 0x61: KP 9, 0x62: KP 0, 0x63: KP .
    0x48, 0x49, 0x52, 0x53,
    // 0x64: Non-US \, 0x65: Application, 0x66: Power, 0x67: KP =
    0x56, 0x00, 0x00, 0x00,
    // 0x68-0x6B: F13-F16
    0x00, 0x00, 0x00, 0x00,
    // 0x6C-0x6F: F17-F20
    0x00, 0x00, 0x00, 0x00,
    // 0x70-0x73: F21-F24
    0x00, 0x00, 0x00, 0x00,
    // 0x74-0x77: Execute, Help, Menu, Select
    0x00, 0x00, 0x00, 0x00,
    // 0x78-0x7B: Stop, Again, Undo, Cut
    0x00, 0x00, 0x00, 0x00,
    // 0x7C-0x7F: Copy, Paste, Find, Mute
    0x00, 0x00, 0x00, 0x00,
];

/// Convert a USB HID Usage ID to a PS/2 scancode.
pub fn hid_usage_to_scancode(usage_id: u8) -> u8 {
    if (usage_id as usize) < HID_TO_SCANCODE.len() {
        HID_TO_SCANCODE[usage_id as usize]
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Boot protocol report parser
// ---------------------------------------------------------------------------

/// Boot protocol keyboard report: 8 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootKeyboardReport {
    /// Modifier keys bitmap
    pub modifiers: u8,
    /// Reserved byte (should be 0)
    pub reserved: u8,
    /// Up to 6 keycodes currently pressed
    pub keycodes: [u8; 6],
}

impl BootKeyboardReport {
    /// Parse from an 8-byte buffer.
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < 8 {
            log::warn!("xhci: HID report too short ({} bytes, need 8)", buf.len());
            return None;
        }

        Some(Self {
            modifiers: buf[0],
            reserved: buf[1],
            keycodes: [buf[2], buf[3], buf[4], buf[5], buf[6], buf[7]],
        })
    }

    /// Check for phantom/rollover condition (all keycodes = 0x01)
    pub fn is_rollover(&self) -> bool {
        self.keycodes.iter().all(|&k| k == 0x01)
    }

    /// Check if a specific keycode is present in this report
    pub fn has_keycode(&self, code: u8) -> bool {
        self.keycodes.iter().any(|&k| k == code)
    }
}

// ---------------------------------------------------------------------------
// Keyboard state tracker
// ---------------------------------------------------------------------------

/// Tracks keyboard state across reports and generates press/release events
/// by diffing consecutive reports.
pub struct KeyboardState {
    /// Previous report (for diffing)
    prev_report: BootKeyboardReport,
    /// Queue of events to be consumed by the kernel
    event_queue: VecDeque<KeyEvent>,
}

impl KeyboardState {
    /// Create a new keyboard state tracker.
    pub fn new() -> Self {
        log::debug!("xhci: keyboard state tracker initialized");
        Self {
            prev_report: BootKeyboardReport {
                modifiers: 0,
                reserved: 0,
                keycodes: [0; 6],
            },
            event_queue: VecDeque::new(),
        }
    }

    /// Process a new HID boot protocol report and generate key events.
    pub fn process_report(&mut self, report: &BootKeyboardReport) {
        if report.is_rollover() {
            log::trace!("xhci: keyboard rollover detected, ignoring report");
            return;
        }

        // Detect modifier changes
        self.process_modifier_changes(report.modifiers);

        // Detect released keys (in prev but not in new)
        for &prev_key in &self.prev_report.keycodes {
            if prev_key != 0 && !report.has_keycode(prev_key) {
                let scancode = hid_usage_to_scancode(prev_key);
                log::trace!(
                    "xhci: key released: usage={:#x} scancode={:#x}",
                    prev_key, scancode
                );
                self.event_queue.push_back(KeyEvent {
                    usage_id: prev_key,
                    scancode,
                    pressed: false,
                    modifiers: report.modifiers,
                });
            }
        }

        // Detect pressed keys (in new but not in prev)
        for &new_key in &report.keycodes {
            if new_key != 0 && !self.prev_report.has_keycode(new_key) {
                let scancode = hid_usage_to_scancode(new_key);
                log::trace!(
                    "xhci: key pressed: usage={:#x} scancode={:#x}",
                    new_key, scancode
                );
                self.event_queue.push_back(KeyEvent {
                    usage_id: new_key,
                    scancode,
                    pressed: true,
                    modifiers: report.modifiers,
                });
            }
        }

        self.prev_report = *report;
    }

    /// Process modifier key changes as individual events.
    fn process_modifier_changes(&mut self, new_mods: u8) {
        let old_mods = self.prev_report.modifiers;
        let changed = old_mods ^ new_mods;

        if changed == 0 {
            return;
        }

        // Map each modifier bit to a USB HID usage ID
        // Left Ctrl=0xE0, Left Shift=0xE1, Left Alt=0xE2, Left GUI=0xE3
        // Right Ctrl=0xE4, Right Shift=0xE5, Right Alt=0xE6, Right GUI=0xE7
        let mod_bits = [
            (MOD_LEFT_CTRL, 0xE0u8),
            (MOD_LEFT_SHIFT, 0xE1),
            (MOD_LEFT_ALT, 0xE2),
            (MOD_LEFT_GUI, 0xE3),
            (MOD_RIGHT_CTRL, 0xE4),
            (MOD_RIGHT_SHIFT, 0xE5),
            (MOD_RIGHT_ALT, 0xE6),
            (MOD_RIGHT_GUI, 0xE7),
        ];

        // PS/2 scancodes for modifier keys
        let mod_scancodes: [u8; 8] = [
            0x1D, // Left Ctrl
            0x2A, // Left Shift
            0x38, // Left Alt
            0x00, // Left GUI (no PS/2 equivalent in set 1 base)
            0x1D, // Right Ctrl (extended)
            0x36, // Right Shift
            0x38, // Right Alt (extended)
            0x00, // Right GUI
        ];

        for (i, &(bit, usage)) in mod_bits.iter().enumerate() {
            if changed & bit != 0 {
                let pressed = new_mods & bit != 0;
                let scancode = mod_scancodes[i];
                log::trace!(
                    "xhci: modifier {}: usage={:#x} scancode={:#x} pressed={}",
                    i, usage, scancode, pressed
                );
                self.event_queue.push_back(KeyEvent {
                    usage_id: usage,
                    scancode,
                    pressed,
                    modifiers: new_mods,
                });
            }
        }
    }

    /// Dequeue the next key event, if any.
    pub fn next_event(&mut self) -> Option<KeyEvent> {
        self.event_queue.pop_front()
    }

    /// Check if there are pending key events.
    pub fn has_events(&self) -> bool {
        !self.event_queue.is_empty()
    }

    /// Get the current modifier state.
    pub fn modifiers(&self) -> u8 {
        self.prev_report.modifiers
    }
}
