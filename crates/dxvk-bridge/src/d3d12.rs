//! Direct3D 12 to Vulkan translation.
//!
//! Implements ID3D12Device, ID3D12CommandQueue, and ID3D12GraphicsCommandList
//! COM interfaces that translate D3D12 API calls to Vulkan.
//!
//! D3D12 maps more naturally to Vulkan than D3D9/D3D11 since both are explicit,
//! low-level APIs with command lists, pipeline state objects, and root signatures.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::com::*;

// =============================================================================
// D3D12 GUIDs
// =============================================================================

/// IID_ID3D12Device: {189819F1-1DB6-4B57-BE54-1821339B85F7}
pub const IID_ID3D12DEVICE: Guid = Guid {
    data1: 0x189819F1,
    data2: 0x1DB6,
    data3: 0x4B57,
    data4: [0xBE, 0x54, 0x18, 0x21, 0x33, 0x9B, 0x85, 0xF7],
};

/// IID_ID3D12CommandQueue: {0EC870A6-5D7E-4C22-8CFC-5BAAE07616ED}
pub const IID_ID3D12COMMANDQUEUE: Guid = Guid {
    data1: 0x0EC870A6,
    data2: 0x5D7E,
    data3: 0x4C22,
    data4: [0x8C, 0xFC, 0x5B, 0xAA, 0xE0, 0x76, 0x16, 0xED],
};

/// IID_ID3D12GraphicsCommandList: {5B160D0F-AC1B-4185-8BA8-B3AE42A5A455}
pub const IID_ID3D12GRAPHICSCOMMANDLIST: Guid = Guid {
    data1: 0x5B160D0F,
    data2: 0xAC1B,
    data3: 0x4185,
    data4: [0x8B, 0xA8, 0xB3, 0xAE, 0x42, 0xA5, 0xA4, 0x55],
};

// =============================================================================
// D3D12 types
// =============================================================================

/// D3D12 command list types.
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum D3d12CommandListType {
    Direct = 0,
    Bundle = 1,
    Compute = 2,
    Copy = 3,
}

/// D3D12_COMMAND_QUEUE_DESC.
#[repr(C)]
pub struct D3d12CommandQueueDesc {
    pub list_type: u32,
    pub priority: i32,
    pub flags: u32,
    pub node_mask: u32,
}

/// D3D12_GRAPHICS_PIPELINE_STATE_DESC (simplified).
#[repr(C)]
pub struct D3d12GraphicsPipelineStateDesc {
    pub root_signature: u64,
    pub vs_bytecode: u64,
    pub vs_length: u64,
    pub ps_bytecode: u64,
    pub ps_length: u64,
    pub blend_state: [u8; 64],
    pub sample_mask: u32,
    pub rasterizer_state: [u8; 40],
    pub depth_stencil_state: [u8; 28],
    pub input_layout_count: u32,
    pub input_layout_elements: u64,
    pub primitive_topology_type: u32,
    pub num_render_targets: u32,
    pub rtv_formats: [u32; 8],
    pub dsv_format: u32,
    pub sample_desc_count: u32,
    pub sample_desc_quality: u32,
}

/// D3D12_ROOT_SIGNATURE_DESC (simplified).
#[repr(C)]
pub struct D3d12RootSignatureDesc {
    pub num_parameters: u32,
    pub parameters: u64,
    pub num_static_samplers: u32,
    pub static_samplers: u64,
    pub flags: u32,
}

/// D3D12 primitive topology types.
#[repr(u32)]
pub enum D3d12PrimitiveTopologyType {
    Undefined = 0,
    Point = 1,
    Line = 2,
    Triangle = 3,
    Patch = 4,
}

// =============================================================================
// ID3D12Device implementation
// =============================================================================

/// Internal state for ID3D12Device.
struct D3d12DeviceData {
    node_count: u32,
    /// Created command queues.
    command_queue_count: u32,
    /// Created pipeline state objects.
    pso_count: u32,
    /// Created root signatures.
    root_sig_count: u32,
    /// Created command allocators.
    cmd_allocator_count: u32,
}

unsafe extern "system" fn d3d12_device_qi(
    this: *mut ComBase,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if ppv.is_null() { return E_POINTER; }
    *ppv = core::ptr::null_mut();
    if riid.is_null() { return E_INVALIDARG; }

    let iid = unsafe { &*riid };
    if *iid == IID_IUNKNOWN || *iid == IID_ID3D12DEVICE {
        *ppv = this as *mut core::ffi::c_void;
        d3d12_device_add_ref(this);
        return S_OK;
    }
    E_NOINTERFACE
}

unsafe extern "system" fn d3d12_device_add_ref(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    (*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn d3d12_device_release(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    let prev = (*this).ref_count.fetch_sub(1, Ordering::Relaxed);
    if prev == 1 {
        if !(*this).impl_data.is_null() {
            drop(Box::from_raw((*this).impl_data as *mut D3d12DeviceData));
        }
        drop(Box::from_raw(this));
        0
    } else {
        prev - 1
    }
}

static D3D12_DEVICE_VTBL: IUnknownVtbl = IUnknownVtbl {
    query_interface: d3d12_device_qi,
    add_ref: d3d12_device_add_ref,
    release: d3d12_device_release,
};

// =============================================================================
// ID3D12CommandQueue implementation
// =============================================================================

struct D3d12CommandQueueData {
    device: *mut ComBase,
    list_type: u32,
    /// Submitted command list count.
    executed_lists: u64,
}

unsafe extern "system" fn d3d12_queue_qi(
    this: *mut ComBase,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if ppv.is_null() { return E_POINTER; }
    *ppv = core::ptr::null_mut();
    if riid.is_null() { return E_INVALIDARG; }

    let iid = unsafe { &*riid };
    if *iid == IID_IUNKNOWN || *iid == IID_ID3D12COMMANDQUEUE {
        *ppv = this as *mut core::ffi::c_void;
        d3d12_queue_add_ref(this);
        return S_OK;
    }
    E_NOINTERFACE
}

unsafe extern "system" fn d3d12_queue_add_ref(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    (*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn d3d12_queue_release(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    let prev = (*this).ref_count.fetch_sub(1, Ordering::Relaxed);
    if prev == 1 {
        if !(*this).impl_data.is_null() {
            drop(Box::from_raw((*this).impl_data as *mut D3d12CommandQueueData));
        }
        drop(Box::from_raw(this));
        0
    } else {
        prev - 1
    }
}

static D3D12_QUEUE_VTBL: IUnknownVtbl = IUnknownVtbl {
    query_interface: d3d12_queue_qi,
    add_ref: d3d12_queue_add_ref,
    release: d3d12_queue_release,
};

// =============================================================================
// ID3D12GraphicsCommandList implementation
// =============================================================================

struct D3d12CommandListData {
    device: *mut ComBase,
    list_type: u32,
    /// Whether recording is in progress.
    recording: bool,
    /// Current pipeline state object.
    pipeline_state: u64,
    /// Current root signature.
    root_signature: u64,
    /// Draw call counter.
    draw_calls: u64,
    /// Dispatch counter.
    dispatches: u64,
}

unsafe extern "system" fn d3d12_cmdlist_qi(
    this: *mut ComBase,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if ppv.is_null() { return E_POINTER; }
    *ppv = core::ptr::null_mut();
    if riid.is_null() { return E_INVALIDARG; }

    let iid = unsafe { &*riid };
    if *iid == IID_IUNKNOWN || *iid == IID_ID3D12GRAPHICSCOMMANDLIST {
        *ppv = this as *mut core::ffi::c_void;
        d3d12_cmdlist_add_ref(this);
        return S_OK;
    }
    E_NOINTERFACE
}

unsafe extern "system" fn d3d12_cmdlist_add_ref(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    (*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn d3d12_cmdlist_release(this: *mut ComBase) -> u32 {
    if this.is_null() { return 0; }
    let prev = (*this).ref_count.fetch_sub(1, Ordering::Relaxed);
    if prev == 1 {
        if !(*this).impl_data.is_null() {
            drop(Box::from_raw((*this).impl_data as *mut D3d12CommandListData));
        }
        drop(Box::from_raw(this));
        0
    } else {
        prev - 1
    }
}

static D3D12_CMDLIST_VTBL: IUnknownVtbl = IUnknownVtbl {
    query_interface: d3d12_cmdlist_qi,
    add_ref: d3d12_cmdlist_add_ref,
    release: d3d12_cmdlist_release,
};

// =============================================================================
// D3D12 API entry points
// =============================================================================

/// D3D12CreateDevice — create a D3D12 device.
pub fn d3d12_create_device(
    adapter: *mut ComBase,
    minimum_feature_level: u32,
    riid: *const Guid,
    device_out: *mut *mut core::ffi::c_void,
) -> HResult {
    log::info!(
        "[dxvk:d3d12] D3D12CreateDevice: feature_level=0x{:X}",
        minimum_feature_level
    );

    if device_out.is_null() {
        return E_INVALIDARG;
    }

    let data = D3d12DeviceData {
        node_count: 1,
        command_queue_count: 0,
        pso_count: 0,
        root_sig_count: 0,
        cmd_allocator_count: 0,
    };

    let device = create_com_object_with_data(TYPE_D3D12_DEVICE, &D3D12_DEVICE_VTBL, data);
    unsafe { *device_out = device as *mut core::ffi::c_void; }

    S_OK
}

/// ID3D12Device::CreateCommandQueue.
pub fn create_command_queue(
    device: *mut ComBase,
    desc: *const D3d12CommandQueueDesc,
    riid: *const Guid,
    queue_out: *mut *mut core::ffi::c_void,
) -> HResult {
    if device.is_null() || desc.is_null() || queue_out.is_null() {
        return E_INVALIDARG;
    }

    let list_type = unsafe { (*desc).list_type };
    log::debug!("[dxvk:d3d12] CreateCommandQueue: type={}", list_type);

    let data = D3d12CommandQueueData {
        device,
        list_type,
        executed_lists: 0,
    };

    let queue = create_com_object_with_data(TYPE_D3D12_COMMAND_QUEUE, &D3D12_QUEUE_VTBL, data);
    unsafe { *queue_out = queue as *mut core::ffi::c_void; }

    unsafe {
        if let Some(dev) = get_impl_data::<D3d12DeviceData>(device) {
            dev.command_queue_count += 1;
        }
    }

    S_OK
}

/// ID3D12Device::CreateGraphicsPipelineState.
pub fn create_graphics_pipeline_state(
    device: *mut ComBase,
    desc: *const D3d12GraphicsPipelineStateDesc,
    riid: *const Guid,
    pso_out: *mut *mut core::ffi::c_void,
) -> HResult {
    if device.is_null() || desc.is_null() || pso_out.is_null() {
        return E_INVALIDARG;
    }

    log::debug!("[dxvk:d3d12] CreateGraphicsPipelineState");

    // In a full implementation: compile DXBC/DXIL shaders to SPIR-V,
    // create VkPipeline with matching state

    let pso = create_com_object(0, &DEFAULT_IUNKNOWN_VTBL);
    unsafe { *pso_out = pso as *mut core::ffi::c_void; }

    unsafe {
        if let Some(dev) = get_impl_data::<D3d12DeviceData>(device) {
            dev.pso_count += 1;
        }
    }

    S_OK
}

/// ID3D12Device::CreateRootSignature.
pub fn create_root_signature(
    device: *mut ComBase,
    node_mask: u32,
    blob: *const u8,
    blob_length: u64,
    riid: *const Guid,
    root_sig_out: *mut *mut core::ffi::c_void,
) -> HResult {
    if device.is_null() || root_sig_out.is_null() {
        return E_INVALIDARG;
    }

    log::debug!("[dxvk:d3d12] CreateRootSignature: blob_len={}", blob_length);

    // In a full implementation: parse root signature blob, create VkPipelineLayout
    let rs = create_com_object(0, &DEFAULT_IUNKNOWN_VTBL);
    unsafe { *root_sig_out = rs as *mut core::ffi::c_void; }

    unsafe {
        if let Some(dev) = get_impl_data::<D3d12DeviceData>(device) {
            dev.root_sig_count += 1;
        }
    }

    S_OK
}

/// ID3D12GraphicsCommandList::DrawInstanced.
pub fn draw_instanced(
    cmdlist: *mut ComBase,
    vertex_count_per_instance: u32,
    instance_count: u32,
    start_vertex_location: u32,
    start_instance_location: u32,
) {
    if cmdlist.is_null() { return; }

    log::trace!(
        "[dxvk:d3d12] DrawInstanced: verts={}, instances={}",
        vertex_count_per_instance, instance_count
    );

    unsafe {
        if let Some(data) = get_impl_data::<D3d12CommandListData>(cmdlist) {
            data.draw_calls += 1;
        }
    }
    // In a full implementation: record vkCmdDraw
}

/// ID3D12GraphicsCommandList::DrawIndexedInstanced.
pub fn draw_indexed_instanced(
    cmdlist: *mut ComBase,
    index_count_per_instance: u32,
    instance_count: u32,
    start_index_location: u32,
    base_vertex_location: i32,
    start_instance_location: u32,
) {
    if cmdlist.is_null() { return; }

    log::trace!(
        "[dxvk:d3d12] DrawIndexedInstanced: indices={}, instances={}",
        index_count_per_instance, instance_count
    );

    unsafe {
        if let Some(data) = get_impl_data::<D3d12CommandListData>(cmdlist) {
            data.draw_calls += 1;
        }
    }
    // In a full implementation: record vkCmdDrawIndexed
}

/// ID3D12CommandQueue::ExecuteCommandLists.
pub fn execute_command_lists(
    queue: *mut ComBase,
    num_command_lists: u32,
    command_lists: *const *mut ComBase,
) {
    if queue.is_null() { return; }

    log::trace!("[dxvk:d3d12] ExecuteCommandLists: count={}", num_command_lists);

    unsafe {
        if let Some(data) = get_impl_data::<D3d12CommandQueueData>(queue) {
            data.executed_lists += num_command_lists as u64;
        }
    }
    // In a full implementation: submit VkCommandBuffers to VkQueue
}
