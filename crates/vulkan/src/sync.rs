//! Synchronization primitives — vkCreateFence, vkCreateSemaphore,
//! vkWaitForFences, vkResetFences.
//!
//! These map to GPU fence/semaphore registers. In ClaudioOS's cooperative
//! model, fences check a memory-mapped value written by the GPU on completion.

use alloc::vec::Vec;
use spin::Mutex;
use log::{debug};

use crate::{VkResult, VkHandle, VkStructureType, alloc_handle};

/// Fence creation flags
pub const VK_FENCE_CREATE_SIGNALED_BIT: u32 = 0x01;

/// Fence creation info
#[derive(Debug, Clone)]
pub struct VkFenceCreateInfo {
    pub s_type: VkStructureType,
    pub flags: u32,
}

/// Semaphore creation info
#[derive(Debug, Clone)]
pub struct VkSemaphoreCreateInfo {
    pub s_type: VkStructureType,
    pub flags: u32,
}

/// A Vulkan fence — CPU-GPU synchronization
pub struct VkFence {
    pub handle: VkHandle,
    pub signaled: bool,
    /// GPU memory address where the fence value is written
    pub gpu_fence_addr: u64,
}

/// A Vulkan semaphore — GPU-GPU synchronization (between queue submissions)
pub struct VkSemaphore {
    pub handle: VkHandle,
    pub signaled: bool,
    /// Timeline value (for timeline semaphores, Vulkan 1.2+)
    pub value: u64,
}

static FENCES: Mutex<Vec<VkFence>> = Mutex::new(Vec::new());
static SEMAPHORES: Mutex<Vec<VkSemaphore>> = Mutex::new(Vec::new());

/// GPU fence address bump allocator
static NEXT_FENCE_ADDR: Mutex<u64> = Mutex::new(0x2000_0000);

/// vkCreateFence
pub fn vk_create_fence(
    _device: VkHandle,
    create_info: &VkFenceCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();
    let signaled = create_info.flags & VK_FENCE_CREATE_SIGNALED_BIT != 0;

    let gpu_fence_addr = {
        let mut addr = NEXT_FENCE_ADDR.lock();
        let a = *addr;
        *addr += 64; // Each fence gets 64 bytes (cache line aligned)
        a
    };

    debug!("vulkan: vkCreateFence handle={:?} signaled={} addr={:#x}",
           handle, signaled, gpu_fence_addr);

    let fence = VkFence {
        handle,
        signaled,
        gpu_fence_addr,
    };

    FENCES.lock().push(fence);
    Ok(handle)
}

/// vkDestroyFence
pub fn vk_destroy_fence(_device: VkHandle, fence: VkHandle) -> VkResult {
    let mut fences = FENCES.lock();
    if let Some(pos) = fences.iter().position(|f| f.handle == fence) {
        fences.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// vkWaitForFences — block until fence(s) are signaled
///
/// In a real implementation, this spins on (or `hlt`-waits for an interrupt from)
/// the GPU's fence memory location. The GPU writes a completion value when
/// the associated work finishes.
pub fn vk_wait_for_fences(
    _device: VkHandle,
    fences_to_wait: &[VkHandle],
    wait_all: bool,
    timeout_ns: u64,
) -> VkResult {
    let fences = FENCES.lock();

    let signaled_count = fences_to_wait.iter()
        .filter(|&&h| fences.iter().any(|f| f.handle == h && f.signaled))
        .count();

    if wait_all {
        if signaled_count == fences_to_wait.len() {
            VkResult::Success
        } else {
            // TODO: actually poll GPU fence registers with timeout
            // For now, return timeout if not all signaled
            debug!(
                "vulkan: vkWaitForFences {}/{} signaled, timeout={}ns",
                signaled_count, fences_to_wait.len(), timeout_ns
            );
            VkResult::Timeout
        }
    } else {
        if signaled_count > 0 {
            VkResult::Success
        } else {
            VkResult::Timeout
        }
    }
}

/// vkResetFences — reset fences to unsignaled state
pub fn vk_reset_fences(_device: VkHandle, fences_to_reset: &[VkHandle]) -> VkResult {
    let mut fences = FENCES.lock();

    for &handle in fences_to_reset {
        if let Some(fence) = fences.iter_mut().find(|f| f.handle == handle) {
            fence.signaled = false;
        }
    }

    debug!("vulkan: vkResetFences {} fences", fences_to_reset.len());
    VkResult::Success
}

/// vkCreateSemaphore
pub fn vk_create_semaphore(
    _device: VkHandle,
    _create_info: &VkSemaphoreCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();

    debug!("vulkan: vkCreateSemaphore handle={:?}", handle);

    let semaphore = VkSemaphore {
        handle,
        signaled: false,
        value: 0,
    };

    SEMAPHORES.lock().push(semaphore);
    Ok(handle)
}

/// vkDestroySemaphore
pub fn vk_destroy_semaphore(_device: VkHandle, semaphore: VkHandle) -> VkResult {
    let mut semas = SEMAPHORES.lock();
    if let Some(pos) = semas.iter().position(|s| s.handle == semaphore) {
        semas.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// Internal: signal a fence (called by queue submit completion)
pub fn signal_fence(fence: VkHandle) {
    let mut fences = FENCES.lock();
    if let Some(f) = fences.iter_mut().find(|f| f.handle == fence) {
        f.signaled = true;
        debug!("vulkan: fence {:?} signaled", fence);
    }
}

/// Internal: signal a semaphore
pub fn signal_semaphore(semaphore: VkHandle) {
    let mut semas = SEMAPHORES.lock();
    if let Some(s) = semas.iter_mut().find(|s| s.handle == semaphore) {
        s.signaled = true;
        s.value += 1;
    }
}
