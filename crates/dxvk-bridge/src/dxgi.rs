//! DXGI (DirectX Graphics Infrastructure) to Vulkan translation.
//!
//! Implements IDXGIFactory, IDXGIAdapter, and IDXGISwapChain COM interfaces.
//! The swap chain presents to the ClaudioOS framebuffer via the Vulkan surface.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::com::*;

// =============================================================================
// DXGI GUIDs
// =============================================================================

/// IID_IDXGIFactory: {7B7166EC-21C7-44AE-B21A-C9AE321AE369}
pub const IID_IDXGIFACTORY: Guid = Guid {
    data1: 0x7B7166EC,
    data2: 0x21C7,
    data3: 0x44AE,
    data4: [0xB2, 0x1A, 0xC9, 0xAE, 0x32, 0x1A, 0xE3, 0x69],
};

/// IID_IDXGIAdapter: {2411E7E1-12AC-4CCF-BD14-9798E8534DC0}
pub const IID_IDXGIADAPTER: Guid = Guid {
    data1: 0x2411E7E1,
    data2: 0x12AC,
    data3: 0x4CCF,
    data4: [0xBD, 0x14, 0x97, 0x98, 0xE8, 0x53, 0x4D, 0xC0],
};

/// IID_IDXGISwapChain: {310D36A0-D2E7-4C0A-AA04-6A9D23B8886A}
pub const IID_IDXGISWAPCHAIN: Guid = Guid {
    data1: 0x310D36A0,
    data2: 0xD2E7,
    data3: 0x4C0A,
    data4: [0xAA, 0x04, 0x6A, 0x9D, 0x23, 0xB8, 0x88, 0x6A],
};

// =============================================================================
// DXGI types
// =============================================================================

/// DXGI_SWAP_CHAIN_DESC.
#[repr(C)]
pub struct DxgiSwapChainDesc {
    pub buffer_width: u32,
    pub buffer_height: u32,
    pub format: u32,           // DXGI_FORMAT
    pub refresh_rate_num: u32,
    pub refresh_rate_den: u32,
    pub buffer_usage: u32,     // DXGI_USAGE
    pub buffer_count: u32,
    pub output_window: u64,    // HWND
    pub windowed: i32,         // BOOL
    pub swap_effect: u32,      // DXGI_SWAP_EFFECT
    pub flags: u32,
}

/// DXGI_ADAPTER_DESC.
#[repr(C)]
pub struct DxgiAdapterDesc {
    pub description: [u16; 128],
    pub vendor_id: u32,
    pub device_id: u32,
    pub sub_sys_id: u32,
    pub revision: u32,
    pub dedicated_video_memory: u64,
    pub dedicated_system_memory: u64,
    pub shared_system_memory: u64,
    pub adapter_luid_low: u32,
    pub adapter_luid_high: i32,
}

/// DXGI_FORMAT (commonly used values).
pub const DXGI_FORMAT_UNKNOWN: u32 = 0;
pub const DXGI_FORMAT_R8G8B8A8_UNORM: u32 = 28;
pub const DXGI_FORMAT_R8G8B8A8_UNORM_SRGB: u32 = 29;
pub const DXGI_FORMAT_B8G8R8A8_UNORM: u32 = 87;
pub const DXGI_FORMAT_B8G8R8A8_UNORM_SRGB: u32 = 91;
pub const DXGI_FORMAT_R16G16B16A16_FLOAT: u32 = 10;
pub const DXGI_FORMAT_D24_UNORM_S8_UINT: u32 = 45;
pub const DXGI_FORMAT_D32_FLOAT: u32 = 40;

/// DXGI_SWAP_EFFECT.
pub const DXGI_SWAP_EFFECT_DISCARD: u32 = 0;
pub const DXGI_SWAP_EFFECT_SEQUENTIAL: u32 = 1;
pub const DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL: u32 = 3;
pub const DXGI_SWAP_EFFECT_FLIP_DISCARD: u32 = 4;

/// DXGI_PRESENT flags.
pub const DXGI_PRESENT_TEST: u32 = 0x00000001;
pub const DXGI_PRESENT_DO_NOT_SEQUENCE: u32 = 0x00000002;

// =============================================================================
// IDXGIFactory implementation
// =============================================================================

struct DxgiFactoryData {
    /// Enumerated adapters.
    adapters: Vec<*mut ComBase>,
}

unsafe extern "system" fn factory_qi(
    this: *mut ComBase,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if ppv.is_null() { return E_POINTER; }
    *ppv = core::ptr::null_mut();
    if riid.is_null() { return E_INVALIDARG; }

    let iid = unsafe { &*riid };
    if *iid == IID_IUNKNOWN || *iid == IID_IDXGIFACTORY {
        *ppv = this as *mut core::ffi::c_void;
        factory_add_ref(this);
        return S_OK;
    }
    E_NOINTERFACE
}

unsafe extern "system" fn factory_add_ref(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    (*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn factory_release(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    let prev = (*this).ref_count.fetch_sub(1, Ordering::Relaxed);
    if prev == 1 {
        if !(*this).impl_data.is_null() {
            drop(Box::from_raw((*this).impl_data as *mut DxgiFactoryData));
        }
        drop(Box::from_raw(this));
        0
    } else {
        prev - 1
    }
}

static DXGI_FACTORY_VTBL: IUnknownVtbl = IUnknownVtbl {
    query_interface: factory_qi,
    add_ref: factory_add_ref,
    release: factory_release,
};

// =============================================================================
// IDXGIAdapter implementation
// =============================================================================

struct DxgiAdapterData {
    index: u32,
    description: String,
    vendor_id: u32,
    device_id: u32,
    dedicated_video_memory: u64,
}

unsafe extern "system" fn adapter_qi(
    this: *mut ComBase,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if ppv.is_null() { return E_POINTER; }
    *ppv = core::ptr::null_mut();
    if riid.is_null() { return E_INVALIDARG; }

    let iid = unsafe { &*riid };
    if *iid == IID_IUNKNOWN || *iid == IID_IDXGIADAPTER {
        *ppv = this as *mut core::ffi::c_void;
        adapter_add_ref(this);
        return S_OK;
    }
    E_NOINTERFACE
}

unsafe extern "system" fn adapter_add_ref(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    (*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn adapter_release(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    let prev = (*this).ref_count.fetch_sub(1, Ordering::Relaxed);
    if prev == 1 {
        if !(*this).impl_data.is_null() {
            drop(Box::from_raw((*this).impl_data as *mut DxgiAdapterData));
        }
        drop(Box::from_raw(this));
        0
    } else {
        prev - 1
    }
}

static DXGI_ADAPTER_VTBL: IUnknownVtbl = IUnknownVtbl {
    query_interface: adapter_qi,
    add_ref: adapter_add_ref,
    release: adapter_release,
};

// =============================================================================
// IDXGISwapChain implementation
// =============================================================================

struct DxgiSwapChainData {
    device: *mut ComBase,
    factory: *mut ComBase,
    width: u32,
    height: u32,
    format: u32,
    buffer_count: u32,
    window: u64,
    /// Present counter.
    present_count: u64,
    /// Current back buffer index.
    current_buffer: u32,
}

unsafe extern "system" fn swapchain_qi(
    this: *mut ComBase,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if ppv.is_null() { return E_POINTER; }
    *ppv = core::ptr::null_mut();
    if riid.is_null() { return E_INVALIDARG; }

    let iid = unsafe { &*riid };
    if *iid == IID_IUNKNOWN || *iid == IID_IDXGISWAPCHAIN {
        *ppv = this as *mut core::ffi::c_void;
        swapchain_add_ref(this);
        return S_OK;
    }
    E_NOINTERFACE
}

unsafe extern "system" fn swapchain_add_ref(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    (*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn swapchain_release(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    let prev = (*this).ref_count.fetch_sub(1, Ordering::Relaxed);
    if prev == 1 {
        if !(*this).impl_data.is_null() {
            drop(Box::from_raw((*this).impl_data as *mut DxgiSwapChainData));
        }
        drop(Box::from_raw(this));
        0
    } else {
        prev - 1
    }
}

static DXGI_SWAPCHAIN_VTBL: IUnknownVtbl = IUnknownVtbl {
    query_interface: swapchain_qi,
    add_ref: swapchain_add_ref,
    release: swapchain_release,
};

// =============================================================================
// DXGI API entry points
// =============================================================================

/// CreateDXGIFactory — create a DXGI factory.
pub fn create_dxgi_factory(riid: *const Guid, factory_out: *mut *mut core::ffi::c_void) -> HResult {
    log::info!("[dxvk:dxgi] CreateDXGIFactory");

    if factory_out.is_null() {
        return E_INVALIDARG;
    }

    // Create a default adapter representing our Vulkan device
    let adapter_data = DxgiAdapterData {
        index: 0,
        description: String::from("ClaudioOS Vulkan Adapter"),
        vendor_id: 0x10DE,  // NVIDIA
        device_id: 0x2484,  // RTX 3070 Ti
        dedicated_video_memory: 8 * 1024 * 1024 * 1024, // 8 GB
    };
    let adapter = create_com_object_with_data(TYPE_DXGI_ADAPTER, &DXGI_ADAPTER_VTBL, adapter_data);

    let factory_data = DxgiFactoryData {
        adapters: alloc::vec![adapter],
    };

    let factory = create_com_object_with_data(TYPE_DXGI_FACTORY, &DXGI_FACTORY_VTBL, factory_data);
    unsafe { *factory_out = factory as *mut core::ffi::c_void; }

    S_OK
}

/// IDXGIFactory::EnumAdapters.
pub fn enum_adapters(
    factory: *mut ComBase,
    index: u32,
    adapter_out: *mut *mut ComBase,
) -> HResult {
    if factory.is_null() || adapter_out.is_null() {
        return E_INVALIDARG;
    }

    unsafe {
        if let Some(data) = get_impl_data::<DxgiFactoryData>(factory) {
            if (index as usize) < data.adapters.len() {
                let adapter = data.adapters[index as usize];
                // AddRef the adapter
                ((*(*adapter).vtbl).add_ref)(adapter);
                *adapter_out = adapter;
                return S_OK;
            }
        }
    }

    DXGI_ERROR_NOT_FOUND
}

/// IDXGIAdapter::GetDesc.
pub fn adapter_get_desc(adapter: *mut ComBase, desc: *mut DxgiAdapterDesc) -> HResult {
    if adapter.is_null() || desc.is_null() {
        return E_INVALIDARG;
    }

    unsafe {
        if let Some(data) = get_impl_data::<DxgiAdapterData>(adapter) {
            // Write description as UTF-16LE
            let utf16 = com_utf8_to_utf16(&data.description);
            let copy_len = utf16.len().min(127);
            let desc_ref = &mut *desc;
            desc_ref.description = [0u16; 128];
            desc_ref.description[..copy_len].copy_from_slice(&utf16[..copy_len]);

            desc_ref.vendor_id = data.vendor_id;
            desc_ref.device_id = data.device_id;
            desc_ref.sub_sys_id = 0;
            desc_ref.revision = 0;
            desc_ref.dedicated_video_memory = data.dedicated_video_memory;
            desc_ref.dedicated_system_memory = 0;
            desc_ref.shared_system_memory = 512 * 1024 * 1024; // 512 MB
            desc_ref.adapter_luid_low = 1;
            desc_ref.adapter_luid_high = 0;

            return S_OK;
        }
    }

    E_FAIL
}

/// IDXGIFactory::CreateSwapChain.
pub fn create_swap_chain(
    factory: *mut ComBase,
    device: *mut ComBase,
    desc: *const DxgiSwapChainDesc,
    swapchain_out: *mut *mut ComBase,
) -> HResult {
    if factory.is_null() || device.is_null() || desc.is_null() || swapchain_out.is_null() {
        return E_INVALIDARG;
    }

    let (width, height, format, buffer_count, window) = unsafe {
        ((*desc).buffer_width, (*desc).buffer_height, (*desc).format,
         (*desc).buffer_count, (*desc).output_window)
    };

    log::info!(
        "[dxvk:dxgi] CreateSwapChain: {}x{}, format={}, buffers={}, window=0x{:X}",
        width, height, format, buffer_count, window
    );

    // In a full implementation: create VkSwapchainKHR via claudio-vulkan
    let data = DxgiSwapChainData {
        device,
        factory,
        width,
        height,
        format,
        buffer_count,
        window,
        present_count: 0,
        current_buffer: 0,
    };

    let swapchain = create_com_object_with_data(TYPE_DXGI_SWAPCHAIN, &DXGI_SWAPCHAIN_VTBL, data);
    unsafe { *swapchain_out = swapchain; }

    S_OK
}

/// IDXGISwapChain::Present.
pub fn swap_chain_present(swapchain: *mut ComBase, sync_interval: u32, flags: u32) -> HResult {
    if swapchain.is_null() {
        return E_INVALIDARG;
    }

    unsafe {
        if let Some(data) = get_impl_data::<DxgiSwapChainData>(swapchain) {
            data.present_count += 1;
            data.current_buffer = (data.current_buffer + 1) % data.buffer_count.max(1);

            if data.present_count % 60 == 0 {
                log::trace!(
                    "[dxvk:dxgi] Present: frame={}, sync={}, flags=0x{:X}",
                    data.present_count, sync_interval, flags
                );
            }
        }
    }

    // In a full implementation: vkQueuePresentKHR
    S_OK
}

/// IDXGISwapChain::GetBuffer.
pub fn swap_chain_get_buffer(
    swapchain: *mut ComBase,
    buffer_index: u32,
    riid: *const Guid,
    surface_out: *mut *mut core::ffi::c_void,
) -> HResult {
    if swapchain.is_null() || surface_out.is_null() {
        return E_INVALIDARG;
    }

    log::trace!("[dxvk:dxgi] GetBuffer: index={}", buffer_index);

    // Return a texture COM object representing the swapchain buffer
    let tex = create_com_object(0, &DEFAULT_IUNKNOWN_VTBL);
    unsafe { *surface_out = tex as *mut core::ffi::c_void; }

    S_OK
}

// =============================================================================
// Initialization and helpers
// =============================================================================

/// Initialize DXGI subsystem.
pub fn init() {
    log::info!("[dxvk:dxgi] DXGI initialized");
}

/// Helper: convert UTF-8 to UTF-16 (no null terminator).
fn com_utf8_to_utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}
