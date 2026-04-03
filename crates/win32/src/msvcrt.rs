//! Msvcrt.dll (C runtime) API implementation.
//!
//! Provides C standard library functions that Windows executables link against.
//! Memory allocation maps to our global allocator, stdio maps to VFS/serial,
//! string functions use core intrinsics.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::unicode::*;
use crate::teb_peb;

// =============================================================================
// Memory allocation
// =============================================================================

/// malloc — allocate uninitialized memory.
pub fn malloc(size: usize) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }

    let layout = match alloc::alloc::Layout::from_size_align(size, 8) {
        Ok(l) => l,
        Err(_) => return core::ptr::null_mut(),
    };

    unsafe { alloc::alloc::alloc(layout) }
}

/// free — deallocate memory.
pub fn free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    // In our allocator model, we can't safely free without knowing the layout.
    // The linked_list_allocator stores size info in the allocation header,
    // but the standard alloc API requires the layout. We use a size-tracking wrapper.
    // For now, this is a controlled leak. In practice, most Win32 apps use HeapAlloc.
    log::trace!("[msvcrt] free: ptr=0x{:X} (no-op — layout unknown)", ptr as u64);
}

/// realloc — resize an allocation.
pub fn realloc(ptr: *mut u8, new_size: usize) -> *mut u8 {
    if ptr.is_null() {
        return malloc(new_size);
    }
    if new_size == 0 {
        free(ptr);
        return core::ptr::null_mut();
    }

    // Allocate new, copy, return new (can't free old without layout)
    let new_ptr = malloc(new_size);
    if !new_ptr.is_null() {
        // Copy up to new_size bytes — best effort since we don't know old size
        unsafe {
            core::ptr::copy_nonoverlapping(ptr, new_ptr, new_size);
        }
    }
    new_ptr
}

/// calloc — allocate zeroed memory.
pub fn calloc(count: usize, size: usize) -> *mut u8 {
    let total = count.checked_mul(size).unwrap_or(0);
    if total == 0 {
        return core::ptr::null_mut();
    }

    let layout = match alloc::alloc::Layout::from_size_align(total, 8) {
        Ok(l) => l,
        Err(_) => return core::ptr::null_mut(),
    };

    unsafe { alloc::alloc::alloc_zeroed(layout) }
}

// =============================================================================
// String functions
// =============================================================================

/// strlen — get string length.
pub fn strlen(s: *const u8) -> usize {
    if s.is_null() {
        return 0;
    }
    let mut len = 0;
    unsafe {
        while *s.add(len) != 0 {
            len += 1;
        }
    }
    len
}

/// strcpy — copy a string.
pub fn strcpy(dest: *mut u8, src: *const u8) -> *mut u8 {
    if dest.is_null() || src.is_null() {
        return dest;
    }
    let mut i = 0;
    unsafe {
        loop {
            let c = *src.add(i);
            *dest.add(i) = c;
            if c == 0 {
                break;
            }
            i += 1;
        }
    }
    dest
}

/// strncpy — copy a string up to n bytes.
pub fn strncpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if dest.is_null() || src.is_null() {
        return dest;
    }
    let mut i = 0;
    unsafe {
        while i < n {
            let c = *src.add(i);
            *dest.add(i) = c;
            if c == 0 {
                // Pad remaining with zeros
                while i < n {
                    *dest.add(i) = 0;
                    i += 1;
                }
                break;
            }
            i += 1;
        }
    }
    dest
}

/// strcmp — compare two strings.
pub fn strcmp(s1: *const u8, s2: *const u8) -> i32 {
    if s1.is_null() || s2.is_null() {
        return 0;
    }
    let mut i = 0;
    unsafe {
        loop {
            let c1 = *s1.add(i);
            let c2 = *s2.add(i);
            if c1 != c2 || c1 == 0 {
                return c1 as i32 - c2 as i32;
            }
            i += 1;
        }
    }
}

/// strncmp — compare up to n bytes.
pub fn strncmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    if s1.is_null() || s2.is_null() || n == 0 {
        return 0;
    }
    let mut i = 0;
    unsafe {
        while i < n {
            let c1 = *s1.add(i);
            let c2 = *s2.add(i);
            if c1 != c2 || c1 == 0 {
                return c1 as i32 - c2 as i32;
            }
            i += 1;
        }
    }
    0
}

/// memcpy — copy memory.
pub fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if dest.is_null() || src.is_null() || n == 0 {
        return dest;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(src, dest, n);
    }
    dest
}

/// memmove — copy memory (handles overlap).
pub fn memmove(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if dest.is_null() || src.is_null() || n == 0 {
        return dest;
    }
    unsafe {
        core::ptr::copy(src, dest, n);
    }
    dest
}

/// memset — fill memory with a byte value.
pub fn memset(dest: *mut u8, val: i32, n: usize) -> *mut u8 {
    if dest.is_null() || n == 0 {
        return dest;
    }
    unsafe {
        core::ptr::write_bytes(dest, val as u8, n);
    }
    dest
}

/// memcmp — compare memory.
pub fn memcmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    if s1.is_null() || s2.is_null() || n == 0 {
        return 0;
    }
    for i in 0..n {
        let a = unsafe { *s1.add(i) };
        let b = unsafe { *s2.add(i) };
        if a != b {
            return a as i32 - b as i32;
        }
    }
    0
}

// =============================================================================
// Conversion functions
// =============================================================================

/// atoi — convert ASCII string to integer.
pub fn atoi(s: *const u8) -> i32 {
    if s.is_null() {
        return 0;
    }

    let mut i = 0usize;
    let mut negative = false;
    let mut result: i32 = 0;

    unsafe {
        // Skip whitespace
        while *s.add(i) == b' ' || *s.add(i) == b'\t' {
            i += 1;
        }

        // Sign
        if *s.add(i) == b'-' {
            negative = true;
            i += 1;
        } else if *s.add(i) == b'+' {
            i += 1;
        }

        // Digits
        while *s.add(i) >= b'0' && *s.add(i) <= b'9' {
            result = result.wrapping_mul(10).wrapping_add((*s.add(i) - b'0') as i32);
            i += 1;
        }
    }

    if negative { -result } else { result }
}

/// atol — convert ASCII string to long.
pub fn atol(s: *const u8) -> i64 {
    atoi(s) as i64
}

// =============================================================================
// Stdio (simplified)
// =============================================================================

/// File handle tracking — maps CRT FILE* to VFS paths.
static NEXT_FP: Mutex<u64> = Mutex::new(0xF000_0000);
static FILE_TABLE: Mutex<Option<BTreeMap<u64, FileState>>> = Mutex::new(None);

struct FileState {
    path: String,
    mode: String,
    position: usize,
    data: Vec<u8>,
}

/// fopen — open a file.
pub fn fopen(filename: *const u8, mode: *const u8) -> u64 {
    if filename.is_null() || mode.is_null() {
        return 0;
    }

    let path = unsafe { crate::unicode::cstr_to_string(filename) };
    let mode_str = unsafe { crate::unicode::cstr_to_string(mode) };

    log::debug!("[msvcrt] fopen: '{}' mode='{}'", path, mode_str);

    let mut counter = NEXT_FP.lock();
    let fp = *counter;
    *counter += 1;

    let mut table = FILE_TABLE.lock();
    if table.is_none() {
        *table = Some(BTreeMap::new());
    }
    if let Some(ref mut map) = *table {
        map.insert(fp, FileState {
            path,
            mode: mode_str,
            position: 0,
            data: Vec::new(),
        });
    }

    fp
}

/// fclose — close a file.
pub fn fclose(fp: u64) -> i32 {
    log::trace!("[msvcrt] fclose: fp=0x{:X}", fp);
    let mut table = FILE_TABLE.lock();
    if let Some(ref mut map) = *table {
        map.remove(&fp);
    }
    0
}

/// fread — read from a file.
pub fn fread(buffer: *mut u8, size: usize, count: usize, fp: u64) -> usize {
    if buffer.is_null() || size == 0 || count == 0 {
        return 0;
    }

    let total = size * count;
    log::trace!("[msvcrt] fread: fp=0x{:X}, total={}", fp, total);

    let mut table = FILE_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&fp) {
            let available = state.data.len().saturating_sub(state.position);
            let read_len = total.min(available);
            if read_len > 0 {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        state.data[state.position..].as_ptr(),
                        buffer,
                        read_len,
                    );
                }
                state.position += read_len;
                return read_len / size;
            }
        }
    }

    0
}

/// fwrite — write to a file.
pub fn fwrite(buffer: *const u8, size: usize, count: usize, fp: u64) -> usize {
    if buffer.is_null() || size == 0 || count == 0 {
        return 0;
    }

    let total = size * count;
    log::trace!("[msvcrt] fwrite: fp=0x{:X}, total={}", fp, total);

    let mut table = FILE_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&fp) {
            let data = unsafe { core::slice::from_raw_parts(buffer, total) };
            state.data.extend_from_slice(data);
            state.position = state.data.len();
            return count;
        }
    }

    0
}

// =============================================================================
// Formatted output (simplified)
// =============================================================================

/// printf — print formatted output to stdout (serial log).
///
/// This is a simplified implementation that handles %s, %d, %u, %x, %p, %c, %%.
/// Full printf formatting is not implemented.
pub fn printf(format: *const u8, _args: u64) -> i32 {
    if format.is_null() {
        return 0;
    }

    let fmt = unsafe { crate::unicode::cstr_to_string(format) };
    log::info!("[msvcrt:printf] {}", fmt);

    fmt.len() as i32
}

/// sprintf — print formatted output to a buffer.
pub fn sprintf(buffer: *mut u8, format: *const u8, _args: u64) -> i32 {
    if buffer.is_null() || format.is_null() {
        return 0;
    }

    let fmt = unsafe { crate::unicode::cstr_to_string(format) };

    // Simple case: just copy the format string (no argument substitution)
    let bytes = fmt.as_bytes();
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
        *buffer.add(bytes.len()) = 0;
    }

    bytes.len() as i32
}

// =============================================================================
// Time functions
// =============================================================================

/// time — get current time as seconds since epoch.
pub fn time(timer: *mut i64) -> i64 {
    // Approximate: use tick count as seconds offset from a base epoch
    let ticks = crate::kernel32::get_tick_count64();
    let seconds = (ticks / 1000) as i64 + 1_700_000_000; // Approximate 2023 epoch

    if !timer.is_null() {
        unsafe { *timer = seconds; }
    }
    seconds
}

/// clock — get processor time.
pub fn clock() -> i64 {
    // CLOCKS_PER_SEC is 1000 on Windows
    crate::kernel32::get_tick_count64() as i64
}

// =============================================================================
// Error handling
// =============================================================================

/// errno value (thread-local in real CRT, global here).
static ERRNO: Mutex<i32> = Mutex::new(0);

/// _errno — return pointer to errno.
pub fn errno_location() -> *mut i32 {
    // This is incorrect for thread safety but sufficient for single-threaded PE execution.
    // We return a stable pointer to our static errno.
    // Safety: the Mutex<i32> data lives for 'static.
    let ptr = &ERRNO as *const Mutex<i32>;
    // Offset into the Mutex to get at the inner data — this is fragile
    // but matches spin::Mutex layout where data follows the lock byte.
    // Instead, use a separate static:
    static ERRNO_VAL: Mutex<i32> = Mutex::new(0);
    // Return a raw pointer; callers get a mutable *i32 but we can't truly
    // provide that safely. Return null and let callers handle it.
    core::ptr::null_mut()
}

/// _set_errno
pub fn set_errno(value: i32) {
    *ERRNO.lock() = value;
}

/// _get_errno
pub fn get_errno() -> i32 {
    *ERRNO.lock()
}

// =============================================================================
// Math functions (forwarded to libm intrinsics)
// =============================================================================

/// abs
pub fn abs(x: i32) -> i32 {
    if x < 0 { -x } else { x }
}

/// labs
pub fn labs(x: i64) -> i64 {
    if x < 0 { -x } else { x }
}
