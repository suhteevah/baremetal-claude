//! Vulkan buffer management — vkCreateBuffer, vkDestroyBuffer,
//! vkBindBufferMemory. Supports vertex, index, uniform, and storage buffers.

use alloc::vec::Vec;
use spin::Mutex;
use log::{info, debug};

use crate::{
    VkResult, VkHandle, VkStructureType, alloc_handle,
    VK_BUFFER_USAGE_VERTEX_BUFFER_BIT, VK_BUFFER_USAGE_INDEX_BUFFER_BIT,
    VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT, VK_BUFFER_USAGE_STORAGE_BUFFER_BIT,
};

/// Buffer creation parameters
#[derive(Debug, Clone)]
pub struct VkBufferCreateInfo {
    pub s_type: VkStructureType,
    pub size: u64,
    pub usage: u32,
    pub sharing_mode: VkSharingMode,
}

/// Buffer sharing mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkSharingMode {
    Exclusive = 0,
    Concurrent = 1,
}

/// A Vulkan buffer object
pub struct VkBuffer {
    pub handle: VkHandle,
    pub size: u64,
    pub usage: u32,
    pub sharing_mode: VkSharingMode,
    /// Bound device memory (set by vkBindBufferMemory)
    pub bound_memory: Option<VkHandle>,
    /// Offset within the bound memory allocation
    pub memory_offset: u64,
}

/// Global buffer registry
static BUFFERS: Mutex<Vec<VkBuffer>> = Mutex::new(Vec::new());

/// Format usage flags as human-readable string for logging
fn usage_str(usage: u32) -> &'static str {
    if usage & VK_BUFFER_USAGE_VERTEX_BUFFER_BIT != 0 {
        "vertex"
    } else if usage & VK_BUFFER_USAGE_INDEX_BUFFER_BIT != 0 {
        "index"
    } else if usage & VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT != 0 {
        "uniform"
    } else if usage & VK_BUFFER_USAGE_STORAGE_BUFFER_BIT != 0 {
        "storage"
    } else {
        "generic"
    }
}

/// vkCreateBuffer — create a buffer object (does not allocate memory)
pub fn vk_create_buffer(
    _device: VkHandle,
    create_info: &VkBufferCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();

    info!(
        "vulkan: vkCreateBuffer {} bytes, usage={} ({}), sharing={:?}",
        create_info.size, create_info.usage,
        usage_str(create_info.usage), create_info.sharing_mode
    );

    let buffer = VkBuffer {
        handle,
        size: create_info.size,
        usage: create_info.usage,
        sharing_mode: create_info.sharing_mode,
        bound_memory: None,
        memory_offset: 0,
    };

    BUFFERS.lock().push(buffer);
    Ok(handle)
}

/// vkDestroyBuffer — destroy a buffer object
pub fn vk_destroy_buffer(_device: VkHandle, buffer: VkHandle) -> VkResult {
    debug!("vulkan: vkDestroyBuffer handle={:?}", buffer);
    let mut buffers = BUFFERS.lock();
    if let Some(pos) = buffers.iter().position(|b| b.handle == buffer) {
        buffers.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// vkBindBufferMemory — bind device memory to a buffer
///
/// After this call, the buffer's GPU address is:
///   memory.vram_offset + memory_offset
pub fn vk_bind_buffer_memory(
    _device: VkHandle,
    buffer: VkHandle,
    memory: VkHandle,
    memory_offset: u64,
) -> VkResult {
    let mut buffers = BUFFERS.lock();
    if let Some(buf) = buffers.iter_mut().find(|b| b.handle == buffer) {
        if buf.bound_memory.is_some() {
            log::warn!("vulkan: buffer {:?} already has bound memory", buffer);
            return VkResult::ErrorOutOfDeviceMemory;
        }

        buf.bound_memory = Some(memory);
        buf.memory_offset = memory_offset;

        debug!(
            "vulkan: vkBindBufferMemory buffer={:?} memory={:?} offset={}",
            buffer, memory, memory_offset
        );

        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// Get buffer memory requirements (alignment, size, compatible memory types)
pub fn vk_get_buffer_memory_requirements(
    _device: VkHandle,
    buffer: VkHandle,
) -> Result<MemoryRequirements, VkResult> {
    let buffers = BUFFERS.lock();
    let buf = buffers.iter()
        .find(|b| b.handle == buffer)
        .ok_or(VkResult::ErrorOutOfHostMemory)?;

    // Align to 256 bytes (NVIDIA minimum for UBOs)
    let alignment = if buf.usage & VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT != 0 {
        256
    } else {
        64
    };

    Ok(MemoryRequirements {
        size: (buf.size + alignment - 1) & !(alignment - 1),
        alignment,
        memory_type_bits: 0b111, // Compatible with all three memory types
    })
}

/// Memory requirements returned by vkGetBufferMemoryRequirements
#[derive(Debug, Clone)]
pub struct MemoryRequirements {
    pub size: u64,
    pub alignment: u64,
    pub memory_type_bits: u32,
}
