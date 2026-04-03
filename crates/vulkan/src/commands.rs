//! Command buffer recording and submission — vkCreateCommandPool,
//! vkAllocateCommandBuffers, command recording (draw, dispatch, copy, barriers),
//! vkQueueSubmit.

use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;
use log::{info, debug};

use crate::{VkResult, VkHandle, VkStructureType, alloc_handle};

/// Command pool creation info
#[derive(Debug, Clone)]
pub struct VkCommandPoolCreateInfo {
    pub s_type: VkStructureType,
    pub flags: u32,
    pub queue_family_index: u32,
}

/// Command pool flags
pub const VK_COMMAND_POOL_CREATE_TRANSIENT_BIT: u32 = 0x01;
pub const VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT: u32 = 0x02;

/// Command buffer level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkCommandBufferLevel {
    Primary = 0,
    Secondary = 1,
}

/// Command buffer allocation info
#[derive(Debug, Clone)]
pub struct VkCommandBufferAllocateInfo {
    pub s_type: VkStructureType,
    pub command_pool: VkHandle,
    pub level: VkCommandBufferLevel,
    pub command_buffer_count: u32,
}

/// Command buffer begin info
#[derive(Debug, Clone)]
pub struct VkCommandBufferBeginInfo {
    pub s_type: VkStructureType,
    pub flags: u32,
}

/// Command buffer usage flags
pub const VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT: u32 = 0x01;
pub const VK_COMMAND_BUFFER_USAGE_SIMULTANEOUS_USE_BIT: u32 = 0x04;

/// Recorded GPU command
#[derive(Debug, Clone)]
pub enum GpuCommand {
    /// Begin a render pass
    BeginRenderPass {
        render_pass: VkHandle,
        framebuffer: VkHandle,
        clear_values: Vec<ClearValue>,
    },
    /// End the current render pass
    EndRenderPass,
    /// Bind a pipeline
    BindPipeline {
        pipeline: VkHandle,
        is_compute: bool,
    },
    /// Bind vertex buffers
    BindVertexBuffers {
        first_binding: u32,
        buffers: Vec<VkHandle>,
        offsets: Vec<u64>,
    },
    /// Bind an index buffer
    BindIndexBuffer {
        buffer: VkHandle,
        offset: u64,
        index_type: VkIndexType,
    },
    /// Bind descriptor sets
    BindDescriptorSets {
        is_compute: bool,
        layout: VkHandle,
        first_set: u32,
        descriptor_sets: Vec<VkHandle>,
        dynamic_offsets: Vec<u32>,
    },
    /// Draw vertices
    Draw {
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    },
    /// Draw indexed vertices
    DrawIndexed {
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        vertex_offset: i32,
        first_instance: u32,
    },
    /// Dispatch compute work groups
    Dispatch {
        group_count_x: u32,
        group_count_y: u32,
        group_count_z: u32,
    },
    /// Copy between buffers
    CopyBuffer {
        src: VkHandle,
        dst: VkHandle,
        regions: Vec<BufferCopy>,
    },
    /// Pipeline barrier for synchronization
    PipelineBarrier {
        src_stage_mask: u32,
        dst_stage_mask: u32,
        memory_barriers: Vec<MemoryBarrier>,
        image_barriers: Vec<ImageMemoryBarrier>,
    },
    /// Push constants
    PushConstants {
        layout: VkHandle,
        stage_flags: u32,
        offset: u32,
        data: Vec<u8>,
    },
    /// Set viewport dynamically
    SetViewport {
        first_viewport: u32,
        viewports: Vec<crate::VkViewport>,
    },
    /// Set scissor dynamically
    SetScissor {
        first_scissor: u32,
        scissors: Vec<crate::VkRect2D>,
    },
}

/// Clear value for render pass begin
#[derive(Debug, Clone, Copy)]
pub enum ClearValue {
    Color([f32; 4]),
    DepthStencil(f32, u32),
}

/// Index type for indexed draws
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkIndexType {
    Uint16 = 0,
    Uint32 = 1,
}

/// Buffer copy region
#[derive(Debug, Clone, Copy)]
pub struct BufferCopy {
    pub src_offset: u64,
    pub dst_offset: u64,
    pub size: u64,
}

/// Memory barrier
#[derive(Debug, Clone)]
pub struct MemoryBarrier {
    pub src_access_mask: u32,
    pub dst_access_mask: u32,
}

/// Image memory barrier (includes layout transition)
#[derive(Debug, Clone)]
pub struct ImageMemoryBarrier {
    pub src_access_mask: u32,
    pub dst_access_mask: u32,
    pub old_layout: crate::image::VkImageLayout,
    pub new_layout: crate::image::VkImageLayout,
    pub image: VkHandle,
}

/// Submit info for vkQueueSubmit
#[derive(Debug)]
pub struct VkSubmitInfo {
    pub s_type: VkStructureType,
    pub wait_semaphores: Vec<VkHandle>,
    pub wait_dst_stage_mask: Vec<u32>,
    pub command_buffers: Vec<VkHandle>,
    pub signal_semaphores: Vec<VkHandle>,
}

/// A command pool — allocates command buffers for a specific queue family
pub struct VkCommandPool {
    pub handle: VkHandle,
    pub queue_family_index: u32,
    pub flags: u32,
}

/// A command buffer — records GPU commands for later submission
pub struct VkCommandBuffer {
    pub handle: VkHandle,
    pub pool: VkHandle,
    pub level: VkCommandBufferLevel,
    pub recording: bool,
    pub commands: Vec<GpuCommand>,
}

static COMMAND_POOLS: Mutex<Vec<VkCommandPool>> = Mutex::new(Vec::new());
static COMMAND_BUFFERS: Mutex<Vec<VkCommandBuffer>> = Mutex::new(Vec::new());

/// vkCreateCommandPool
pub fn vk_create_command_pool(
    _device: VkHandle,
    create_info: &VkCommandPoolCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();

    debug!(
        "vulkan: vkCreateCommandPool family={} flags={:#x}",
        create_info.queue_family_index, create_info.flags
    );

    let pool = VkCommandPool {
        handle,
        queue_family_index: create_info.queue_family_index,
        flags: create_info.flags,
    };

    COMMAND_POOLS.lock().push(pool);
    Ok(handle)
}

/// vkDestroyCommandPool
pub fn vk_destroy_command_pool(_device: VkHandle, pool: VkHandle) -> VkResult {
    // Remove all command buffers belonging to this pool
    COMMAND_BUFFERS.lock().retain(|cb| cb.pool != pool);

    let mut pools = COMMAND_POOLS.lock();
    if let Some(pos) = pools.iter().position(|p| p.handle == pool) {
        pools.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// vkAllocateCommandBuffers
pub fn vk_allocate_command_buffers(
    _device: VkHandle,
    alloc_info: &VkCommandBufferAllocateInfo,
) -> Result<Vec<VkHandle>, VkResult> {
    let count = alloc_info.command_buffer_count as usize;
    let mut handles = Vec::with_capacity(count);
    let mut cbs = COMMAND_BUFFERS.lock();

    for _ in 0..count {
        let handle = alloc_handle();
        let cb = VkCommandBuffer {
            handle,
            pool: alloc_info.command_pool,
            level: alloc_info.level,
            recording: false,
            commands: Vec::new(),
        };
        cbs.push(cb);
        handles.push(handle);
    }

    debug!(
        "vulkan: vkAllocateCommandBuffers {} buffers from pool {:?}",
        count, alloc_info.command_pool
    );

    Ok(handles)
}

/// vkBeginCommandBuffer — start recording commands
pub fn vk_begin_command_buffer(
    command_buffer: VkHandle,
    _begin_info: &VkCommandBufferBeginInfo,
) -> VkResult {
    let mut cbs = COMMAND_BUFFERS.lock();
    if let Some(cb) = cbs.iter_mut().find(|c| c.handle == command_buffer) {
        cb.recording = true;
        cb.commands.clear();
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// vkEndCommandBuffer — finish recording
pub fn vk_end_command_buffer(command_buffer: VkHandle) -> VkResult {
    let mut cbs = COMMAND_BUFFERS.lock();
    if let Some(cb) = cbs.iter_mut().find(|c| c.handle == command_buffer) {
        cb.recording = false;
        debug!(
            "vulkan: vkEndCommandBuffer {:?} with {} commands",
            command_buffer, cb.commands.len()
        );
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

// ============================================================================
// Command recording helpers
// ============================================================================

fn record_command(command_buffer: VkHandle, cmd: GpuCommand) -> VkResult {
    let mut cbs = COMMAND_BUFFERS.lock();
    if let Some(cb) = cbs.iter_mut().find(|c| c.handle == command_buffer) {
        if !cb.recording {
            return VkResult::ErrorOutOfHostMemory;
        }
        cb.commands.push(cmd);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// vkCmdBeginRenderPass
pub fn vk_cmd_begin_render_pass(
    command_buffer: VkHandle,
    render_pass: VkHandle,
    framebuffer: VkHandle,
    clear_values: Vec<ClearValue>,
) -> VkResult {
    record_command(command_buffer, GpuCommand::BeginRenderPass {
        render_pass,
        framebuffer,
        clear_values,
    })
}

/// vkCmdEndRenderPass
pub fn vk_cmd_end_render_pass(command_buffer: VkHandle) -> VkResult {
    record_command(command_buffer, GpuCommand::EndRenderPass)
}

/// vkCmdBindPipeline
pub fn vk_cmd_bind_pipeline(
    command_buffer: VkHandle,
    pipeline: VkHandle,
    is_compute: bool,
) -> VkResult {
    record_command(command_buffer, GpuCommand::BindPipeline { pipeline, is_compute })
}

/// vkCmdBindVertexBuffers
pub fn vk_cmd_bind_vertex_buffers(
    command_buffer: VkHandle,
    first_binding: u32,
    buffers: Vec<VkHandle>,
    offsets: Vec<u64>,
) -> VkResult {
    record_command(command_buffer, GpuCommand::BindVertexBuffers {
        first_binding,
        buffers,
        offsets,
    })
}

/// vkCmdBindIndexBuffer
pub fn vk_cmd_bind_index_buffer(
    command_buffer: VkHandle,
    buffer: VkHandle,
    offset: u64,
    index_type: VkIndexType,
) -> VkResult {
    record_command(command_buffer, GpuCommand::BindIndexBuffer {
        buffer,
        offset,
        index_type,
    })
}

/// vkCmdDraw — non-indexed draw
pub fn vk_cmd_draw(
    command_buffer: VkHandle,
    vertex_count: u32,
    instance_count: u32,
    first_vertex: u32,
    first_instance: u32,
) -> VkResult {
    record_command(command_buffer, GpuCommand::Draw {
        vertex_count,
        instance_count,
        first_vertex,
        first_instance,
    })
}

/// vkCmdDrawIndexed — indexed draw
pub fn vk_cmd_draw_indexed(
    command_buffer: VkHandle,
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    vertex_offset: i32,
    first_instance: u32,
) -> VkResult {
    record_command(command_buffer, GpuCommand::DrawIndexed {
        index_count,
        instance_count,
        first_index,
        vertex_offset,
        first_instance,
    })
}

/// vkCmdDispatch — dispatch compute work groups
pub fn vk_cmd_dispatch(
    command_buffer: VkHandle,
    group_count_x: u32,
    group_count_y: u32,
    group_count_z: u32,
) -> VkResult {
    record_command(command_buffer, GpuCommand::Dispatch {
        group_count_x,
        group_count_y,
        group_count_z,
    })
}

/// vkCmdCopyBuffer — copy data between buffers
pub fn vk_cmd_copy_buffer(
    command_buffer: VkHandle,
    src: VkHandle,
    dst: VkHandle,
    regions: Vec<BufferCopy>,
) -> VkResult {
    record_command(command_buffer, GpuCommand::CopyBuffer { src, dst, regions })
}

/// vkCmdPipelineBarrier — insert a pipeline barrier for synchronization
pub fn vk_cmd_pipeline_barrier(
    command_buffer: VkHandle,
    src_stage_mask: u32,
    dst_stage_mask: u32,
    memory_barriers: Vec<MemoryBarrier>,
    image_barriers: Vec<ImageMemoryBarrier>,
) -> VkResult {
    record_command(command_buffer, GpuCommand::PipelineBarrier {
        src_stage_mask,
        dst_stage_mask,
        memory_barriers,
        image_barriers,
    })
}

/// vkQueueSubmit — submit command buffers to a queue for execution
///
/// This is where recorded commands get translated into GPU FIFO push buffer
/// entries and submitted via claudio-gpu's GPFIFO channel. Each GpuCommand
/// maps to a sequence of NV class methods pushed into the FIFO.
pub fn vk_queue_submit(
    queue: VkHandle,
    submits: &[VkSubmitInfo],
    fence: VkHandle,
) -> VkResult {
    let cbs = COMMAND_BUFFERS.lock();

    for (i, submit) in submits.iter().enumerate() {
        debug!(
            "vulkan: vkQueueSubmit[{}] to queue {:?}: {} cmd bufs, wait {} semas, signal {} semas",
            i, queue,
            submit.command_buffers.len(),
            submit.wait_semaphores.len(),
            submit.signal_semaphores.len()
        );

        let total_cmds: usize = submit.command_buffers.iter()
            .filter_map(|h| cbs.iter().find(|c| c.handle == *h))
            .map(|c| c.commands.len())
            .sum();

        debug!("vulkan: total GPU commands in submission: {}", total_cmds);

        // TODO: translate GpuCommand list to NVIDIA FIFO push buffer:
        //   1. Wait on semaphores (acquire fence values)
        //   2. For each command buffer, walk commands and emit NV methods:
        //      - Draw/DrawIndexed -> 3D class VERTEX_BEGIN_GL, VERTEX_BUFFER_FIRST, etc.
        //      - Dispatch -> compute class LAUNCH_DMA
        //      - CopyBuffer -> copy engine CLASS_DMA
        //      - PipelineBarrier -> memory barrier methods
        //   3. Signal semaphores
        //   4. If fence is valid, write fence value
    }

    // Signal the fence if provided
    if !fence.is_null() {
        crate::sync::signal_fence(fence);
    }

    VkResult::Success
}
