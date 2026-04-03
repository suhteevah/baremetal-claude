//! NT Native API (ntdll.dll) implementation.
//!
//! These are the lowest-level Windows APIs. kernel32 functions often just wrap
//! these with friendlier types. Programs rarely call these directly, but the
//! CRT and system DLLs use them internally.

use alloc::string::String;
use crate::unicode::{Handle, DWord, Bool, LpcWStr, SizeT, INVALID_HANDLE_VALUE};
use crate::handles;
use crate::teb_peb;

/// NTSTATUS type — 0 = success, negative = error.
pub type NtStatus = i32;

/// NT status codes.
pub const STATUS_SUCCESS: NtStatus = 0;
pub const STATUS_INVALID_HANDLE: NtStatus = 0xC0000008_u32 as i32;
pub const STATUS_INVALID_PARAMETER: NtStatus = 0xC000000D_u32 as i32;
pub const STATUS_NOT_IMPLEMENTED: NtStatus = 0xC0000002_u32 as i32;
pub const STATUS_NO_MEMORY: NtStatus = 0xC0000017_u32 as i32;
pub const STATUS_ACCESS_DENIED: NtStatus = 0xC0000022_u32 as i32;
pub const STATUS_OBJECT_NAME_NOT_FOUND: NtStatus = 0xC0000034_u32 as i32;
pub const STATUS_BUFFER_TOO_SMALL: NtStatus = 0xC0000023_u32 as i32;

/// IO_STATUS_BLOCK.
#[repr(C)]
pub struct IoStatusBlock {
    pub status: NtStatus,
    pub information: u64,
}

/// UNICODE_STRING.
#[repr(C)]
pub struct UnicodeString {
    pub length: u16,
    pub maximum_length: u16,
    pub buffer: LpcWStr,
}

/// OBJECT_ATTRIBUTES.
#[repr(C)]
pub struct ObjectAttributes {
    pub length: u32,
    pub root_directory: Handle,
    pub object_name: *const UnicodeString,
    pub attributes: u32,
    pub security_descriptor: u64,
    pub security_quality_of_service: u64,
}

/// LARGE_INTEGER.
#[repr(C)]
pub union LargeInteger {
    pub quad_part: i64,
    pub parts: LargeIntegerParts,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LargeIntegerParts {
    pub low_part: u32,
    pub high_part: i32,
}

// --- File operations ---

/// NtCreateFile — create or open a file.
pub fn nt_create_file(
    file_handle: *mut Handle,
    desired_access: u32,
    object_attributes: *const ObjectAttributes,
    io_status_block: *mut IoStatusBlock,
    _allocation_size: *const LargeInteger,
    _file_attributes: u32,
    _share_access: u32,
    _create_disposition: u32,
    _create_options: u32,
    _ea_buffer: u64,
    _ea_length: u32,
) -> NtStatus {
    if file_handle.is_null() || object_attributes.is_null() {
        return STATUS_INVALID_PARAMETER;
    }

    let path = unsafe {
        let attrs = &*object_attributes;
        if attrs.object_name.is_null() {
            return STATUS_INVALID_PARAMETER;
        }
        let ustr = &*attrs.object_name;
        if ustr.buffer.is_null() || ustr.length == 0 {
            return STATUS_INVALID_PARAMETER;
        }
        crate::unicode::utf16_to_utf8(ustr.buffer)
    };

    // Strip NT-style path prefix (\??\, \Device\, etc.)
    let clean_path = strip_nt_prefix(&path);
    log::debug!("[ntdll] NtCreateFile: '{}' (access=0x{:X})", clean_path, desired_access);

    let handle = handles::alloc_handle(handles::HandleObject::File {
        path: String::from(clean_path),
        fd: 0,
        access: desired_access,
    });

    unsafe {
        *file_handle = handle;
        if !io_status_block.is_null() {
            (*io_status_block).status = STATUS_SUCCESS;
            (*io_status_block).information = 0; // FILE_OPENED
        }
    }

    STATUS_SUCCESS
}

/// NtReadFile — read data from a file.
pub fn nt_read_file(
    file_handle: Handle,
    _event: Handle,
    _apc_routine: u64,
    _apc_context: u64,
    io_status_block: *mut IoStatusBlock,
    buffer: *mut u8,
    length: u32,
    _byte_offset: *const LargeInteger,
    _key: *const u32,
) -> NtStatus {
    if handles::get_handle(file_handle).is_none() {
        return STATUS_INVALID_HANDLE;
    }
    if buffer.is_null() {
        return STATUS_INVALID_PARAMETER;
    }

    log::trace!("[ntdll] NtReadFile: handle=0x{:X}, len={}", file_handle, length);

    // Stub: return zero bytes read
    unsafe {
        if !io_status_block.is_null() {
            (*io_status_block).status = STATUS_SUCCESS;
            (*io_status_block).information = 0;
        }
    }

    STATUS_SUCCESS
}

/// NtWriteFile — write data to a file.
pub fn nt_write_file(
    file_handle: Handle,
    _event: Handle,
    _apc_routine: u64,
    _apc_context: u64,
    io_status_block: *mut IoStatusBlock,
    buffer: *const u8,
    length: u32,
    _byte_offset: *const LargeInteger,
    _key: *const u32,
) -> NtStatus {
    if handles::get_handle(file_handle).is_none() {
        return STATUS_INVALID_HANDLE;
    }
    if buffer.is_null() {
        return STATUS_INVALID_PARAMETER;
    }

    log::trace!("[ntdll] NtWriteFile: handle=0x{:X}, len={}", file_handle, length);

    unsafe {
        if !io_status_block.is_null() {
            (*io_status_block).status = STATUS_SUCCESS;
            (*io_status_block).information = length as u64;
        }
    }

    STATUS_SUCCESS
}

/// NtClose — close a handle.
pub fn nt_close(handle: Handle) -> NtStatus {
    log::trace!("[ntdll] NtClose: handle=0x{:X}", handle);
    if handles::close_handle(handle) {
        STATUS_SUCCESS
    } else {
        STATUS_INVALID_HANDLE
    }
}

// --- Memory operations ---

/// Memory allocation type flags.
pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_RELEASE: u32 = 0x8000;

/// NtAllocateVirtualMemory.
pub fn nt_allocate_virtual_memory(
    _process_handle: Handle,
    base_address: *mut u64,
    _zero_bits: u64,
    region_size: *mut SizeT,
    allocation_type: u32,
    protect: u32,
) -> NtStatus {
    if base_address.is_null() || region_size.is_null() {
        return STATUS_INVALID_PARAMETER;
    }

    let size = unsafe { *region_size } as usize;
    log::debug!(
        "[ntdll] NtAllocateVirtualMemory: size={}, type=0x{:X}, protect=0x{:X}",
        size, allocation_type, protect
    );

    if size == 0 || size > 1024 * 1024 * 1024 {
        return STATUS_INVALID_PARAMETER;
    }

    // Allocate from our heap
    let layout = alloc::alloc::Layout::from_size_align(size, 4096).unwrap();
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if ptr.is_null() {
        return STATUS_NO_MEMORY;
    }

    unsafe {
        *base_address = ptr as u64;
        *region_size = size as u64;
    }

    STATUS_SUCCESS
}

/// NtFreeVirtualMemory.
pub fn nt_free_virtual_memory(
    _process_handle: Handle,
    base_address: *mut u64,
    region_size: *mut SizeT,
    free_type: u32,
) -> NtStatus {
    if base_address.is_null() {
        return STATUS_INVALID_PARAMETER;
    }

    let addr = unsafe { *base_address };
    let size = if region_size.is_null() { 0 } else { unsafe { *region_size as usize } };
    log::debug!("[ntdll] NtFreeVirtualMemory: addr=0x{:X}, size={}, type=0x{:X}", addr, size, free_type);

    if addr != 0 && size > 0 {
        let layout = alloc::alloc::Layout::from_size_align(size, 4096).unwrap();
        unsafe { alloc::alloc::dealloc(addr as *mut u8, layout); }
    }

    STATUS_SUCCESS
}

/// NtProtectVirtualMemory.
pub fn nt_protect_virtual_memory(
    _process_handle: Handle,
    base_address: *mut u64,
    region_size: *mut SizeT,
    new_protect: u32,
    old_protect: *mut u32,
) -> NtStatus {
    log::trace!("[ntdll] NtProtectVirtualMemory: protect=0x{:X}", new_protect);
    // In our single-address-space model, memory protection is not enforced.
    // Just return the old protection as PAGE_EXECUTE_READWRITE.
    if !old_protect.is_null() {
        unsafe { *old_protect = 0x40; } // PAGE_EXECUTE_READWRITE
    }
    STATUS_SUCCESS
}

// --- Thread/Process operations ---

/// NtCreateThread (stub).
pub fn nt_create_thread(
    thread_handle: *mut Handle,
    _desired_access: u32,
    _object_attributes: *const ObjectAttributes,
    _process_handle: Handle,
    _client_id: u64,
    _thread_context: u64,
    _initial_teb: u64,
    _create_suspended: Bool,
) -> NtStatus {
    log::warn!("[ntdll] NtCreateThread: stub — ClaudioOS uses async tasks, not OS threads");
    if !thread_handle.is_null() {
        let handle = handles::alloc_handle(handles::HandleObject::Thread { thread_id: 2 });
        unsafe { *thread_handle = handle; }
    }
    STATUS_SUCCESS
}

/// NtTerminateThread.
pub fn nt_terminate_thread(thread_handle: Handle, exit_status: NtStatus) -> NtStatus {
    log::info!("[ntdll] NtTerminateThread: handle=0x{:X}, status={}", thread_handle, exit_status);
    handles::close_handle(thread_handle);
    STATUS_SUCCESS
}

/// NtTerminateProcess.
pub fn nt_terminate_process(_process_handle: Handle, exit_status: NtStatus) -> NtStatus {
    log::info!("[ntdll] NtTerminateProcess: status={}", exit_status);
    STATUS_SUCCESS
}

// --- Synchronization ---

/// NtWaitForSingleObject.
pub fn nt_wait_for_single_object(
    handle: Handle,
    alertable: Bool,
    timeout: *const LargeInteger,
) -> NtStatus {
    log::trace!("[ntdll] NtWaitForSingleObject: handle=0x{:X}", handle);

    // Check if the object is already signaled
    if let Some(obj) = handles::get_handle(handle) {
        match obj {
            handles::HandleObject::Event { signaled, .. } if signaled => {
                return STATUS_SUCCESS; // WAIT_OBJECT_0
            }
            handles::HandleObject::Mutex { locked: false, .. } => {
                return STATUS_SUCCESS;
            }
            _ => {}
        }
    }

    // In our cooperative async model, we can't truly block.
    // Return success (signaled) to prevent deadlocks.
    STATUS_SUCCESS
}

/// NtWaitForMultipleObjects.
pub fn nt_wait_for_multiple_objects(
    count: u32,
    handles_ptr: *const Handle,
    wait_type: u32, // 0 = WaitAll, 1 = WaitAny
    alertable: Bool,
    timeout: *const LargeInteger,
) -> NtStatus {
    log::trace!("[ntdll] NtWaitForMultipleObjects: count={}, type={}", count, wait_type);
    STATUS_SUCCESS
}

/// NtCreateEvent.
pub fn nt_create_event(
    event_handle: *mut Handle,
    desired_access: u32,
    object_attributes: *const ObjectAttributes,
    event_type: u32, // 0 = NotificationEvent (manual), 1 = SynchronizationEvent (auto)
    initial_state: Bool,
) -> NtStatus {
    let manual_reset = event_type == 0;
    let signaled = initial_state != 0;

    let handle = handles::alloc_handle(handles::HandleObject::Event {
        name: None,
        signaled,
        manual_reset,
    });

    if !event_handle.is_null() {
        unsafe { *event_handle = handle; }
    }
    log::trace!("[ntdll] NtCreateEvent: handle=0x{:X}, manual={}, signaled={}", handle, manual_reset, signaled);
    STATUS_SUCCESS
}

/// NtCreateMutant (Mutex).
pub fn nt_create_mutant(
    mutant_handle: *mut Handle,
    desired_access: u32,
    object_attributes: *const ObjectAttributes,
    initial_owner: Bool,
) -> NtStatus {
    let handle = handles::alloc_handle(handles::HandleObject::Mutex {
        name: None,
        owner_thread: if initial_owner != 0 { Some(1) } else { None },
        locked: initial_owner != 0,
    });

    if !mutant_handle.is_null() {
        unsafe { *mutant_handle = handle; }
    }
    log::trace!("[ntdll] NtCreateMutant: handle=0x{:X}", handle);
    STATUS_SUCCESS
}

/// NtCreateSemaphore.
pub fn nt_create_semaphore(
    semaphore_handle: *mut Handle,
    desired_access: u32,
    object_attributes: *const ObjectAttributes,
    initial_count: i32,
    maximum_count: i32,
) -> NtStatus {
    let handle = handles::alloc_handle(handles::HandleObject::Semaphore {
        name: None,
        count: initial_count,
        max_count: maximum_count,
    });

    if !semaphore_handle.is_null() {
        unsafe { *semaphore_handle = handle; }
    }
    log::trace!("[ntdll] NtCreateSemaphore: handle=0x{:X}, count={}", handle, initial_count);
    STATUS_SUCCESS
}

// --- Query operations ---

/// NtQueryInformationProcess (stub).
pub fn nt_query_information_process(
    _process_handle: Handle,
    info_class: u32,
    info_buffer: *mut u8,
    info_length: u32,
    return_length: *mut u32,
) -> NtStatus {
    log::trace!("[ntdll] NtQueryInformationProcess: class={}", info_class);
    // Return minimal info for common queries
    if !return_length.is_null() {
        unsafe { *return_length = 0; }
    }
    STATUS_SUCCESS
}

/// NtQuerySystemInformation (stub).
pub fn nt_query_system_information(
    info_class: u32,
    info_buffer: *mut u8,
    info_length: u32,
    return_length: *mut u32,
) -> NtStatus {
    log::trace!("[ntdll] NtQuerySystemInformation: class={}", info_class);
    if !return_length.is_null() {
        unsafe { *return_length = 0; }
    }
    STATUS_SUCCESS
}

// --- Memory section operations ---

/// NtCreateSection.
pub fn nt_create_section(
    section_handle: *mut Handle,
    desired_access: u32,
    _object_attributes: *const ObjectAttributes,
    _maximum_size: *const LargeInteger,
    _section_page_protection: u32,
    _allocation_attributes: u32,
    _file_handle: Handle,
) -> NtStatus {
    let handle = handles::alloc_handle(handles::HandleObject::Section {
        base: 0,
        size: 0,
    });
    if !section_handle.is_null() {
        unsafe { *section_handle = handle; }
    }
    log::trace!("[ntdll] NtCreateSection: handle=0x{:X}", handle);
    STATUS_SUCCESS
}

/// NtMapViewOfSection.
pub fn nt_map_view_of_section(
    section_handle: Handle,
    _process_handle: Handle,
    base_address: *mut u64,
    _zero_bits: u64,
    _commit_size: SizeT,
    _section_offset: *mut LargeInteger,
    view_size: *mut SizeT,
    _inherit_disposition: u32,
    _allocation_type: u32,
    _win32_protect: u32,
) -> NtStatus {
    log::trace!("[ntdll] NtMapViewOfSection: section=0x{:X}", section_handle);
    STATUS_SUCCESS
}

// --- Rtl utility functions ---

/// RtlInitUnicodeString — initialize a UNICODE_STRING from a wide string pointer.
pub fn rtl_init_unicode_string(dest: *mut UnicodeString, source: LpcWStr) {
    if dest.is_null() {
        return;
    }
    if source.is_null() {
        unsafe {
            (*dest).length = 0;
            (*dest).maximum_length = 0;
            (*dest).buffer = core::ptr::null();
        }
    } else {
        // Count characters
        let mut len = 0u16;
        unsafe {
            let mut p = source;
            while *p != 0 {
                len += 1;
                p = p.add(1);
            }
            let byte_len = len * 2;
            (*dest).length = byte_len;
            (*dest).maximum_length = byte_len + 2;
            (*dest).buffer = source;
        }
    }
}

/// RtlCopyMemory — same as memcpy.
pub fn rtl_copy_memory(dest: *mut u8, source: *const u8, length: usize) {
    if dest.is_null() || source.is_null() || length == 0 {
        return;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(source, dest, length);
    }
}

/// Strip NT path prefixes like "\??\" or "\Device\HarddiskVolume1\" etc.
fn strip_nt_prefix(path: &str) -> &str {
    if let Some(rest) = path.strip_prefix("\\??\\") {
        rest
    } else if let Some(rest) = path.strip_prefix("\\Device\\") {
        // Skip to the first backslash after the device name
        if let Some(pos) = rest.find('\\') {
            &rest[pos..]
        } else {
            rest
        }
    } else {
        path
    }
}
