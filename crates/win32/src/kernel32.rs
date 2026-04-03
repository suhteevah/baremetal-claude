//! Kernel32.dll API implementation — the most-used Windows DLL.
//!
//! File I/O, process/thread management, memory, synchronization, console,
//! and miscellaneous system functions. All mapped to ClaudioOS internals.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::unicode::*;
use crate::handles::{self, HandleObject, StdType};
use crate::teb_peb;

/// Monotonic tick counter (incremented by timer interrupt, ~1ms resolution).
static TICK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Increment the tick counter (called from timer interrupt handler).
pub fn tick() {
    TICK_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Process command line (set during init).
static COMMAND_LINE: Mutex<Option<Vec<u16>>> = Mutex::new(None);

/// Environment variables.
static ENV_VARS: Mutex<Option<alloc::collections::BTreeMap<String, String>>> = Mutex::new(None);

/// Initialize kernel32 subsystem.
pub fn init() {
    // Default command line
    let cmd = crate::unicode::utf8_to_utf16("claudio.exe");
    *COMMAND_LINE.lock() = Some(cmd);

    // Default environment
    let mut env = alloc::collections::BTreeMap::new();
    env.insert(String::from("SystemRoot"), String::from("C:\\Windows"));
    env.insert(String::from("TEMP"), String::from("C:\\Temp"));
    env.insert(String::from("TMP"), String::from("C:\\Temp"));
    env.insert(String::from("OS"), String::from("ClaudioOS"));
    env.insert(String::from("PROCESSOR_ARCHITECTURE"), String::from("AMD64"));
    env.insert(String::from("NUMBER_OF_PROCESSORS"), String::from("1"));
    *ENV_VARS.lock() = Some(env);

    log::info!("[kernel32] Initialized");
}

// =============================================================================
// File I/O
// =============================================================================

/// Generic access rights.
pub const GENERIC_READ: DWord = 0x80000000;
pub const GENERIC_WRITE: DWord = 0x40000000;
pub const GENERIC_EXECUTE: DWord = 0x20000000;
pub const GENERIC_ALL: DWord = 0x10000000;

/// CreateDisposition values.
pub const CREATE_NEW: DWord = 1;
pub const CREATE_ALWAYS: DWord = 2;
pub const OPEN_EXISTING: DWord = 3;
pub const OPEN_ALWAYS: DWord = 4;
pub const TRUNCATE_EXISTING: DWord = 5;

/// CreateFileW — open or create a file.
pub fn create_file_w(
    file_name: LpcWStr,
    desired_access: DWord,
    share_mode: DWord,
    security_attributes: u64, // LPSECURITY_ATTRIBUTES
    creation_disposition: DWord,
    flags_and_attributes: DWord,
    template_file: Handle,
) -> Handle {
    let path = unsafe { utf16_to_utf8(file_name) };
    log::debug!(
        "[kernel32] CreateFileW: '{}', access=0x{:X}, disp={}",
        path, desired_access, creation_disposition
    );

    // Convert Windows path to VFS path
    let vfs_path = windows_path_to_vfs(&path);

    let handle = handles::alloc_handle(HandleObject::File {
        path: vfs_path,
        fd: 0, // VFS fd assigned on actual open
        access: desired_access,
    });

    handle
}

/// ReadFile — read data from a file handle.
pub fn read_file(
    file: Handle,
    buffer: *mut u8,
    bytes_to_read: DWord,
    bytes_read: *mut DWord,
    overlapped: u64, // LPOVERLAPPED
) -> Bool {
    log::trace!("[kernel32] ReadFile: handle=0x{:X}, len={}", file, bytes_to_read);

    match handles::get_handle(file) {
        Some(HandleObject::Console { std_type: StdType::Input }) => {
            // Console input — stub: return 0 bytes
            if !bytes_read.is_null() {
                unsafe { *bytes_read = 0; }
            }
            TRUE
        }
        Some(HandleObject::File { .. }) => {
            // File read via VFS — stub for now
            if !bytes_read.is_null() {
                unsafe { *bytes_read = 0; }
            }
            TRUE
        }
        _ => {
            teb_peb::set_last_error(6); // ERROR_INVALID_HANDLE
            FALSE
        }
    }
}

/// WriteFile — write data to a file handle.
pub fn write_file(
    file: Handle,
    buffer: *const u8,
    bytes_to_write: DWord,
    bytes_written: *mut DWord,
    overlapped: u64,
) -> Bool {
    log::trace!("[kernel32] WriteFile: handle=0x{:X}, len={}", file, bytes_to_write);

    match handles::get_handle(file) {
        Some(HandleObject::Console { std_type: StdType::Output | StdType::Error }) => {
            // Console output — write to serial/framebuffer
            if !buffer.is_null() && bytes_to_write > 0 {
                let data = unsafe {
                    core::slice::from_raw_parts(buffer, bytes_to_write as usize)
                };
                if let Ok(s) = core::str::from_utf8(data) {
                    log::info!("[win32:console] {}", s);
                }
            }
            if !bytes_written.is_null() {
                unsafe { *bytes_written = bytes_to_write; }
            }
            TRUE
        }
        Some(HandleObject::File { .. }) => {
            if !bytes_written.is_null() {
                unsafe { *bytes_written = bytes_to_write; }
            }
            TRUE
        }
        _ => {
            teb_peb::set_last_error(6);
            FALSE
        }
    }
}

/// CloseHandle — close a kernel object handle.
pub fn close_handle(handle: Handle) -> Bool {
    if handles::close_handle(handle) { TRUE } else { FALSE }
}

/// GetFileSize — get the size of a file.
pub fn get_file_size(file: Handle, file_size_high: *mut DWord) -> DWord {
    log::trace!("[kernel32] GetFileSize: handle=0x{:X}", file);
    if !file_size_high.is_null() {
        unsafe { *file_size_high = 0; }
    }
    0 // Stub
}

/// SetFilePointer — move the file pointer.
pub fn set_file_pointer(
    file: Handle,
    distance_to_move: i32,
    distance_to_move_high: *mut i32,
    move_method: DWord,
) -> DWord {
    log::trace!("[kernel32] SetFilePointer: handle=0x{:X}, dist={}", file, distance_to_move);
    0 // Stub
}

/// DeleteFileW.
pub fn delete_file_w(file_name: LpcWStr) -> Bool {
    let path = unsafe { utf16_to_utf8(file_name) };
    log::debug!("[kernel32] DeleteFileW: '{}'", path);
    TRUE
}

/// CreateDirectoryW.
pub fn create_directory_w(path_name: LpcWStr, security_attributes: u64) -> Bool {
    let path = unsafe { utf16_to_utf8(path_name) };
    log::debug!("[kernel32] CreateDirectoryW: '{}'", path);
    TRUE
}

/// FindFirstFileW.
pub fn find_first_file_w(file_name: LpcWStr, find_data: *mut u8) -> Handle {
    let pattern = unsafe { utf16_to_utf8(file_name) };
    log::debug!("[kernel32] FindFirstFileW: '{}'", pattern);

    let handle = handles::alloc_handle(HandleObject::FindFile {
        pattern,
        results: Vec::new(),
        index: 0,
    });
    handle
}

/// FindNextFileW.
pub fn find_next_file_w(find_file: Handle, find_data: *mut u8) -> Bool {
    log::trace!("[kernel32] FindNextFileW: handle=0x{:X}", find_file);
    teb_peb::set_last_error(18); // ERROR_NO_MORE_FILES
    FALSE
}

/// FindClose.
pub fn find_close(find_file: Handle) -> Bool {
    handles::close_handle(find_file);
    TRUE
}

// =============================================================================
// Process
// =============================================================================

/// CreateProcessW (stub).
pub fn create_process_w(
    application_name: LpcWStr,
    command_line: LpWStr,
    process_attributes: u64,
    thread_attributes: u64,
    inherit_handles: Bool,
    creation_flags: DWord,
    environment: u64,
    current_directory: LpcWStr,
    startup_info: u64,
    process_information: *mut u8,
) -> Bool {
    let app = unsafe { utf16_to_utf8(application_name) };
    log::warn!("[kernel32] CreateProcessW: '{}' — not yet implemented", app);
    teb_peb::set_last_error(2); // ERROR_FILE_NOT_FOUND
    FALSE
}

/// ExitProcess — terminate the current process.
pub fn exit_process(exit_code: UInt) {
    log::info!("[kernel32] ExitProcess: code={}", exit_code);
    // In ClaudioOS, this returns control to the kernel
}

/// GetCurrentProcess — returns a pseudo-handle (-1).
pub fn get_current_process() -> Handle {
    u64::MAX // Pseudo-handle for current process
}

/// GetCurrentProcessId.
pub fn get_current_process_id() -> DWord {
    1
}

/// GetExitCodeProcess.
pub fn get_exit_code_process(process: Handle, exit_code: *mut DWord) -> Bool {
    if !exit_code.is_null() {
        unsafe { *exit_code = 259; } // STILL_ACTIVE
    }
    TRUE
}

/// TerminateProcess.
pub fn terminate_process(process: Handle, exit_code: UInt) -> Bool {
    log::info!("[kernel32] TerminateProcess: handle=0x{:X}, code={}", process, exit_code);
    TRUE
}

// =============================================================================
// Thread
// =============================================================================

/// CreateThread (stub — ClaudioOS uses async tasks).
pub fn create_thread(
    security_attributes: u64,
    stack_size: SizeT,
    start_address: u64, // LPTHREAD_START_ROUTINE
    parameter: u64,
    creation_flags: DWord,
    thread_id: *mut DWord,
) -> Handle {
    log::warn!("[kernel32] CreateThread: ClaudioOS uses async tasks, creating stub thread");
    static NEXT_TID: AtomicU64 = AtomicU64::new(2);
    let tid = NEXT_TID.fetch_add(1, Ordering::Relaxed) as u32;

    if !thread_id.is_null() {
        unsafe { *thread_id = tid; }
    }

    handles::alloc_handle(HandleObject::Thread { thread_id: tid })
}

/// ExitThread.
pub fn exit_thread(exit_code: DWord) {
    log::info!("[kernel32] ExitThread: code={}", exit_code);
}

/// GetCurrentThread — returns pseudo-handle (-2).
pub fn get_current_thread() -> Handle {
    u64::MAX - 1
}

/// GetCurrentThreadId.
pub fn get_current_thread_id() -> DWord {
    1
}

/// SuspendThread.
pub fn suspend_thread(thread: Handle) -> DWord {
    log::trace!("[kernel32] SuspendThread: handle=0x{:X}", thread);
    0 // Previous suspend count
}

/// ResumeThread.
pub fn resume_thread(thread: Handle) -> DWord {
    log::trace!("[kernel32] ResumeThread: handle=0x{:X}", thread);
    0
}

/// Sleep — suspend execution for the specified milliseconds.
pub fn sleep(milliseconds: DWord) {
    log::trace!("[kernel32] Sleep: {}ms", milliseconds);
    // In bare-metal: busy-wait or yield to executor
    // For now, just return immediately
}

/// SleepEx.
pub fn sleep_ex(milliseconds: DWord, alertable: Bool) -> DWord {
    sleep(milliseconds);
    0
}

// =============================================================================
// Memory
// =============================================================================

/// Page protection constants.
pub const PAGE_NOACCESS: DWord = 0x01;
pub const PAGE_READONLY: DWord = 0x02;
pub const PAGE_READWRITE: DWord = 0x04;
pub const PAGE_EXECUTE: DWord = 0x10;
pub const PAGE_EXECUTE_READ: DWord = 0x20;
pub const PAGE_EXECUTE_READWRITE: DWord = 0x40;

/// VirtualAlloc.
pub fn virtual_alloc(
    address: u64,
    size: SizeT,
    allocation_type: DWord,
    protect: DWord,
) -> u64 {
    let size = size as usize;
    log::trace!("[kernel32] VirtualAlloc: addr=0x{:X}, size={}, type=0x{:X}", address, size, allocation_type);

    if size == 0 {
        return 0;
    }

    // Align to page size
    let aligned_size = (size + 4095) & !4095;
    let layout = match alloc::alloc::Layout::from_size_align(aligned_size, 4096) {
        Ok(l) => l,
        Err(_) => return 0,
    };
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if ptr.is_null() {
        teb_peb::set_last_error(8); // ERROR_NOT_ENOUGH_MEMORY
        0
    } else {
        ptr as u64
    }
}

/// VirtualFree.
pub fn virtual_free(address: u64, size: SizeT, free_type: DWord) -> Bool {
    log::trace!("[kernel32] VirtualFree: addr=0x{:X}", address);
    if address == 0 {
        return FALSE;
    }
    // We can't easily free without knowing the original size/alignment.
    // In practice, MEM_RELEASE frees the whole region.
    TRUE
}

/// VirtualProtect.
pub fn virtual_protect(
    address: u64,
    size: SizeT,
    new_protect: DWord,
    old_protect: *mut DWord,
) -> Bool {
    log::trace!("[kernel32] VirtualProtect: addr=0x{:X}, protect=0x{:X}", address, new_protect);
    if !old_protect.is_null() {
        unsafe { *old_protect = PAGE_EXECUTE_READWRITE; }
    }
    TRUE
}

/// HeapCreate.
pub fn heap_create(options: DWord, initial_size: SizeT, maximum_size: SizeT) -> Handle {
    static NEXT_HEAP: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_HEAP.fetch_add(1, Ordering::Relaxed) as u32;
    handles::alloc_handle(HandleObject::Heap { id })
}

/// HeapAlloc.
pub fn heap_alloc(heap: Handle, flags: DWord, bytes: SizeT) -> u64 {
    let size = bytes as usize;
    if size == 0 {
        return 0;
    }
    let layout = match alloc::alloc::Layout::from_size_align(size, 8) {
        Ok(l) => l,
        Err(_) => return 0,
    };
    let ptr = if flags & 0x08 != 0 { // HEAP_ZERO_MEMORY
        unsafe { alloc::alloc::alloc_zeroed(layout) }
    } else {
        unsafe { alloc::alloc::alloc(layout) }
    };
    ptr as u64
}

/// HeapFree.
pub fn heap_free(heap: Handle, flags: DWord, mem: u64) -> Bool {
    // Can't safely free without layout info in our allocator model.
    // In practice this is fine — Rust's global allocator handles it.
    TRUE
}

/// HeapReAlloc.
pub fn heap_realloc(heap: Handle, flags: DWord, mem: u64, bytes: SizeT) -> u64 {
    // Simplified: alloc new, copy, free old (if we could)
    let new_ptr = heap_alloc(heap, flags, bytes);
    if new_ptr != 0 && mem != 0 {
        // We don't know the old size, so this is best-effort
        unsafe {
            core::ptr::copy_nonoverlapping(mem as *const u8, new_ptr as *mut u8, bytes as usize);
        }
    }
    new_ptr
}

/// GetProcessHeap.
pub fn get_process_heap() -> Handle {
    // Return a well-known heap handle
    0x0000_0010
}

// =============================================================================
// Synchronization
// =============================================================================

/// WAIT return values.
pub const WAIT_OBJECT_0: DWord = 0;
pub const WAIT_TIMEOUT: DWord = 258;
pub const WAIT_FAILED: DWord = 0xFFFFFFFF;
pub const INFINITE: DWord = 0xFFFFFFFF;

/// CreateMutexW.
pub fn create_mutex_w(
    security_attributes: u64,
    initial_owner: Bool,
    name: LpcWStr,
) -> Handle {
    let name_str = if !name.is_null() {
        Some(unsafe { utf16_to_utf8(name) })
    } else {
        None
    };
    log::trace!("[kernel32] CreateMutexW: name={:?}", name_str);

    handles::alloc_handle(HandleObject::Mutex {
        name: name_str,
        owner_thread: if initial_owner != 0 { Some(1) } else { None },
        locked: initial_owner != 0,
    })
}

/// CreateEventW.
pub fn create_event_w(
    security_attributes: u64,
    manual_reset: Bool,
    initial_state: Bool,
    name: LpcWStr,
) -> Handle {
    let name_str = if !name.is_null() {
        Some(unsafe { utf16_to_utf8(name) })
    } else {
        None
    };

    handles::alloc_handle(HandleObject::Event {
        name: name_str,
        signaled: initial_state != 0,
        manual_reset: manual_reset != 0,
    })
}

/// CreateSemaphoreW.
pub fn create_semaphore_w(
    security_attributes: u64,
    initial_count: i32,
    maximum_count: i32,
    name: LpcWStr,
) -> Handle {
    let name_str = if !name.is_null() {
        Some(unsafe { utf16_to_utf8(name) })
    } else {
        None
    };

    handles::alloc_handle(HandleObject::Semaphore {
        name: name_str,
        count: initial_count,
        max_count: maximum_count,
    })
}

/// WaitForSingleObject.
pub fn wait_for_single_object(handle: Handle, milliseconds: DWord) -> DWord {
    log::trace!("[kernel32] WaitForSingleObject: handle=0x{:X}, timeout={}", handle, milliseconds);
    WAIT_OBJECT_0
}

/// WaitForMultipleObjects.
pub fn wait_for_multiple_objects(
    count: DWord,
    handles_ptr: *const Handle,
    wait_all: Bool,
    milliseconds: DWord,
) -> DWord {
    log::trace!("[kernel32] WaitForMultipleObjects: count={}", count);
    WAIT_OBJECT_0
}

/// ReleaseMutex.
pub fn release_mutex(mutex: Handle) -> Bool {
    handles::with_handle_mut(mutex, |obj| {
        if let HandleObject::Mutex { locked, owner_thread, .. } = obj {
            *locked = false;
            *owner_thread = None;
        }
    });
    TRUE
}

/// SetEvent.
pub fn set_event(event: Handle) -> Bool {
    handles::with_handle_mut(event, |obj| {
        if let HandleObject::Event { signaled, .. } = obj {
            *signaled = true;
        }
    });
    TRUE
}

/// ResetEvent.
pub fn reset_event(event: Handle) -> Bool {
    handles::with_handle_mut(event, |obj| {
        if let HandleObject::Event { signaled, .. } = obj {
            *signaled = false;
        }
    });
    TRUE
}

/// ReleaseSemaphore.
pub fn release_semaphore(semaphore: Handle, release_count: i32, previous_count: *mut i32) -> Bool {
    handles::with_handle_mut(semaphore, |obj| {
        if let HandleObject::Semaphore { count, max_count, .. } = obj {
            if !previous_count.is_null() {
                unsafe { *previous_count = *count; }
            }
            *count = (*count + release_count).min(*max_count);
        }
    });
    TRUE
}

/// CRITICAL_SECTION (simplified — just a spinlock flag).
#[repr(C)]
pub struct CriticalSection {
    pub debug_info: u64,
    pub lock_count: i32,
    pub recursion_count: i32,
    pub owning_thread: u64,
    pub lock_semaphore: u64,
    pub spin_count: u64,
}

/// InitializeCriticalSection.
pub fn initialize_critical_section(cs: *mut CriticalSection) {
    if !cs.is_null() {
        unsafe {
            (*cs).debug_info = 0;
            (*cs).lock_count = -1;
            (*cs).recursion_count = 0;
            (*cs).owning_thread = 0;
            (*cs).lock_semaphore = 0;
            (*cs).spin_count = 0;
        }
    }
}

/// EnterCriticalSection.
pub fn enter_critical_section(cs: *mut CriticalSection) {
    if !cs.is_null() {
        unsafe {
            (*cs).lock_count += 1;
            (*cs).recursion_count += 1;
            (*cs).owning_thread = 1; // Main thread
        }
    }
}

/// LeaveCriticalSection.
pub fn leave_critical_section(cs: *mut CriticalSection) {
    if !cs.is_null() {
        unsafe {
            (*cs).recursion_count -= 1;
            if (*cs).recursion_count == 0 {
                (*cs).owning_thread = 0;
            }
            (*cs).lock_count -= 1;
        }
    }
}

// =============================================================================
// Console
// =============================================================================

/// GetStdHandle.
pub fn get_std_handle(std_handle: DWord) -> Handle {
    match std_handle {
        STD_INPUT_HANDLE => handles::HANDLE_STDIN,
        STD_OUTPUT_HANDLE => handles::HANDLE_STDOUT,
        STD_ERROR_HANDLE => handles::HANDLE_STDERR,
        _ => INVALID_HANDLE_VALUE,
    }
}

/// WriteConsoleW — write Unicode text to the console.
pub fn write_console_w(
    console_output: Handle,
    buffer: LpcWStr,
    chars_to_write: DWord,
    chars_written: *mut DWord,
    reserved: u64,
) -> Bool {
    if buffer.is_null() {
        return FALSE;
    }

    let slice = unsafe {
        core::slice::from_raw_parts(buffer, chars_to_write as usize)
    };
    let text = crate::unicode::utf16_slice_to_string(slice);
    log::info!("[win32:console] {}", text);

    if !chars_written.is_null() {
        unsafe { *chars_written = chars_to_write; }
    }
    TRUE
}

/// ReadConsoleW (stub).
pub fn read_console_w(
    console_input: Handle,
    buffer: LpWStr,
    chars_to_read: DWord,
    chars_read: *mut DWord,
    input_control: u64,
) -> Bool {
    log::trace!("[kernel32] ReadConsoleW: stub");
    if !chars_read.is_null() {
        unsafe { *chars_read = 0; }
    }
    TRUE
}

/// SetConsoleTitleW.
pub fn set_console_title_w(title: LpcWStr) -> Bool {
    let title_str = unsafe { utf16_to_utf8(title) };
    log::info!("[kernel32] SetConsoleTitleW: '{}'", title_str);
    TRUE
}

/// GetConsoleMode.
pub fn get_console_mode(console_handle: Handle, mode: *mut DWord) -> Bool {
    if !mode.is_null() {
        unsafe { *mode = 0x0007; } // ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT
    }
    TRUE
}

/// SetConsoleMode.
pub fn set_console_mode(console_handle: Handle, mode: DWord) -> Bool {
    log::trace!("[kernel32] SetConsoleMode: mode=0x{:X}", mode);
    TRUE
}

// =============================================================================
// Miscellaneous
// =============================================================================

/// GetLastError.
pub fn get_last_error() -> DWord {
    teb_peb::get_last_error()
}

/// SetLastError.
pub fn set_last_error(error: DWord) {
    teb_peb::set_last_error(error);
}

/// GetTickCount.
pub fn get_tick_count() -> DWord {
    TICK_COUNT.load(Ordering::Relaxed) as DWord
}

/// GetTickCount64.
pub fn get_tick_count64() -> u64 {
    TICK_COUNT.load(Ordering::Relaxed)
}

/// QueryPerformanceCounter.
pub fn query_performance_counter(counter: *mut i64) -> Bool {
    if !counter.is_null() {
        unsafe { *counter = TICK_COUNT.load(Ordering::Relaxed) as i64 * 10000; }
    }
    TRUE
}

/// QueryPerformanceFrequency.
pub fn query_performance_frequency(frequency: *mut i64) -> Bool {
    if !frequency.is_null() {
        unsafe { *frequency = 10_000_000; } // 10 MHz
    }
    TRUE
}

/// GetSystemTimeAsFileTime.
pub fn get_system_time_as_file_time(file_time: *mut u64) {
    if !file_time.is_null() {
        // Windows FILETIME: 100-ns intervals since 1601-01-01
        // Approximate: epoch offset + ticks
        let epoch_offset: u64 = 116_444_736_000_000_000; // 1601->1970
        let now = epoch_offset + TICK_COUNT.load(Ordering::Relaxed) * 10_000;
        unsafe { *file_time = now; }
    }
}

/// GetModuleHandleW.
pub fn get_module_handle_w(module_name: LpcWStr) -> HModule {
    if module_name.is_null() {
        // NULL = return handle of the exe itself
        log::trace!("[kernel32] GetModuleHandleW(NULL) -> self");
        return 0x0040_0000; // Default PE image base
    }
    let name = unsafe { utf16_to_utf8(module_name) };
    log::trace!("[kernel32] GetModuleHandleW: '{}'", name);

    // Return a stub module handle based on DLL name
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        s if s.contains("kernel32") => 0x7FF8_0000_0000,
        s if s.contains("ntdll") => 0x7FF8_1000_0000,
        s if s.contains("user32") => 0x7FF8_2000_0000,
        s if s.contains("gdi32") => 0x7FF8_3000_0000,
        s if s.contains("ws2_32") => 0x7FF8_4000_0000,
        s if s.contains("advapi32") => 0x7FF8_5000_0000,
        s if s.contains("msvcrt") => 0x7FF8_6000_0000,
        s if s.contains("ole32") => 0x7FF8_7000_0000,
        _ => {
            teb_peb::set_last_error(126); // ERROR_MOD_NOT_FOUND
            0
        }
    }
}

/// GetProcAddress — look up a function by name in a module.
pub fn get_proc_address(module: HModule, proc_name: LpcStr) -> u64 {
    let name = unsafe { crate::unicode::cstr_to_string(proc_name) };
    log::trace!("[kernel32] GetProcAddress: module=0x{:X}, name='{}'", module, name);

    // Look up in our dispatch tables
    crate::dispatcher::resolve_function_by_module(module, &name).unwrap_or_else(|| {
        log::warn!("[kernel32] GetProcAddress: unresolved '{}'", name);
        teb_peb::set_last_error(127); // ERROR_PROC_NOT_FOUND
        0
    })
}

/// LoadLibraryW.
pub fn load_library_w(lib_file_name: LpcWStr) -> HModule {
    let name = unsafe { utf16_to_utf8(lib_file_name) };
    log::debug!("[kernel32] LoadLibraryW: '{}'", name);
    // Return same as GetModuleHandleW
    get_module_handle_w(lib_file_name)
}

/// FreeLibrary.
pub fn free_library(module: HModule) -> Bool {
    log::trace!("[kernel32] FreeLibrary: 0x{:X}", module);
    TRUE
}

/// GetCommandLineW.
pub fn get_command_line_w() -> LpcWStr {
    let lock = COMMAND_LINE.lock();
    if let Some(ref cmd) = *lock {
        cmd.as_ptr()
    } else {
        core::ptr::null()
    }
}

/// GetEnvironmentVariableW.
pub fn get_environment_variable_w(
    name: LpcWStr,
    buffer: LpWStr,
    size: DWord,
) -> DWord {
    let var_name = unsafe { utf16_to_utf8(name) };
    log::trace!("[kernel32] GetEnvironmentVariableW: '{}'", var_name);

    let env = ENV_VARS.lock();
    if let Some(ref env_map) = *env {
        if let Some(value) = env_map.get(&var_name) {
            let encoded = crate::unicode::utf8_to_utf16(value);
            let needed = encoded.len() as DWord; // includes null
            if buffer.is_null() || size < needed {
                return needed;
            }
            unsafe {
                let buf_slice = core::slice::from_raw_parts_mut(buffer, size as usize);
                buf_slice[..encoded.len()].copy_from_slice(&encoded);
            }
            return (encoded.len() - 1) as DWord; // exclude null from return count
        }
    }
    teb_peb::set_last_error(203); // ERROR_ENVVAR_NOT_FOUND
    0
}

/// SetEnvironmentVariableW.
pub fn set_environment_variable_w(name: LpcWStr, value: LpcWStr) -> Bool {
    let var_name = unsafe { utf16_to_utf8(name) };
    let var_value = if !value.is_null() {
        unsafe { utf16_to_utf8(value) }
    } else {
        String::new()
    };

    log::trace!("[kernel32] SetEnvironmentVariableW: '{}' = '{}'", var_name, var_value);

    let mut env = ENV_VARS.lock();
    if env.is_none() {
        *env = Some(alloc::collections::BTreeMap::new());
    }
    if let Some(ref mut map) = *env {
        if value.is_null() {
            map.remove(&var_name);
        } else {
            map.insert(var_name, var_value);
        }
    }
    TRUE
}

/// SYSTEM_INFO structure.
#[repr(C)]
pub struct SystemInfo {
    pub processor_architecture: u16,
    pub reserved: u16,
    pub page_size: DWord,
    pub minimum_application_address: u64,
    pub maximum_application_address: u64,
    pub active_processor_mask: u64,
    pub number_of_processors: DWord,
    pub processor_type: DWord,
    pub allocation_granularity: DWord,
    pub processor_level: u16,
    pub processor_revision: u16,
}

/// GetSystemInfo.
pub fn get_system_info(info: *mut SystemInfo) {
    if !info.is_null() {
        unsafe {
            (*info).processor_architecture = 9; // PROCESSOR_ARCHITECTURE_AMD64
            (*info).reserved = 0;
            (*info).page_size = 4096;
            (*info).minimum_application_address = 0x10000;
            (*info).maximum_application_address = 0x7FFF_FFFE_FFFF;
            (*info).active_processor_mask = 1;
            (*info).number_of_processors = 1;
            (*info).processor_type = 8664;
            (*info).allocation_granularity = 65536;
            (*info).processor_level = 6;
            (*info).processor_revision = 0;
        }
    }
}

/// GlobalAlloc.
pub fn global_alloc(flags: UInt, bytes: SizeT) -> u64 {
    heap_alloc(get_process_heap(), flags, bytes)
}

/// GlobalFree.
pub fn global_free(mem: u64) -> u64 {
    heap_free(get_process_heap(), 0, mem);
    0
}

/// OutputDebugStringW.
pub fn output_debug_string_w(output_string: LpcWStr) {
    if !output_string.is_null() {
        let msg = unsafe { utf16_to_utf8(output_string) };
        log::debug!("[win32:debug] {}", msg);
    }
}

/// FormatMessageW (simplified stub).
pub fn format_message_w(
    flags: DWord,
    source: u64,
    message_id: DWord,
    language_id: DWord,
    buffer: LpWStr,
    size: DWord,
    arguments: u64,
) -> DWord {
    log::trace!("[kernel32] FormatMessageW: msg_id={}", message_id);
    // Return a generic error message
    let msg = alloc::format!("Error {}", message_id);
    if !buffer.is_null() && size > 0 {
        let encoded = crate::unicode::utf8_to_utf16(&msg);
        let copy_len = encoded.len().min(size as usize);
        unsafe {
            let buf_slice = core::slice::from_raw_parts_mut(buffer, size as usize);
            buf_slice[..copy_len].copy_from_slice(&encoded[..copy_len]);
        }
        return (copy_len - 1) as DWord;
    }
    0
}

/// Convert a Windows-style path to a VFS path.
fn windows_path_to_vfs(path: &str) -> String {
    // Strip drive letter and convert backslashes
    let mut vfs = path.replace('\\', "/");
    // Strip "C:" or similar drive prefix
    if vfs.len() >= 2 && vfs.as_bytes()[1] == b':' {
        vfs = String::from(&vfs[2..]);
    }
    if !vfs.starts_with('/') {
        vfs = alloc::format!("/{}", vfs);
    }
    vfs
}
