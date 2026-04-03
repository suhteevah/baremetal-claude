//! Direct3D 11 to Vulkan translation.
//!
//! Implements ID3D11Device and ID3D11DeviceContext COM interfaces that
//! translate D3D11 API calls into Vulkan commands.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::com::*;

// =============================================================================
// D3D11 GUIDs
// =============================================================================

/// IID_ID3D11Device: {DB6F6DDB-AC77-4E88-8253-819DF9BBF140}
pub const IID_ID3D11DEVICE: Guid = Guid {
    data1: 0xDB6F6DDB,
    data2: 0xAC77,
    data3: 0x4E88,
    data4: [0x82, 0x53, 0x81, 0x9D, 0xF9, 0xBB, 0xF1, 0x40],
};

/// IID_ID3D11DeviceContext: {C0BFA96C-E089-44FB-8EAF-26F8796190DA}
pub const IID_ID3D11DEVICECONTEXT: Guid = Guid {
    data1: 0xC0BFA96C,
    data2: 0xE089,
    data3: 0x44FB,
    data4: [0x8E, 0xAF, 0x26, 0xF8, 0x79, 0x61, 0x90, 0xDA],
};

// =============================================================================
// D3D11 types
// =============================================================================

/// D3D11_BUFFER_DESC.
#[repr(C)]
pub struct D3d11BufferDesc {
    pub byte_width: u32,
    pub usage: u32,         // D3D11_USAGE
    pub bind_flags: u32,    // D3D11_BIND_FLAG
    pub cpu_access_flags: u32,
    pub misc_flags: u32,
    pub structure_byte_stride: u32,
}

/// D3D11_TEXTURE2D_DESC.
#[repr(C)]
pub struct D3d11Texture2dDesc {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    pub array_size: u32,
    pub format: u32,        // DXGI_FORMAT
    pub sample_count: u32,
    pub sample_quality: u32,
    pub usage: u32,
    pub bind_flags: u32,
    pub cpu_access_flags: u32,
    pub misc_flags: u32,
}

/// D3D11_SUBRESOURCE_DATA.
#[repr(C)]
pub struct D3d11SubresourceData {
    pub sys_mem: *const core::ffi::c_void,
    pub sys_mem_pitch: u32,
    pub sys_mem_slice_pitch: u32,
}

/// D3D11 bind flags.
pub const D3D11_BIND_VERTEX_BUFFER: u32 = 0x1;
pub const D3D11_BIND_INDEX_BUFFER: u32 = 0x2;
pub const D3D11_BIND_CONSTANT_BUFFER: u32 = 0x4;
pub const D3D11_BIND_SHADER_RESOURCE: u32 = 0x8;
pub const D3D11_BIND_RENDER_TARGET: u32 = 0x20;
pub const D3D11_BIND_DEPTH_STENCIL: u32 = 0x40;
pub const D3D11_BIND_UNORDERED_ACCESS: u32 = 0x80;

/// D3D feature levels.
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum D3dFeatureLevel {
    Level9_1 = 0x9100,
    Level9_3 = 0x9300,
    Level10_0 = 0xa000,
    Level10_1 = 0xa100,
    Level11_0 = 0xb000,
    Level11_1 = 0xb100,
    Level12_0 = 0xc000,
    Level12_1 = 0xc100,
}

// =============================================================================
// ID3D11Device implementation
// =============================================================================

/// Internal state for ID3D11Device.
struct D3d11DeviceData {
    feature_level: u32,
    /// Immediate context associated with this device.
    immediate_context: *mut ComBase,
    /// Resource counter for debugging.
    buffer_count: u32,
    texture_count: u32,
}

unsafe extern "system" fn d3d11_device_qi(
    this: *mut ComBase,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if ppv.is_null() { return E_POINTER; }
    *ppv = core::ptr::null_mut();
    if riid.is_null() { return E_INVALIDARG; }

    let iid = unsafe { &*riid };
    if *iid == IID_IUNKNOWN || *iid == IID_ID3D11DEVICE {
        *ppv = this as *mut core::ffi::c_void;
        d3d11_device_add_ref(this);
        return S_OK;
    }
    E_NOINTERFACE
}

unsafe extern "system" fn d3d11_device_add_ref(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    (*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn d3d11_device_release(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    let prev = (*this).ref_count.fetch_sub(1, Ordering::Relaxed);
    if prev == 1 {
        if !(*this).impl_data.is_null() {
            drop(Box::from_raw((*this).impl_data as *mut D3d11DeviceData));
        }
        drop(Box::from_raw(this));
        0
    } else {
        prev - 1
    }
}

static D3D11_DEVICE_VTBL: IUnknownVtbl = IUnknownVtbl {
    query_interface: d3d11_device_qi,
    add_ref: d3d11_device_add_ref,
    release: d3d11_device_release,
};

// =============================================================================
// ID3D11DeviceContext implementation
// =============================================================================

/// Internal state for ID3D11DeviceContext.
struct D3d11ContextData {
    /// Parent device.
    device: *mut ComBase,
    /// Draw call counter.
    draw_calls: u64,
    /// Bound vertex shader.
    vs_shader: u64,
    /// Bound pixel shader.
    ps_shader: u64,
    /// Bound vertex buffers (up to 16 slots).
    vertex_buffers: [u64; 16],
    /// Bound index buffer.
    index_buffer: u64,
    /// Bound render targets (up to 8).
    render_targets: [u64; 8],
    /// Bound depth-stencil view.
    depth_stencil: u64,
}

unsafe extern "system" fn d3d11_ctx_qi(
    this: *mut ComBase,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if ppv.is_null() { return E_POINTER; }
    *ppv = core::ptr::null_mut();
    if riid.is_null() { return E_INVALIDARG; }

    let iid = unsafe { &*riid };
    if *iid == IID_IUNKNOWN || *iid == IID_ID3D11DEVICECONTEXT {
        *ppv = this as *mut core::ffi::c_void;
        d3d11_ctx_add_ref(this);
        return S_OK;
    }
    E_NOINTERFACE
}

unsafe extern "system" fn d3d11_ctx_add_ref(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    (*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn d3d11_ctx_release(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    let prev = (*this).ref_count.fetch_sub(1, Ordering::Relaxed);
    if prev == 1 {
        if !(*this).impl_data.is_null() {
            drop(Box::from_raw((*this).impl_data as *mut D3d11ContextData));
        }
        drop(Box::from_raw(this));
        0
    } else {
        prev - 1
    }
}

static D3D11_CONTEXT_VTBL: IUnknownVtbl = IUnknownVtbl {
    query_interface: d3d11_ctx_qi,
    add_ref: d3d11_ctx_add_ref,
    release: d3d11_ctx_release,
};

// =============================================================================
// D3D11 API entry points
// =============================================================================

/// D3D11CreateDevice — create a D3D11 device and immediate context.
pub fn d3d11_create_device(
    adapter: *mut ComBase,   // IDXGIAdapter
    driver_type: u32,
    software: u64,
    flags: u32,
    feature_levels: *const u32,
    feature_level_count: u32,
    sdk_version: u32,
    device_out: *mut *mut ComBase,
    feature_level_out: *mut u32,
    context_out: *mut *mut ComBase,
) -> HResult {
    log::info!(
        "[dxvk:d3d11] D3D11CreateDevice: driver_type={}, flags=0x{:X}, sdk={}",
        driver_type, flags, sdk_version
    );

    // Pick the highest supported feature level
    let chosen_level = if !feature_levels.is_null() && feature_level_count > 0 {
        unsafe { *feature_levels }
    } else {
        D3dFeatureLevel::Level11_1 as u32
    };

    if !feature_level_out.is_null() {
        unsafe { *feature_level_out = chosen_level; }
    }

    // Create the immediate context first
    let ctx_data = D3d11ContextData {
        device: core::ptr::null_mut(), // Will be patched below
        draw_calls: 0,
        vs_shader: 0,
        ps_shader: 0,
        vertex_buffers: [0; 16],
        index_buffer: 0,
        render_targets: [0; 8],
        depth_stencil: 0,
    };
    let ctx = create_com_object_with_data(TYPE_D3D11_CONTEXT, &D3D11_CONTEXT_VTBL, ctx_data);

    // Create the device
    let dev_data = D3d11DeviceData {
        feature_level: chosen_level,
        immediate_context: ctx,
        buffer_count: 0,
        texture_count: 0,
    };
    let device = create_com_object_with_data(TYPE_D3D11_DEVICE, &D3D11_DEVICE_VTBL, dev_data);

    // Patch the context's device pointer
    unsafe {
        if let Some(ctx_impl) = get_impl_data::<D3d11ContextData>(ctx) {
            ctx_impl.device = device;
        }
    }

    if !device_out.is_null() {
        unsafe { *device_out = device; }
    }
    if !context_out.is_null() {
        unsafe { *context_out = ctx; }
    }

    log::info!("[dxvk:d3d11] Device created with feature level 0x{:X}", chosen_level);
    S_OK
}

/// ID3D11Device::CreateBuffer.
pub fn create_buffer(
    device: *mut ComBase,
    desc: *const D3d11BufferDesc,
    initial_data: *const D3d11SubresourceData,
    buffer_out: *mut *mut ComBase,
) -> HResult {
    if device.is_null() || desc.is_null() || buffer_out.is_null() {
        return E_INVALIDARG;
    }

    let (size, bind_flags) = unsafe {
        ((*desc).byte_width, (*desc).bind_flags)
    };

    log::debug!(
        "[dxvk:d3d11] CreateBuffer: size={}, bind=0x{:X}",
        size, bind_flags
    );

    // In a full implementation: create VkBuffer with appropriate usage flags
    let buf = create_com_object(TYPE_D3D11_BUFFER, &DEFAULT_IUNKNOWN_VTBL);
    unsafe { *buffer_out = buf; }

    // Track count
    unsafe {
        if let Some(data) = get_impl_data::<D3d11DeviceData>(device) {
            data.buffer_count += 1;
        }
    }

    S_OK
}

/// ID3D11Device::CreateTexture2D.
pub fn create_texture2d(
    device: *mut ComBase,
    desc: *const D3d11Texture2dDesc,
    initial_data: *const D3d11SubresourceData,
    texture_out: *mut *mut ComBase,
) -> HResult {
    if device.is_null() || desc.is_null() || texture_out.is_null() {
        return E_INVALIDARG;
    }

    let (width, height, format) = unsafe {
        ((*desc).width, (*desc).height, (*desc).format)
    };

    log::debug!(
        "[dxvk:d3d11] CreateTexture2D: {}x{}, format={}, bind=0x{:X}",
        width, height, format, unsafe { (*desc).bind_flags }
    );

    // In a full implementation: create VkImage + VkImageView
    let tex = create_com_object(TYPE_D3D11_TEXTURE2D, &DEFAULT_IUNKNOWN_VTBL);
    unsafe { *texture_out = tex; }

    unsafe {
        if let Some(data) = get_impl_data::<D3d11DeviceData>(device) {
            data.texture_count += 1;
        }
    }

    S_OK
}

/// ID3D11DeviceContext::Draw — draw non-indexed primitives.
pub fn draw(ctx: *mut ComBase, vertex_count: u32, start_vertex_location: u32) {
    if ctx.is_null() { return; }

    log::trace!(
        "[dxvk:d3d11] Draw: verts={}, start={}",
        vertex_count, start_vertex_location
    );

    unsafe {
        if let Some(data) = get_impl_data::<D3d11ContextData>(ctx) {
            data.draw_calls += 1;
        }
    }

    // In a full implementation: record vkCmdDraw
}

/// ID3D11DeviceContext::DrawIndexed — draw indexed primitives.
pub fn draw_indexed(
    ctx: *mut ComBase,
    index_count: u32,
    start_index_location: u32,
    base_vertex_location: i32,
) {
    if ctx.is_null() { return; }

    log::trace!(
        "[dxvk:d3d11] DrawIndexed: indices={}, start={}, base={}",
        index_count, start_index_location, base_vertex_location
    );

    unsafe {
        if let Some(data) = get_impl_data::<D3d11ContextData>(ctx) {
            data.draw_calls += 1;
        }
    }

    // In a full implementation: record vkCmdDrawIndexed
}

/// ID3D11DeviceContext::VSSetShader — bind a vertex shader.
pub fn vs_set_shader(ctx: *mut ComBase, shader: u64, class_instances: u64, num_instances: u32) {
    if ctx.is_null() { return; }

    log::trace!("[dxvk:d3d11] VSSetShader: shader=0x{:X}", shader);

    unsafe {
        if let Some(data) = get_impl_data::<D3d11ContextData>(ctx) {
            data.vs_shader = shader;
        }
    }
}

/// ID3D11DeviceContext::PSSetShader — bind a pixel shader.
pub fn ps_set_shader(ctx: *mut ComBase, shader: u64, class_instances: u64, num_instances: u32) {
    if ctx.is_null() { return; }

    log::trace!("[dxvk:d3d11] PSSetShader: shader=0x{:X}", shader);

    unsafe {
        if let Some(data) = get_impl_data::<D3d11ContextData>(ctx) {
            data.ps_shader = shader;
        }
    }
}
