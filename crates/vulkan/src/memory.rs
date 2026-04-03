//! GPU memory management — vkAllocateMemory, vkFreeMemory, vkMapMemory,
//! vkUnmapMemory. Wraps the GPU VRAM allocator from `claudio-gpu`.

use alloc::vec::Vec;
use core::ptr;
use spin::Mutex;
use log::{info, debug, warn};

use crate::{VkResult, VkHandle, VkStructureType, alloc_handle};

/// Memory allocation parameters
#[derive(Debug, Clone)]
pub struct VkMemoryAllocateInfo {
    pub s_type: VkStructureType,
    pub allocation_size: u64,
    pub memory_type_index: u32,
}

/// A device memory allocation
pub struct DeviceMemory {
    pub handle: VkHandle,
    /// Size in bytes
    pub size: u64,
    /// Memory type index (into VkPhysicalDeviceMemoryProperties)
    pub memory_type_index: u32,
    /// VRAM offset (from claudio-gpu allocator)
    pub vram_offset: u64,
    /// If mapped, the host-visible pointer
    pub mapped_ptr: Option<*mut u8>,
    /// Mapped offset within this allocation
    pub mapped_offset: u64,
    /// Mapped size
    pub mapped_size: u64,
}

// Safety: DeviceMemory mapped_ptr is only used from the single-address-space
// kernel context — no cross-process concerns in ClaudioOS.
unsafe impl Send for DeviceMemory {}
unsafe impl Sync for DeviceMemory {}

/// Global allocation registry
static ALLOCATIONS: Mutex<Vec<DeviceMemory>> = Mutex::new(Vec::new());

/// Simple bump allocator for VRAM offsets (placeholder for claudio-gpu integration)
static VRAM_BUMP: Mutex<u64> = Mutex::new(0x1000_0000); // Start at 256 MiB offset

/// vkAllocateMemory — allocate GPU device memory
pub fn vk_allocate_memory(
    _device: VkHandle,
    alloc_info: &VkMemoryAllocateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();

    // Align to 256 bytes (GPU alignment requirement)
    let aligned_size = (alloc_info.allocation_size + 255) & !255;

    let vram_offset = {
        let mut bump = VRAM_BUMP.lock();
        let offset = *bump;
        *bump += aligned_size;
        offset
    };

    info!(
        "vulkan: vkAllocateMemory {} bytes (aligned {}) type={} -> VRAM offset {:#x}",
        alloc_info.allocation_size, aligned_size,
        alloc_info.memory_type_index, vram_offset
    );

    let allocation = DeviceMemory {
        handle,
        size: alloc_info.allocation_size,
        memory_type_index: alloc_info.memory_type_index,
        vram_offset,
        mapped_ptr: None,
        mapped_offset: 0,
        mapped_size: 0,
    };

    ALLOCATIONS.lock().push(allocation);
    Ok(handle)
}

/// vkFreeMemory — release a device memory allocation
pub fn vk_free_memory(_device: VkHandle, memory: VkHandle) -> VkResult {
    debug!("vulkan: vkFreeMemory handle={:?}", memory);

    let mut allocs = ALLOCATIONS.lock();
    if let Some(pos) = allocs.iter().position(|a| a.handle == memory) {
        let alloc = &allocs[pos];
        if alloc.mapped_ptr.is_some() {
            warn!("vulkan: freeing memory that is still mapped!");
        }
        // TODO: return VRAM region to claudio-gpu allocator
        allocs.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfDeviceMemory
    }
}

/// vkMapMemory — map device memory into host-visible address space
///
/// For host-visible memory types (types 1 and 2), this returns a pointer
/// that the CPU can read/write. For ClaudioOS's single-address-space model,
/// the GPU BAR region is already mapped — we return the BAR base + offset.
pub fn vk_map_memory(
    _device: VkHandle,
    memory: VkHandle,
    offset: u64,
    size: u64,
) -> Result<*mut u8, VkResult> {
    let mut allocs = ALLOCATIONS.lock();
    let alloc = allocs.iter_mut()
        .find(|a| a.handle == memory)
        .ok_or(VkResult::ErrorMemoryMapFailed)?;

    if alloc.mapped_ptr.is_some() {
        return Err(VkResult::ErrorMemoryMapFailed); // Already mapped
    }

    // Only host-visible memory types can be mapped
    if alloc.memory_type_index == 0 {
        warn!("vulkan: cannot map device-local-only memory (type 0)");
        return Err(VkResult::ErrorMemoryMapFailed);
    }

    let map_size = if size == u64::MAX {
        alloc.size - offset
    } else {
        size
    };

    // In a real implementation, this would compute:
    //   BAR_base + alloc.vram_offset + offset
    // For now, return a placeholder pointer based on VRAM offset.
    // The actual memory-mapped I/O address comes from PCI BAR configuration
    // in claudio-gpu.
    let ptr = (alloc.vram_offset + offset) as *mut u8;

    alloc.mapped_ptr = Some(ptr);
    alloc.mapped_offset = offset;
    alloc.mapped_size = map_size;

    debug!(
        "vulkan: vkMapMemory handle={:?} offset={} size={} -> ptr={:?}",
        memory, offset, map_size, ptr
    );

    Ok(ptr)
}

/// vkUnmapMemory — unmap previously mapped device memory
pub fn vk_unmap_memory(_device: VkHandle, memory: VkHandle) -> VkResult {
    let mut allocs = ALLOCATIONS.lock();
    if let Some(alloc) = allocs.iter_mut().find(|a| a.handle == memory) {
        if alloc.mapped_ptr.is_none() {
            warn!("vulkan: unmapping memory that is not mapped");
        }
        alloc.mapped_ptr = None;
        alloc.mapped_offset = 0;
        alloc.mapped_size = 0;
        debug!("vulkan: vkUnmapMemory handle={:?}", memory);
        VkResult::Success
    } else {
        VkResult::ErrorMemoryMapFailed
    }
}

/// Helper: get VRAM offset for a memory allocation
pub fn get_memory_vram_offset(memory: VkHandle) -> Result<u64, VkResult> {
    let allocs = ALLOCATIONS.lock();
    let alloc = allocs.iter()
        .find(|a| a.handle == memory)
        .ok_or(VkResult::ErrorOutOfDeviceMemory)?;
    Ok(alloc.vram_offset)
}
