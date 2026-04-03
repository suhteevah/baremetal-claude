//! Logical device and queue management — vkCreateDevice, vkDestroyDevice,
//! vkGetDeviceQueue, vkDeviceWaitIdle.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::collections::BTreeMap;
use spin::Mutex;
use log::{info, debug};

use crate::{VkResult, VkHandle, VkStructureType, alloc_handle};
use crate::instance::VkPhysicalDeviceFeatures;

/// Queue priority and creation info
#[derive(Debug, Clone)]
pub struct VkDeviceQueueCreateInfo {
    pub s_type: VkStructureType,
    pub queue_family_index: u32,
    pub queue_priorities: Vec<f32>,
}

/// Device creation parameters
#[derive(Debug, Clone)]
pub struct VkDeviceCreateInfo {
    pub s_type: VkStructureType,
    pub queue_create_infos: Vec<VkDeviceQueueCreateInfo>,
    pub enabled_extension_names: Vec<String>,
    pub enabled_features: Option<VkPhysicalDeviceFeatures>,
}

/// A logical Vulkan device bound to a physical GPU
pub struct VkDevice {
    pub handle: VkHandle,
    pub physical_device: VkHandle,
    pub queues: BTreeMap<(u32, u32), VkQueue>,
    pub enabled_extensions: Vec<String>,
}

/// A Vulkan queue — represents a GPU command submission endpoint
pub struct VkQueue {
    pub handle: VkHandle,
    pub family_index: u32,
    pub queue_index: u32,
    pub priority: f32,
}

/// Global device registry
static DEVICES: Mutex<Vec<VkDevice>> = Mutex::new(Vec::new());

/// Supported device extensions
const SUPPORTED_DEVICE_EXTENSIONS: &[&str] = &[
    "VK_KHR_swapchain",
];

/// vkCreateDevice — create a logical device from a physical device
pub fn vk_create_device(
    physical_device: VkHandle,
    create_info: &VkDeviceCreateInfo,
) -> Result<VkHandle, VkResult> {
    info!("vulkan: vkCreateDevice for physical device {:?}", physical_device);

    // Validate extensions
    for ext in &create_info.enabled_extension_names {
        if !SUPPORTED_DEVICE_EXTENSIONS.contains(&ext.as_str()) {
            log::warn!("vulkan: unsupported device extension: {}", ext);
            return Err(VkResult::ErrorExtensionNotPresent);
        }
    }

    let device_handle = alloc_handle();
    let mut queues = BTreeMap::new();

    // Create requested queues
    for qci in &create_info.queue_create_infos {
        debug!(
            "vulkan: creating {} queues for family {}",
            qci.queue_priorities.len(),
            qci.queue_family_index
        );

        for (idx, &priority) in qci.queue_priorities.iter().enumerate() {
            let queue = VkQueue {
                handle: alloc_handle(),
                family_index: qci.queue_family_index,
                queue_index: idx as u32,
                priority,
            };
            queues.insert((qci.queue_family_index, idx as u32), queue);
        }
    }

    let device = VkDevice {
        handle: device_handle,
        physical_device,
        queues,
        enabled_extensions: create_info.enabled_extension_names.clone(),
    };

    DEVICES.lock().push(device);

    info!("vulkan: device created, handle={:?}", device_handle);
    Ok(device_handle)
}

/// vkDestroyDevice — destroy a logical device and all its queues
pub fn vk_destroy_device(device: VkHandle) -> VkResult {
    info!("vulkan: vkDestroyDevice handle={:?}", device);
    let mut devices = DEVICES.lock();
    if let Some(pos) = devices.iter().position(|d| d.handle == device) {
        devices.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorDeviceLost
    }
}

/// vkGetDeviceQueue — retrieve a queue handle by family and index
pub fn vk_get_device_queue(
    device: VkHandle,
    queue_family_index: u32,
    queue_index: u32,
) -> Result<VkHandle, VkResult> {
    let devices = DEVICES.lock();
    let dev = devices.iter()
        .find(|d| d.handle == device)
        .ok_or(VkResult::ErrorDeviceLost)?;

    let queue = dev.queues.get(&(queue_family_index, queue_index))
        .ok_or(VkResult::ErrorInitializationFailed)?;

    debug!(
        "vulkan: vkGetDeviceQueue family={} index={} -> {:?}",
        queue_family_index, queue_index, queue.handle
    );

    Ok(queue.handle)
}

/// vkDeviceWaitIdle — block until all queues on the device are idle
///
/// In our implementation, this drains all pending GPU FIFO commands
/// by polling the FIFO fence registers until completion.
pub fn vk_device_wait_idle(device: VkHandle) -> VkResult {
    debug!("vulkan: vkDeviceWaitIdle handle={:?}", device);
    let devices = DEVICES.lock();
    if devices.iter().any(|d| d.handle == device) {
        // TODO: actually drain GPU FIFO via claudio-gpu
        // For now this is a no-op since we're building the framework
        VkResult::Success
    } else {
        VkResult::ErrorDeviceLost
    }
}

/// Helper: access a device by handle
pub fn with_device<F, R>(device: VkHandle, f: F) -> Result<R, VkResult>
where
    F: FnOnce(&VkDevice) -> R,
{
    let devices = DEVICES.lock();
    let dev = devices.iter()
        .find(|d| d.handle == device)
        .ok_or(VkResult::ErrorDeviceLost)?;
    Ok(f(dev))
}
