//! COM (Component Object Model) infrastructure for DXVK.
//!
//! Provides IUnknown base implementation with reference counting,
//! QueryInterface dispatch, and GUID comparison utilities.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

/// GUID (Globally Unique Identifier) — same layout as Windows GUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub const ZERO: Guid = Guid { data1: 0, data2: 0, data3: 0, data4: [0; 8] };
}

/// IID_IUnknown: {00000000-0000-0000-C000-000000000046}
pub const IID_IUNKNOWN: Guid = Guid {
    data1: 0x00000000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

/// HRESULT type.
pub type HResult = i32;

pub const S_OK: HResult = 0;
pub const S_FALSE: HResult = 1;
pub const E_NOINTERFACE: HResult = 0x80004002_u32 as i32;
pub const E_POINTER: HResult = 0x80004003_u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007000E_u32 as i32;
pub const E_INVALIDARG: HResult = 0x80070057_u32 as i32;
pub const E_FAIL: HResult = 0x80004005_u32 as i32;
pub const E_NOTIMPL: HResult = 0x80004001_u32 as i32;
pub const DXGI_ERROR_NOT_FOUND: HResult = 0x887A0002_u32 as i32;
pub const DXGI_ERROR_DEVICE_REMOVED: HResult = 0x887A0005_u32 as i32;
pub const D3DERR_INVALIDCALL: HResult = 0x8876086C_u32 as i32;

// =============================================================================
// IUnknown vtable and COM object base
// =============================================================================

/// IUnknown virtual function table.
#[repr(C)]
pub struct IUnknownVtbl {
    pub query_interface: unsafe extern "system" fn(
        this: *mut ComBase,
        riid: *const Guid,
        ppv: *mut *mut core::ffi::c_void,
    ) -> HResult,
    pub add_ref: unsafe extern "system" fn(this: *mut ComBase) -> u32,
    pub release: unsafe extern "system" fn(this: *mut ComBase) -> u32,
}

/// Base layout for all COM objects. The first field is always a pointer to
/// the vtable, matching the Windows COM binary layout.
#[repr(C)]
pub struct ComBase {
    pub vtbl: *const IUnknownVtbl,
    pub ref_count: AtomicU32,
    /// Object type tag for internal dispatch.
    pub type_id: u32,
    /// Pointer to the actual implementation data (type-specific).
    pub impl_data: *mut core::ffi::c_void,
}

/// Type IDs for COM objects.
pub const TYPE_DXGI_FACTORY: u32 = 1;
pub const TYPE_DXGI_ADAPTER: u32 = 2;
pub const TYPE_DXGI_SWAPCHAIN: u32 = 3;
pub const TYPE_D3D9: u32 = 10;
pub const TYPE_D3D9_DEVICE: u32 = 11;
pub const TYPE_D3D9_VERTEX_BUFFER: u32 = 12;
pub const TYPE_D3D9_TEXTURE: u32 = 13;
pub const TYPE_D3D11_DEVICE: u32 = 20;
pub const TYPE_D3D11_CONTEXT: u32 = 21;
pub const TYPE_D3D11_BUFFER: u32 = 22;
pub const TYPE_D3D11_TEXTURE2D: u32 = 23;
pub const TYPE_D3D12_DEVICE: u32 = 30;
pub const TYPE_D3D12_COMMAND_QUEUE: u32 = 31;
pub const TYPE_D3D12_COMMAND_LIST: u32 = 32;

// =============================================================================
// Default IUnknown implementation
// =============================================================================

/// Default QueryInterface: supports only IUnknown.
unsafe extern "system" fn default_query_interface(
    this: *mut ComBase,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if ppv.is_null() {
        return E_POINTER;
    }
    *ppv = core::ptr::null_mut();

    if riid.is_null() {
        return E_INVALIDARG;
    }

    let iid = &*riid;

    if *iid == IID_IUNKNOWN {
        *ppv = this as *mut core::ffi::c_void;
        default_add_ref(this);
        return S_OK;
    }

    log::trace!(
        "[dxvk:com] QueryInterface miss: {:08X}-{:04X}-{:04X}",
        iid.data1, iid.data2, iid.data3
    );
    E_NOINTERFACE
}

/// Default AddRef.
unsafe extern "system" fn default_add_ref(this: *mut ComBase) -> u32 {
    if this.is_null() {
        return 0;
    }
    let prev = (*this).ref_count.fetch_add(1, Ordering::Relaxed);
    prev + 1
}

/// Default Release.
unsafe extern "system" fn default_release(this: *mut ComBase) -> u32 {
    if this.is_null() {
        return 0;
    }
    let prev = (*this).ref_count.fetch_sub(1, Ordering::Relaxed);
    if prev == 1 {
        // Last reference — clean up
        log::trace!("[dxvk:com] Release: destroying object type={}", (*this).type_id);
        // Drop the impl_data if it was heap-allocated
        // (In practice, each type would have its own Release that knows the layout)
        drop(Box::from_raw(this));
        0
    } else {
        prev - 1
    }
}

/// Static default IUnknown vtable.
pub static DEFAULT_IUNKNOWN_VTBL: IUnknownVtbl = IUnknownVtbl {
    query_interface: default_query_interface,
    add_ref: default_add_ref,
    release: default_release,
};

// =============================================================================
// COM object allocation
// =============================================================================

/// Allocate a new COM object with default IUnknown vtable.
pub fn create_com_object(type_id: u32, vtbl: &'static IUnknownVtbl) -> *mut ComBase {
    let obj = Box::new(ComBase {
        vtbl: vtbl as *const IUnknownVtbl,
        ref_count: AtomicU32::new(1),
        type_id,
        impl_data: core::ptr::null_mut(),
    });
    let ptr = Box::into_raw(obj);
    log::trace!("[dxvk:com] Created COM object type={} at 0x{:X}", type_id, ptr as u64);
    ptr
}

/// Allocate a COM object with implementation data.
pub fn create_com_object_with_data<T>(
    type_id: u32,
    vtbl: &'static IUnknownVtbl,
    data: T,
) -> *mut ComBase {
    let data_ptr = Box::into_raw(Box::new(data));
    let obj = Box::new(ComBase {
        vtbl: vtbl as *const IUnknownVtbl,
        ref_count: AtomicU32::new(1),
        type_id,
        impl_data: data_ptr as *mut core::ffi::c_void,
    });
    let ptr = Box::into_raw(obj);
    log::trace!("[dxvk:com] Created COM object type={} at 0x{:X} with data", type_id, ptr as u64);
    ptr
}

/// Get a reference to the implementation data of a COM object.
///
/// # Safety
/// The caller must ensure `T` matches the type stored in `impl_data`.
pub unsafe fn get_impl_data<T>(obj: *mut ComBase) -> Option<&'static mut T> {
    if obj.is_null() {
        return None;
    }
    let data = (*obj).impl_data;
    if data.is_null() {
        return None;
    }
    Some(&mut *(data as *mut T))
}

// =============================================================================
// GUID comparison utilities
// =============================================================================

/// Compare two GUIDs for equality.
pub fn guid_eq(a: &Guid, b: &Guid) -> bool {
    a == b
}

/// Format a GUID as a string for debug logging.
pub fn guid_to_string(g: &Guid) -> alloc::string::String {
    alloc::format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1, g.data2, g.data3,
        g.data4[0], g.data4[1], g.data4[2], g.data4[3],
        g.data4[4], g.data4[5], g.data4[6], g.data4[7]
    )
}

// =============================================================================
// Initialization
// =============================================================================

/// Global COM object tracking (for debugging / leak detection).
static LIVE_OBJECTS: Mutex<u64> = Mutex::new(0);

/// Initialize COM infrastructure.
pub fn init() {
    *LIVE_OBJECTS.lock() = 0;
    log::info!("[dxvk:com] COM infrastructure initialized");
}

/// Get the count of live COM objects.
pub fn live_object_count() -> u64 {
    *LIVE_OBJECTS.lock()
}
