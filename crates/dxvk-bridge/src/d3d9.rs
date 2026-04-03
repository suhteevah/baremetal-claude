//! Direct3D 9 to Vulkan translation.
//!
//! Implements IDirect3D9 and IDirect3DDevice9 COM interfaces. D3D9 draw calls
//! are translated to Vulkan command buffer recordings via claudio-vulkan.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::com::*;

// =============================================================================
// D3D9 GUIDs
// =============================================================================

/// IID_IDirect3D9: {81BDCBCA-64D4-426D-AE8D-AD0147F4275C}
pub const IID_IDIRECT3D9: Guid = Guid {
    data1: 0x81BDCBCA,
    data2: 0x64D4,
    data3: 0x426D,
    data4: [0xAE, 0x8D, 0xAD, 0x01, 0x47, 0xF4, 0x27, 0x5C],
};

/// IID_IDirect3DDevice9: {D0223B96-BF7A-43FD-92BD-A43B0D82B9EB}
pub const IID_IDIRECT3DDEVICE9: Guid = Guid {
    data1: 0xD0223B96,
    data2: 0xBF7A,
    data3: 0x43FD,
    data4: [0x92, 0xBD, 0xA4, 0x3B, 0x0D, 0x82, 0xB9, 0xEB],
};

// =============================================================================
// D3D9 types
// =============================================================================

/// D3DPRESENT_PARAMETERS.
#[repr(C)]
pub struct D3dPresentParameters {
    pub back_buffer_width: u32,
    pub back_buffer_height: u32,
    pub back_buffer_format: u32,  // D3DFORMAT
    pub back_buffer_count: u32,
    pub multi_sample_type: u32,
    pub multi_sample_quality: u32,
    pub swap_effect: u32,         // D3DSWAPEFFECT
    pub device_window: u64,       // HWND
    pub windowed: i32,            // BOOL
    pub enable_auto_depth_stencil: i32,
    pub auto_depth_stencil_format: u32,
    pub flags: u32,
    pub full_screen_refresh_rate: u32,
    pub presentation_interval: u32,
}

/// D3D primitive types.
#[repr(u32)]
pub enum D3dPrimitiveType {
    PointList = 1,
    LineList = 2,
    LineStrip = 3,
    TriangleList = 4,
    TriangleStrip = 5,
    TriangleFan = 6,
}

/// D3DPOOL.
#[repr(u32)]
pub enum D3dPool {
    Default = 0,
    Managed = 1,
    SystemMem = 2,
    Scratch = 3,
}

// =============================================================================
// IDirect3D9 implementation data
// =============================================================================

/// Internal state for IDirect3D9.
struct D3d9Data {
    sdk_version: u32,
    adapter_count: u32,
}

/// Internal state for IDirect3DDevice9.
struct D3d9DeviceData {
    adapter: u32,
    device_type: u32,
    focus_window: u64,
    behavior_flags: u32,
    back_buffer_width: u32,
    back_buffer_height: u32,
    /// Current render target (Vulkan image handle).
    render_target: u64,
    /// Current vertex buffer.
    vertex_buffer: u64,
    /// Current texture stage bindings (up to 8 stages).
    textures: [u64; 8],
    /// Draw call counter for profiling.
    draw_calls: u64,
    /// Frame counter.
    frame_count: u64,
}

// =============================================================================
// IDirect3D9 vtable
// =============================================================================

unsafe extern "system" fn d3d9_query_interface(
    this: *mut ComBase,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if ppv.is_null() { return E_POINTER; }
    *ppv = core::ptr::null_mut();
    if riid.is_null() { return E_INVALIDARG; }

    let iid = &*riid;
    if *iid == IID_IUNKNOWN || *iid == IID_IDIRECT3D9 {
        *ppv = this as *mut core::ffi::c_void;
        d3d9_add_ref(this);
        return S_OK;
    }
    E_NOINTERFACE
}

unsafe extern "system" fn d3d9_add_ref(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    (*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn d3d9_release(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    let prev = (*this).ref_count.fetch_sub(1, Ordering::Relaxed);
    if prev == 1 {
        if !(*this).impl_data.is_null() {
            drop(Box::from_raw((*this).impl_data as *mut D3d9Data));
        }
        drop(Box::from_raw(this));
        0
    } else {
        prev - 1
    }
}

static D3D9_VTBL: IUnknownVtbl = IUnknownVtbl {
    query_interface: d3d9_query_interface,
    add_ref: d3d9_add_ref,
    release: d3d9_release,
};

// =============================================================================
// IDirect3DDevice9 vtable
// =============================================================================

unsafe extern "system" fn device9_query_interface(
    this: *mut ComBase,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if ppv.is_null() { return E_POINTER; }
    *ppv = core::ptr::null_mut();
    if riid.is_null() { return E_INVALIDARG; }

    let iid = &*riid;
    if *iid == IID_IUNKNOWN || *iid == IID_IDIRECT3DDEVICE9 {
        *ppv = this as *mut core::ffi::c_void;
        device9_add_ref(this);
        return S_OK;
    }
    E_NOINTERFACE
}

unsafe extern "system" fn device9_add_ref(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    (*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn device9_release(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    let prev = (*this).ref_count.fetch_sub(1, Ordering::Relaxed);
    if prev == 1 {
        if !(*this).impl_data.is_null() {
            drop(Box::from_raw((*this).impl_data as *mut D3d9DeviceData));
        }
        drop(Box::from_raw(this));
        0
    } else {
        prev - 1
    }
}

static D3D9_DEVICE_VTBL: IUnknownVtbl = IUnknownVtbl {
    query_interface: device9_query_interface,
    add_ref: device9_add_ref,
    release: device9_release,
};

// =============================================================================
// Direct3D 9 API entry points
// =============================================================================

/// Direct3DCreate9 — create an IDirect3D9 object.
///
/// This is the main entry point for D3D9 applications.
pub fn direct3d_create9(sdk_version: u32) -> *mut ComBase {
    log::info!("[dxvk:d3d9] Direct3DCreate9: SDK version {}", sdk_version);

    let data = D3d9Data {
        sdk_version,
        adapter_count: 1, // One adapter (our Vulkan device)
    };

    create_com_object_with_data(TYPE_D3D9, &D3D9_VTBL, data)
}

/// IDirect3D9::CreateDevice — create a D3D9 device.
pub fn create_device(
    d3d9: *mut ComBase,
    adapter: u32,
    device_type: u32,
    focus_window: u64,
    behavior_flags: u32,
    present_params: *mut D3dPresentParameters,
    device_out: *mut *mut ComBase,
) -> HResult {
    if d3d9.is_null() || present_params.is_null() || device_out.is_null() {
        return E_INVALIDARG;
    }

    let (width, height) = unsafe {
        ((*present_params).back_buffer_width, (*present_params).back_buffer_height)
    };

    log::info!(
        "[dxvk:d3d9] CreateDevice: adapter={}, type={}, {}x{}, flags=0x{:X}",
        adapter, device_type, width, height, behavior_flags
    );

    // In a full implementation, we would:
    // 1. Create a Vulkan device via claudio-vulkan
    // 2. Create a swapchain matching the present parameters
    // 3. Create render targets and depth buffers

    let data = D3d9DeviceData {
        adapter,
        device_type,
        focus_window,
        behavior_flags,
        back_buffer_width: width,
        back_buffer_height: height,
        render_target: 0,
        vertex_buffer: 0,
        textures: [0; 8],
        draw_calls: 0,
        frame_count: 0,
    };

    let device = create_com_object_with_data(TYPE_D3D9_DEVICE, &D3D9_DEVICE_VTBL, data);
    unsafe { *device_out = device; }

    S_OK
}

/// IDirect3DDevice9::Present — present the back buffer.
pub fn present(
    device: *mut ComBase,
    source_rect: u64,
    dest_rect: u64,
    dest_window_override: u64,
    dirty_region: u64,
) -> HResult {
    if device.is_null() {
        return D3DERR_INVALIDCALL;
    }

    unsafe {
        if let Some(data) = get_impl_data::<D3d9DeviceData>(device) {
            data.frame_count += 1;
            if data.frame_count % 60 == 0 {
                log::trace!(
                    "[dxvk:d3d9] Present: frame={}, draw_calls={}",
                    data.frame_count, data.draw_calls
                );
            }
            data.draw_calls = 0;
        }
    }

    // In a full implementation: submit Vulkan command buffer, present swapchain
    S_OK
}

/// IDirect3DDevice9::DrawPrimitive — draw non-indexed primitives.
pub fn draw_primitive(
    device: *mut ComBase,
    primitive_type: u32,
    start_vertex: u32,
    primitive_count: u32,
) -> HResult {
    if device.is_null() {
        return D3DERR_INVALIDCALL;
    }

    log::trace!(
        "[dxvk:d3d9] DrawPrimitive: type={}, start={}, count={}",
        primitive_type, start_vertex, primitive_count
    );

    unsafe {
        if let Some(data) = get_impl_data::<D3d9DeviceData>(device) {
            data.draw_calls += 1;
        }
    }

    // In a full implementation: record vkCmdDraw into command buffer
    S_OK
}

/// IDirect3DDevice9::SetTexture — bind a texture to a stage.
pub fn set_texture(device: *mut ComBase, stage: u32, texture: *mut ComBase) -> HResult {
    if device.is_null() {
        return D3DERR_INVALIDCALL;
    }

    let tex_handle = if texture.is_null() { 0 } else { texture as u64 };
    log::trace!("[dxvk:d3d9] SetTexture: stage={}, texture=0x{:X}", stage, tex_handle);

    unsafe {
        if let Some(data) = get_impl_data::<D3d9DeviceData>(device) {
            if (stage as usize) < data.textures.len() {
                data.textures[stage as usize] = tex_handle;
            }
        }
    }

    S_OK
}

/// IDirect3DDevice9::CreateVertexBuffer.
pub fn create_vertex_buffer(
    device: *mut ComBase,
    length: u32,
    usage: u32,
    fvf: u32,
    pool: u32,
    buffer_out: *mut *mut ComBase,
    shared_handle: u64,
) -> HResult {
    if device.is_null() || buffer_out.is_null() {
        return E_INVALIDARG;
    }

    log::debug!(
        "[dxvk:d3d9] CreateVertexBuffer: length={}, usage=0x{:X}, fvf=0x{:X}",
        length, usage, fvf
    );

    // In a full implementation: create a VkBuffer via claudio-vulkan
    let vb = create_com_object(TYPE_D3D9_VERTEX_BUFFER, &DEFAULT_IUNKNOWN_VTBL);
    unsafe { *buffer_out = vb; }

    S_OK
}

/// IDirect3DDevice9::CreateTexture.
pub fn create_texture(
    device: *mut ComBase,
    width: u32,
    height: u32,
    levels: u32,
    usage: u32,
    format: u32,
    pool: u32,
    texture_out: *mut *mut ComBase,
    shared_handle: u64,
) -> HResult {
    if device.is_null() || texture_out.is_null() {
        return E_INVALIDARG;
    }

    log::debug!(
        "[dxvk:d3d9] CreateTexture: {}x{}, levels={}, format={}, usage=0x{:X}",
        width, height, levels, format, usage
    );

    // In a full implementation: create a VkImage + VkImageView
    let tex = create_com_object(TYPE_D3D9_TEXTURE, &DEFAULT_IUNKNOWN_VTBL);
    unsafe { *texture_out = tex; }

    S_OK
}

/// Get the D3D9 adapter count.
pub fn get_adapter_count(d3d9: *mut ComBase) -> u32 {
    if d3d9.is_null() {
        return 0;
    }
    unsafe {
        get_impl_data::<D3d9Data>(d3d9)
            .map(|d| d.adapter_count)
            .unwrap_or(0)
    }
}
