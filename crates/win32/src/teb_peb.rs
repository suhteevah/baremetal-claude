//! Thread Environment Block (TEB) and Process Environment Block (PEB).
//!
//! On Windows, every thread has a TEB accessible via the GS segment register
//! (GS:[0x30] = self pointer on x64). The PEB is process-wide, accessible via
//! TEB.ProcessEnvironmentBlock (offset 0x60).
//!
//! We allocate these structures on the heap and set the GS base to point to
//! the TEB using WRMSR on IA32_GS_BASE.

use alloc::boxed::Box;
use alloc::vec;
use spin::Mutex;

/// IA32_GS_BASE MSR address.
const IA32_GS_BASE: u32 = 0xC000_0101;

/// Thread Environment Block (simplified for Win64).
///
/// Real Windows TEB is ~4KB. We implement the fields that programs actually read.
#[repr(C)]
pub struct Teb {
    /// Offset 0x00: SEH exception list (not used).
    pub exception_list: u64,
    /// Offset 0x08: Stack base (high address).
    pub stack_base: u64,
    /// Offset 0x10: Stack limit (low address).
    pub stack_limit: u64,
    /// Offset 0x18: SubSystemTib.
    pub sub_system_tib: u64,
    /// Offset 0x20: Fiber data / version.
    pub fiber_data: u64,
    /// Offset 0x28: Arbitrary data pointer.
    pub arbitrary_user_pointer: u64,
    /// Offset 0x30: Self pointer (TEB address — read via GS:[0x30]).
    pub teb_self: u64,
    /// Offset 0x38: Environment pointer.
    pub environment_pointer: u64,
    /// Offset 0x40: Process ID.
    pub process_id: u64,
    /// Offset 0x48: Thread ID.
    pub thread_id: u64,
    /// Offset 0x50: Active RPC handle.
    pub rpc_handle: u64,
    /// Offset 0x58: Thread-local storage pointer.
    pub tls_pointer: u64,
    /// Offset 0x60: Pointer to PEB.
    pub peb: u64,
    /// Offset 0x68: Last error value.
    pub last_error_value: u32,
    /// Padding to keep alignment.
    pub _pad1: u32,
    /// Offset 0x70: Count of owned critical sections.
    pub count_of_owned_critical_sections: u32,
    pub _pad2: u32,
    /// Padding up to TLS expansion slots.
    pub _reserved: [u8; 0x1700],
    /// Offset 0x1780 (approx): TLS slots (64 slots).
    pub tls_slots: [u64; 64],
}

/// Process Environment Block (simplified for Win64).
#[repr(C)]
pub struct Peb {
    /// Offset 0x00: InheritedAddressSpace.
    pub inherited_address_space: u8,
    /// Offset 0x01: ReadImageFileExecOptions.
    pub read_image_file_exec_options: u8,
    /// Offset 0x02: BeingDebugged.
    pub being_debugged: u8,
    /// Offset 0x03: BitField.
    pub bit_field: u8,
    /// Padding.
    pub _pad1: [u8; 4],
    /// Offset 0x08: Mutant (not used).
    pub mutant: u64,
    /// Offset 0x10: ImageBaseAddress.
    pub image_base_address: u64,
    /// Offset 0x18: Pointer to PEB_LDR_DATA.
    pub ldr: u64,
    /// Offset 0x20: Process parameters.
    pub process_parameters: u64,
    /// Offset 0x28: SubSystemData.
    pub sub_system_data: u64,
    /// Offset 0x30: ProcessHeap.
    pub process_heap: u64,
    /// Padding for remaining PEB fields.
    pub _reserved: [u8; 0x200],
    /// Offset (approx 0x238): Number of processors.
    pub number_of_processors: u32,
    /// OS version info.
    pub os_major_version: u32,
    pub os_minor_version: u32,
    pub os_build_number: u16,
    pub os_platform_id: u16,
}

/// Wrapper to make raw pointers Send for use in Mutex.
struct SendPtr<T>(*mut T);
unsafe impl<T> Send for SendPtr<T> {}

/// Global TEB and PEB pointers.
static TEB_PTR: Mutex<Option<SendPtr<Teb>>> = Mutex::new(None);
static PEB_PTR: Mutex<Option<SendPtr<Peb>>> = Mutex::new(None);

/// Allocate and initialize the TEB and PEB for the main thread of a Windows process.
///
/// Sets up the GS segment base to point to the TEB so that GS:[0x30] returns
/// the TEB address (standard Windows x64 TEB self-pointer).
///
/// # Arguments
/// * `image_base` — Base address of the loaded PE image.
/// * `stack_base` — Top of the thread stack.
/// * `stack_limit` — Bottom of the thread stack.
pub fn init(image_base: u64, stack_base: u64, stack_limit: u64) {
    log::info!("[win32:teb-peb] Setting up TEB/PEB for Win32 process");

    // Allocate PEB
    let peb = Box::new(Peb {
        inherited_address_space: 0,
        read_image_file_exec_options: 0,
        being_debugged: 0,
        bit_field: 0,
        _pad1: [0; 4],
        mutant: u64::MAX,
        image_base_address: image_base,
        ldr: 0, // Would point to PEB_LDR_DATA in a full implementation
        process_parameters: 0,
        sub_system_data: 0,
        process_heap: 0, // Set by HeapCreate
        _reserved: [0; 0x200],
        number_of_processors: 1,
        os_major_version: 10,
        os_minor_version: 0,
        os_build_number: 19045, // Windows 10 22H2
        os_platform_id: 2,     // VER_PLATFORM_WIN32_NT
    });
    let peb_ptr = Box::into_raw(peb);

    // Allocate TEB
    let teb = Box::new(Teb {
        exception_list: u64::MAX, // No SEH
        stack_base,
        stack_limit,
        sub_system_tib: 0,
        fiber_data: 0,
        arbitrary_user_pointer: 0,
        teb_self: 0, // Will be set below
        environment_pointer: 0,
        process_id: 1,
        thread_id: 1,
        rpc_handle: 0,
        tls_pointer: 0,
        peb: peb_ptr as u64,
        last_error_value: 0,
        _pad1: 0,
        count_of_owned_critical_sections: 0,
        _pad2: 0,
        _reserved: [0; 0x1700],
        tls_slots: [0; 64],
    });
    let teb_ptr = Box::into_raw(teb);

    // Set the self-pointer
    unsafe {
        (*teb_ptr).teb_self = teb_ptr as u64;
    }

    // Store globally
    *TEB_PTR.lock() = Some(SendPtr(teb_ptr));
    *PEB_PTR.lock() = Some(SendPtr(peb_ptr));

    // Set GS base to point to TEB
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let teb_addr = teb_ptr as u64;
        let low = teb_addr as u32;
        let high = (teb_addr >> 32) as u32;
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_GS_BASE,
            in("eax") low,
            in("edx") high,
            options(nomem, nostack, preserves_flags)
        );
    }

    log::info!(
        "[win32:teb-peb] TEB at 0x{:X}, PEB at 0x{:X}, GS base set",
        teb_ptr as u64, peb_ptr as u64
    );
}

/// Get the current TEB pointer.
pub fn get_teb() -> Option<*mut Teb> {
    TEB_PTR.lock().as_ref().map(|p| p.0)
}

/// Get the current PEB pointer.
pub fn get_peb() -> Option<*mut Peb> {
    PEB_PTR.lock().as_ref().map(|p| p.0)
}

/// Get the last error value from the TEB.
pub fn get_last_error() -> u32 {
    if let Some(teb) = get_teb() {
        unsafe { (*teb).last_error_value }
    } else {
        0
    }
}

/// Set the last error value in the TEB.
pub fn set_last_error(error: u32) {
    if let Some(teb) = get_teb() {
        unsafe {
            (*teb).last_error_value = error;
        }
    }
}

/// Clean up TEB and PEB allocations.
pub fn cleanup() {
    let teb = TEB_PTR.lock().take();
    let peb = PEB_PTR.lock().take();

    if let Some(SendPtr(teb)) = teb {
        unsafe { drop(Box::from_raw(teb)); }
    }
    if let Some(SendPtr(peb)) = peb {
        unsafe { drop(Box::from_raw(peb)); }
    }

    log::info!("[win32:teb-peb] TEB/PEB cleaned up");
}
