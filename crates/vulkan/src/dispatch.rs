//! ICD dispatch table and function pointer resolution — vkGetInstanceProcAddr,
//! vkGetDeviceProcAddr.
//!
//! This implements the Vulkan ICD (Installable Client Driver) interface. Since
//! ClaudioOS has no Vulkan loader, this dispatch table IS the full Vulkan
//! implementation. DXVK and other consumers call directly through these
//! function pointers.

use alloc::collections::BTreeMap;
use alloc::string::String;
use log::debug;

/// Function pointer type — all Vulkan functions are void-returning
/// function pointers at the dispatch level. The caller casts to the
/// correct signature.
pub type PfnVkVoidFunction = usize;

/// The ICD dispatch table — maps function names to implementation addresses
pub struct DispatchTable {
    /// Instance-level function pointers
    pub instance_procs: BTreeMap<String, PfnVkVoidFunction>,
    /// Device-level function pointers
    pub device_procs: BTreeMap<String, PfnVkVoidFunction>,
}

impl DispatchTable {
    /// Build the complete dispatch table with all supported entry points
    pub fn new() -> Self {
        let mut instance_procs = BTreeMap::new();
        let mut device_procs = BTreeMap::new();

        // ================================================================
        // Instance-level functions
        // ================================================================

        // Instance lifecycle
        register_instance(&mut instance_procs, "vkCreateInstance",
            crate::instance::vk_create_instance as usize);
        register_instance(&mut instance_procs, "vkDestroyInstance",
            crate::instance::vk_destroy_instance as usize);
        register_instance(&mut instance_procs, "vkEnumeratePhysicalDevices",
            crate::instance::vk_enumerate_physical_devices as usize);

        // Physical device queries
        register_instance(&mut instance_procs, "vkGetPhysicalDeviceProperties",
            crate::instance::vk_get_physical_device_properties as usize);
        register_instance(&mut instance_procs, "vkGetPhysicalDeviceFeatures",
            crate::instance::vk_get_physical_device_features as usize);
        register_instance(&mut instance_procs, "vkGetPhysicalDeviceMemoryProperties",
            crate::instance::vk_get_physical_device_memory_properties as usize);
        register_instance(&mut instance_procs, "vkGetPhysicalDeviceQueueFamilyProperties",
            crate::instance::vk_get_physical_device_queue_family_properties as usize);

        // Surface (VK_KHR_surface / VK_CLAUDIO_surface)
        register_instance(&mut instance_procs, "vkCreateClaudioSurfaceKHR",
            crate::surface::vk_create_claudio_surface as usize);
        register_instance(&mut instance_procs, "vkDestroySurfaceKHR",
            crate::surface::vk_destroy_surface as usize);
        register_instance(&mut instance_procs, "vkGetPhysicalDeviceSurfaceCapabilitiesKHR",
            crate::surface::vk_get_physical_device_surface_capabilities as usize);
        register_instance(&mut instance_procs, "vkGetPhysicalDeviceSurfaceFormatsKHR",
            crate::surface::vk_get_physical_device_surface_formats as usize);
        register_instance(&mut instance_procs, "vkGetPhysicalDeviceSurfacePresentModesKHR",
            crate::surface::vk_get_physical_device_surface_present_modes as usize);
        register_instance(&mut instance_procs, "vkGetPhysicalDeviceSurfaceSupportKHR",
            crate::surface::vk_get_physical_device_surface_support as usize);

        // Device creation
        register_instance(&mut instance_procs, "vkCreateDevice",
            crate::device::vk_create_device as usize);

        // ================================================================
        // Device-level functions
        // ================================================================

        // Device lifecycle
        register_device(&mut device_procs, "vkDestroyDevice",
            crate::device::vk_destroy_device as usize);
        register_device(&mut device_procs, "vkGetDeviceQueue",
            crate::device::vk_get_device_queue as usize);
        register_device(&mut device_procs, "vkDeviceWaitIdle",
            crate::device::vk_device_wait_idle as usize);

        // Memory
        register_device(&mut device_procs, "vkAllocateMemory",
            crate::memory::vk_allocate_memory as usize);
        register_device(&mut device_procs, "vkFreeMemory",
            crate::memory::vk_free_memory as usize);
        register_device(&mut device_procs, "vkMapMemory",
            crate::memory::vk_map_memory as usize);
        register_device(&mut device_procs, "vkUnmapMemory",
            crate::memory::vk_unmap_memory as usize);

        // Buffers
        register_device(&mut device_procs, "vkCreateBuffer",
            crate::buffer::vk_create_buffer as usize);
        register_device(&mut device_procs, "vkDestroyBuffer",
            crate::buffer::vk_destroy_buffer as usize);
        register_device(&mut device_procs, "vkBindBufferMemory",
            crate::buffer::vk_bind_buffer_memory as usize);
        register_device(&mut device_procs, "vkGetBufferMemoryRequirements",
            crate::buffer::vk_get_buffer_memory_requirements as usize);

        // Images
        register_device(&mut device_procs, "vkCreateImage",
            crate::image::vk_create_image as usize);
        register_device(&mut device_procs, "vkDestroyImage",
            crate::image::vk_destroy_image as usize);
        register_device(&mut device_procs, "vkBindImageMemory",
            crate::image::vk_bind_image_memory as usize);
        register_device(&mut device_procs, "vkCreateImageView",
            crate::image::vk_create_image_view as usize);
        register_device(&mut device_procs, "vkDestroyImageView",
            crate::image::vk_destroy_image_view as usize);
        register_device(&mut device_procs, "vkGetImageMemoryRequirements",
            crate::image::vk_get_image_memory_requirements as usize);

        // Shaders
        register_device(&mut device_procs, "vkCreateShaderModule",
            crate::shader::vk_create_shader_module as usize);
        register_device(&mut device_procs, "vkDestroyShaderModule",
            crate::shader::vk_destroy_shader_module as usize);

        // Pipelines
        register_device(&mut device_procs, "vkCreatePipelineLayout",
            crate::pipeline::vk_create_pipeline_layout as usize);
        register_device(&mut device_procs, "vkDestroyPipelineLayout",
            crate::pipeline::vk_destroy_pipeline_layout as usize);
        register_device(&mut device_procs, "vkCreateGraphicsPipelines",
            crate::pipeline::vk_create_graphics_pipelines as usize);
        register_device(&mut device_procs, "vkCreateComputePipelines",
            crate::pipeline::vk_create_compute_pipelines as usize);
        register_device(&mut device_procs, "vkDestroyPipeline",
            crate::pipeline::vk_destroy_pipeline as usize);

        // Command buffers
        register_device(&mut device_procs, "vkCreateCommandPool",
            crate::commands::vk_create_command_pool as usize);
        register_device(&mut device_procs, "vkDestroyCommandPool",
            crate::commands::vk_destroy_command_pool as usize);
        register_device(&mut device_procs, "vkAllocateCommandBuffers",
            crate::commands::vk_allocate_command_buffers as usize);
        register_device(&mut device_procs, "vkBeginCommandBuffer",
            crate::commands::vk_begin_command_buffer as usize);
        register_device(&mut device_procs, "vkEndCommandBuffer",
            crate::commands::vk_end_command_buffer as usize);
        register_device(&mut device_procs, "vkCmdDraw",
            crate::commands::vk_cmd_draw as usize);
        register_device(&mut device_procs, "vkCmdDrawIndexed",
            crate::commands::vk_cmd_draw_indexed as usize);
        register_device(&mut device_procs, "vkCmdDispatch",
            crate::commands::vk_cmd_dispatch as usize);
        register_device(&mut device_procs, "vkCmdCopyBuffer",
            crate::commands::vk_cmd_copy_buffer as usize);
        register_device(&mut device_procs, "vkCmdPipelineBarrier",
            crate::commands::vk_cmd_pipeline_barrier as usize);
        register_device(&mut device_procs, "vkQueueSubmit",
            crate::commands::vk_queue_submit as usize);

        // Render passes
        register_device(&mut device_procs, "vkCreateRenderPass",
            crate::renderpass::vk_create_render_pass as usize);
        register_device(&mut device_procs, "vkDestroyRenderPass",
            crate::renderpass::vk_destroy_render_pass as usize);
        register_device(&mut device_procs, "vkCreateFramebuffer",
            crate::renderpass::vk_create_framebuffer as usize);
        register_device(&mut device_procs, "vkDestroyFramebuffer",
            crate::renderpass::vk_destroy_framebuffer as usize);

        // Descriptors
        register_device(&mut device_procs, "vkCreateDescriptorSetLayout",
            crate::descriptor::vk_create_descriptor_set_layout as usize);
        register_device(&mut device_procs, "vkDestroyDescriptorSetLayout",
            crate::descriptor::vk_destroy_descriptor_set_layout as usize);
        register_device(&mut device_procs, "vkCreateDescriptorPool",
            crate::descriptor::vk_create_descriptor_pool as usize);
        register_device(&mut device_procs, "vkDestroyDescriptorPool",
            crate::descriptor::vk_destroy_descriptor_pool as usize);
        register_device(&mut device_procs, "vkAllocateDescriptorSets",
            crate::descriptor::vk_allocate_descriptor_sets as usize);
        register_device(&mut device_procs, "vkUpdateDescriptorSets",
            crate::descriptor::vk_update_descriptor_sets as usize);

        // Synchronization
        register_device(&mut device_procs, "vkCreateFence",
            crate::sync::vk_create_fence as usize);
        register_device(&mut device_procs, "vkDestroyFence",
            crate::sync::vk_destroy_fence as usize);
        register_device(&mut device_procs, "vkWaitForFences",
            crate::sync::vk_wait_for_fences as usize);
        register_device(&mut device_procs, "vkResetFences",
            crate::sync::vk_reset_fences as usize);
        register_device(&mut device_procs, "vkCreateSemaphore",
            crate::sync::vk_create_semaphore as usize);
        register_device(&mut device_procs, "vkDestroySemaphore",
            crate::sync::vk_destroy_semaphore as usize);

        // Swapchain (VK_KHR_swapchain)
        register_device(&mut device_procs, "vkCreateSwapchainKHR",
            crate::swapchain::vk_create_swapchain_khr as usize);
        register_device(&mut device_procs, "vkDestroySwapchainKHR",
            crate::swapchain::vk_destroy_swapchain_khr as usize);
        register_device(&mut device_procs, "vkGetSwapchainImagesKHR",
            crate::swapchain::vk_get_swapchain_images_khr as usize);
        register_device(&mut device_procs, "vkAcquireNextImageKHR",
            crate::swapchain::vk_acquire_next_image_khr as usize);
        register_device(&mut device_procs, "vkQueuePresentKHR",
            crate::swapchain::vk_queue_present_khr as usize);

        debug!(
            "vulkan: dispatch table built: {} instance procs, {} device procs",
            instance_procs.len(), device_procs.len()
        );

        DispatchTable {
            instance_procs,
            device_procs,
        }
    }

    /// vkGetInstanceProcAddr — resolve an instance-level function by name
    pub fn get_instance_proc_addr(&self, name: &str) -> Option<PfnVkVoidFunction> {
        // Instance procs include device procs (Vulkan spec: instance can resolve device procs)
        self.instance_procs.get(name)
            .or_else(|| self.device_procs.get(name))
            .copied()
    }

    /// vkGetDeviceProcAddr — resolve a device-level function by name
    pub fn get_device_proc_addr(&self, name: &str) -> Option<PfnVkVoidFunction> {
        self.device_procs.get(name).copied()
    }
}

fn register_instance(map: &mut BTreeMap<String, PfnVkVoidFunction>, name: &str, addr: usize) {
    map.insert(String::from(name), addr);
}

fn register_device(map: &mut BTreeMap<String, PfnVkVoidFunction>, name: &str, addr: usize) {
    map.insert(String::from(name), addr);
}
