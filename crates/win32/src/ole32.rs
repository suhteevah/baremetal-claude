//! Ole32.dll API implementation — COM runtime basics.
//!
//! Provides minimal COM infrastructure: CoInitialize, CoUninitialize,
//! CoCreateInstance, and the IUnknown base interface.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

use crate::unicode::*;
use crate::teb_peb;

// =============================================================================
// COM initialization
// =============================================================================

/// COM threading model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CoinitFlags {
    ApartmentThreaded = 0x2,
    MultiThreaded = 0x0,
    DisableOle1Dde = 0x4,
    SpeedOverMemory = 0x8,
}

/// COM initialization state.
static COM_INIT_COUNT: Mutex<u32> = Mutex::new(0);
static COM_THREADING: Mutex<u32> = Mutex::new(0);

/// HRESULT type.
pub type HResult = i32;

/// Common HRESULT values.
pub const S_OK: HResult = 0;
pub const S_FALSE: HResult = 1;
pub const E_NOINTERFACE: HResult = 0x80004002_u32 as i32;
pub const E_POINTER: HResult = 0x80004003_u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007000E_u32 as i32;
pub const E_INVALIDARG: HResult = 0x80070057_u32 as i32;
pub const E_FAIL: HResult = 0x80004005_u32 as i32;
pub const E_NOTIMPL: HResult = 0x80004001_u32 as i32;
pub const CO_E_NOTINITIALIZED: HResult = 0x800401F0_u32 as i32;
pub const CLASS_E_CLASSNOTAVAILABLE: HResult = 0x80040111_u32 as i32;
pub const REGDB_E_CLASSNOTREG: HResult = 0x80040154_u32 as i32;

// =============================================================================
// IUnknown
// =============================================================================

/// IUnknown GUID: {00000000-0000-0000-C000-000000000046}
pub const IID_IUNKNOWN: Guid = Guid {
    data1: 0x00000000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

/// IUnknown vtable layout.
#[repr(C)]
pub struct IUnknownVtbl {
    pub query_interface: extern "system" fn(*mut IUnknown, *const Guid, *mut *mut core::ffi::c_void) -> HResult,
    pub add_ref: extern "system" fn(*mut IUnknown) -> u32,
    pub release: extern "system" fn(*mut IUnknown) -> u32,
}

/// IUnknown base COM object.
#[repr(C)]
pub struct IUnknown {
    pub vtbl: *const IUnknownVtbl,
}

/// A simple reference-counted COM object wrapper.
pub struct ComObject {
    ref_count: u32,
    clsid: Guid,
    /// Supported interface IDs.
    interfaces: Vec<Guid>,
}

/// Default IUnknown::QueryInterface implementation.
extern "system" fn default_query_interface(
    this: *mut IUnknown,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if ppv.is_null() {
        return E_POINTER;
    }

    unsafe { *ppv = core::ptr::null_mut(); }

    if riid.is_null() {
        return E_INVALIDARG;
    }

    let iid = unsafe { &*riid };
    log::trace!(
        "[ole32] QueryInterface: {:08X}-{:04X}-{:04X}",
        iid.data1, iid.data2, iid.data3
    );

    // Always support IUnknown
    if *iid == IID_IUNKNOWN {
        unsafe {
            *ppv = this as *mut core::ffi::c_void;
        }
        default_add_ref(this);
        return S_OK;
    }

    E_NOINTERFACE
}

/// Default IUnknown::AddRef implementation.
extern "system" fn default_add_ref(this: *mut IUnknown) -> u32 {
    // In our simplified model, we don't track per-object ref counts
    // through the vtable pointer. Return a fixed count.
    1
}

/// Default IUnknown::Release implementation.
extern "system" fn default_release(this: *mut IUnknown) -> u32 {
    0
}

/// Static default IUnknown vtable.
static DEFAULT_VTBL: IUnknownVtbl = IUnknownVtbl {
    query_interface: default_query_interface,
    add_ref: default_add_ref,
    release: default_release,
};

// =============================================================================
// COM API
// =============================================================================

/// CoInitialize — initialize the COM library (single-threaded apartment).
pub fn co_initialize(reserved: u64) -> HResult {
    co_initialize_ex(reserved, CoinitFlags::ApartmentThreaded as u32)
}

/// CoInitializeEx — initialize COM with explicit threading model.
pub fn co_initialize_ex(reserved: u64, coinit: u32) -> HResult {
    let mut count = COM_INIT_COUNT.lock();

    if *count == 0 {
        *COM_THREADING.lock() = coinit;
        log::info!("[ole32] CoInitializeEx: threading=0x{:X}", coinit);
    } else {
        // Already initialized — check for threading model mismatch
        let current = *COM_THREADING.lock();
        if current != coinit {
            log::warn!("[ole32] CoInitializeEx: threading model mismatch (was 0x{:X}, requested 0x{:X})", current, coinit);
        }
    }

    *count += 1;

    if *count == 1 { S_OK } else { S_FALSE }
}

/// CoUninitialize — uninitialize the COM library.
pub fn co_uninitialize() {
    let mut count = COM_INIT_COUNT.lock();

    if *count > 0 {
        *count -= 1;
        if *count == 0 {
            log::info!("[ole32] CoUninitialize: COM shut down");
        }
    } else {
        log::warn!("[ole32] CoUninitialize: called without matching CoInitialize");
    }
}

/// CLSCTX flags.
pub const CLSCTX_INPROC_SERVER: DWord = 0x1;
pub const CLSCTX_INPROC_HANDLER: DWord = 0x2;
pub const CLSCTX_LOCAL_SERVER: DWord = 0x4;
pub const CLSCTX_ALL: DWord = CLSCTX_INPROC_SERVER | CLSCTX_INPROC_HANDLER | CLSCTX_LOCAL_SERVER;

/// CoCreateInstance — create a COM object by CLSID.
pub fn co_create_instance(
    clsid: *const Guid,
    outer: *mut IUnknown,
    cls_context: DWord,
    iid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if clsid.is_null() || iid.is_null() || ppv.is_null() {
        return E_INVALIDARG;
    }

    // Check COM is initialized
    if *COM_INIT_COUNT.lock() == 0 {
        return CO_E_NOTINITIALIZED;
    }

    let cls = unsafe { &*clsid };
    let riid = unsafe { &*iid };

    log::debug!(
        "[ole32] CoCreateInstance: CLSID={{{:08X}-{:04X}-{:04X}}}, IID={{{:08X}-{:04X}-{:04X}}}",
        cls.data1, cls.data2, cls.data3,
        riid.data1, riid.data2, riid.data3
    );

    // We don't have any registered COM servers in our bare-metal environment.
    // The DXVK bridge can register its own factory classes separately.
    unsafe { *ppv = core::ptr::null_mut(); }
    REGDB_E_CLASSNOTREG
}

/// Helper: compare two GUIDs.
pub fn guid_eq(a: &Guid, b: &Guid) -> bool {
    a.data1 == b.data1
        && a.data2 == b.data2
        && a.data3 == b.data3
        && a.data4 == b.data4
}
