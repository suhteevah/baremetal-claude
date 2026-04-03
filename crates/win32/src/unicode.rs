//! UTF-16LE string handling for Win32 compatibility.
//!
//! Windows uses UTF-16LE for all "wide" string APIs (the W-suffix functions).
//! Internally ClaudioOS uses UTF-8. This module provides conversion utilities
//! and type aliases for Win32 string types.

use alloc::string::String;
use alloc::vec::Vec;

/// Pointer to a mutable null-terminated UTF-16LE string (LPWSTR).
pub type LpWStr = *mut u16;
/// Pointer to a const null-terminated UTF-16LE string (LPCWSTR).
pub type LpcWStr = *const u16;
/// Pointer to a mutable null-terminated UTF-8 string (LPSTR).
pub type LpStr = *mut u8;
/// Pointer to a const null-terminated UTF-8 string (LPCSTR).
pub type LpcStr = *const u8;

/// Windows BOOL type (4 bytes, 0 = FALSE, nonzero = TRUE).
pub type Bool = i32;
pub const TRUE: Bool = 1;
pub const FALSE: Bool = 0;

/// Windows DWORD type.
pub type DWord = u32;
/// Windows HANDLE type.
pub type Handle = u64;
/// Windows HMODULE type.
pub type HModule = u64;
/// Windows HINSTANCE type.
pub type HInstance = u64;
/// Windows HWND type.
pub type HWnd = u64;
/// Windows HDC type.
pub type Hdc = u64;
/// Windows HBRUSH type.
pub type HBrush = u64;
/// Windows HFONT type.
pub type HFont = u64;
/// Windows HBITMAP type.
pub type HBitmap = u64;
/// Windows HGDIOBJ type.
pub type HGdiObj = u64;
/// Windows SOCKET type.
pub type Socket = u64;
/// Windows HKEY type.
pub type HKey = u64;
/// Windows ATOM type.
pub type Atom = u16;
/// Windows WPARAM type.
pub type WParam = u64;
/// Windows LPARAM type.
pub type LParam = i64;
/// Windows LRESULT type.
pub type LResult = i64;
/// Windows UINT type.
pub type UInt = u32;
/// Windows SIZE_T type.
pub type SizeT = u64;

/// Invalid handle sentinel.
pub const INVALID_HANDLE_VALUE: Handle = u64::MAX; // -1 as u64

/// Standard handle constants for GetStdHandle.
pub const STD_INPUT_HANDLE: DWord = 0xFFFF_FFF6; // (DWORD)-10
pub const STD_OUTPUT_HANDLE: DWord = 0xFFFF_FFF5; // (DWORD)-11
pub const STD_ERROR_HANDLE: DWord = 0xFFFF_FFF4; // (DWORD)-12

/// Convert a null-terminated UTF-16LE string to a Rust UTF-8 String.
///
/// # Safety
/// `ptr` must be a valid pointer to a null-terminated UTF-16LE string.
pub unsafe fn utf16_to_utf8(ptr: LpcWStr) -> String {
    if ptr.is_null() {
        return String::new();
    }

    // Find the null terminator
    let mut len = 0;
    let mut p = ptr;
    while *p != 0 {
        len += 1;
        p = p.add(1);
    }

    let slice = core::slice::from_raw_parts(ptr, len);
    utf16_slice_to_string(slice)
}

/// Convert a UTF-16LE slice to a Rust String.
pub fn utf16_slice_to_string(slice: &[u16]) -> String {
    let mut result = String::with_capacity(slice.len());
    let mut iter = slice.iter().copied();

    while let Some(unit) = iter.next() {
        if unit < 0xD800 || unit > 0xDFFF {
            // BMP character
            if let Some(ch) = char::from_u32(unit as u32) {
                result.push(ch);
            } else {
                result.push('\u{FFFD}');
            }
        } else if (0xD800..=0xDBFF).contains(&unit) {
            // High surrogate — need low surrogate
            if let Some(low) = iter.next() {
                if (0xDC00..=0xDFFF).contains(&low) {
                    let cp = 0x10000 + ((unit as u32 - 0xD800) << 10) + (low as u32 - 0xDC00);
                    if let Some(ch) = char::from_u32(cp) {
                        result.push(ch);
                    } else {
                        result.push('\u{FFFD}');
                    }
                } else {
                    result.push('\u{FFFD}');
                }
            } else {
                result.push('\u{FFFD}');
            }
        } else {
            // Lone low surrogate
            result.push('\u{FFFD}');
        }
    }

    result
}

/// Convert a Rust UTF-8 string to a null-terminated UTF-16LE vector.
pub fn utf8_to_utf16(s: &str) -> Vec<u16> {
    let mut result: Vec<u16> = s.encode_utf16().collect();
    result.push(0); // Null terminator
    result
}

/// Convert a Rust UTF-8 string to UTF-16LE and write into a buffer.
///
/// Returns the number of u16 units written (including null terminator),
/// or 0 if the buffer is too small.
pub fn utf8_to_utf16_buf(s: &str, buf: &mut [u16]) -> usize {
    let encoded: Vec<u16> = s.encode_utf16().collect();
    let needed = encoded.len() + 1; // +1 for null terminator
    if needed > buf.len() {
        return 0;
    }
    buf[..encoded.len()].copy_from_slice(&encoded);
    buf[encoded.len()] = 0;
    needed
}

/// Read a null-terminated C string (LPCSTR) into a Rust String.
///
/// # Safety
/// `ptr` must be a valid pointer to a null-terminated byte string.
pub unsafe fn cstr_to_string(ptr: LpcStr) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0;
    let mut p = ptr;
    while *p != 0 {
        len += 1;
        p = p.add(1);
    }
    let slice = core::slice::from_raw_parts(ptr, len);
    String::from_utf8_lossy(slice).into_owned()
}

/// Windows GUID structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub const ZERO: Guid = Guid {
        data1: 0,
        data2: 0,
        data3: 0,
        data4: [0; 8],
    };
}
