//! Win32 handle table.
//!
//! Windows uses opaque HANDLE values to reference kernel objects (files, threads,
//! mutexes, events, etc.). This module implements a simple handle table that maps
//! HANDLE values to their underlying objects.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::unicode::Handle;

/// Maximum number of handles per process.
const MAX_HANDLES: usize = 65536;

/// Predefined handle values.
pub const HANDLE_STDIN: Handle = 0x0000_0001;
pub const HANDLE_STDOUT: Handle = 0x0000_0002;
pub const HANDLE_STDERR: Handle = 0x0000_0003;

/// The kind of kernel object a handle refers to.
#[derive(Debug, Clone)]
pub enum HandleObject {
    /// File handle — wraps a VFS file descriptor.
    File {
        path: String,
        fd: u64,
        access: u32,
    },
    /// Console handle (stdin, stdout, stderr).
    Console {
        std_type: StdType,
    },
    /// Thread handle.
    Thread {
        thread_id: u32,
    },
    /// Process handle.
    Process {
        process_id: u32,
        exit_code: Option<u32>,
    },
    /// Mutex.
    Mutex {
        name: Option<String>,
        owner_thread: Option<u32>,
        locked: bool,
    },
    /// Event.
    Event {
        name: Option<String>,
        signaled: bool,
        manual_reset: bool,
    },
    /// Semaphore.
    Semaphore {
        name: Option<String>,
        count: i32,
        max_count: i32,
    },
    /// Registry key.
    RegistryKey {
        path: String,
    },
    /// Memory-mapped section.
    Section {
        base: u64,
        size: u64,
    },
    /// Heap handle.
    Heap {
        id: u32,
    },
    /// Module (DLL).
    Module {
        name: String,
        base: u64,
    },
    /// Find file search handle.
    FindFile {
        pattern: String,
        results: Vec<String>,
        index: usize,
    },
    /// GDI Device Context.
    DeviceContext {
        window: Handle,
    },
    /// Window handle.
    Window {
        class_name: String,
        title: String,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        visible: bool,
        parent: Handle,
    },
    /// Socket.
    Socket {
        family: i32,
        sock_type: i32,
        protocol: i32,
        bound: bool,
        connected: bool,
    },
}

/// Standard I/O types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdType {
    Input,
    Output,
    Error,
}

/// Global handle table.
static HANDLE_TABLE: Mutex<HandleTable> = Mutex::new(HandleTable::new());

/// Handle allocation table.
pub struct HandleTable {
    objects: BTreeMap<u64, HandleObject>,
    next_handle: u64,
}

impl HandleTable {
    const fn new() -> Self {
        Self {
            objects: BTreeMap::new(),
            next_handle: 0x100, // Start after predefined handles
        }
    }
}

/// Initialize the handle table with predefined handles.
pub fn init() {
    let mut table = HANDLE_TABLE.lock();
    table.objects.insert(HANDLE_STDIN, HandleObject::Console { std_type: StdType::Input });
    table.objects.insert(HANDLE_STDOUT, HandleObject::Console { std_type: StdType::Output });
    table.objects.insert(HANDLE_STDERR, HandleObject::Console { std_type: StdType::Error });
    log::info!("[win32:handles] Handle table initialized with stdin/stdout/stderr");
}

/// Allocate a new handle for the given object.
pub fn alloc_handle(object: HandleObject) -> Handle {
    let mut table = HANDLE_TABLE.lock();
    let handle = table.next_handle;
    table.next_handle += 4; // Handles are aligned to 4 (Windows convention)
    if table.objects.len() < MAX_HANDLES {
        table.objects.insert(handle, object);
        log::trace!("[win32:handles] Allocated handle 0x{:X}", handle);
        handle
    } else {
        log::error!("[win32:handles] Handle table exhausted");
        crate::unicode::INVALID_HANDLE_VALUE
    }
}

/// Look up a handle object.
pub fn get_handle(handle: Handle) -> Option<HandleObject> {
    let table = HANDLE_TABLE.lock();
    table.objects.get(&handle).cloned()
}

/// Modify a handle object in-place.
pub fn with_handle_mut<F, R>(handle: Handle, f: F) -> Option<R>
where
    F: FnOnce(&mut HandleObject) -> R,
{
    let mut table = HANDLE_TABLE.lock();
    table.objects.get_mut(&handle).map(f)
}

/// Close (deallocate) a handle.
pub fn close_handle(handle: Handle) -> bool {
    let mut table = HANDLE_TABLE.lock();
    if table.objects.remove(&handle).is_some() {
        log::trace!("[win32:handles] Closed handle 0x{:X}", handle);
        true
    } else {
        log::warn!("[win32:handles] Close: invalid handle 0x{:X}", handle);
        false
    }
}

/// Duplicate a handle (creates a new handle pointing to the same object).
pub fn duplicate_handle(src: Handle) -> Handle {
    let obj = {
        let table = HANDLE_TABLE.lock();
        table.objects.get(&src).cloned()
    };
    if let Some(obj) = obj {
        alloc_handle(obj)
    } else {
        crate::unicode::INVALID_HANDLE_VALUE
    }
}

/// Get the total number of allocated handles.
pub fn handle_count() -> usize {
    HANDLE_TABLE.lock().objects.len()
}
