//! VkSwapchainKHR — present rendered frames to the ClaudioOS framebuffer.
//!
//! Unlike a traditional swapchain that talks to the display server, ours maps
//! directly to double-buffered framebuffer regions. Acquiring an image returns
//! the back buffer; presenting blits to the front buffer via the GPU's copy engine.

use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;
use log::{info, debug};

use crate::{
    VkResult, VkHandle, VkFormat, VkExtent2D, VkStructureType, alloc_handle,
};

/// Color space (we only support sRGB nonlinear)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkColorSpaceKHR {
    SrgbNonlinear = 0,
}

/// Present mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkPresentModeKHR {
    Immediate = 0,
    Mailbox = 1,
    Fifo = 2,
    FifoRelaxed = 3,
}

/// Swapchain creation parameters
#[derive(Debug, Clone)]
pub struct VkSwapchainCreateInfoKHR {
    pub s_type: VkStructureType,
    pub surface: VkHandle,
    pub min_image_count: u32,
    pub image_format: VkFormat,
    pub image_color_space: VkColorSpaceKHR,
    pub image_extent: VkExtent2D,
    pub image_array_layers: u32,
    pub image_usage: u32,
    pub present_mode: VkPresentModeKHR,
    pub old_swapchain: VkHandle,
}

/// Swapchain image — maps to a framebuffer region
#[derive(Debug)]
pub struct SwapchainImage {
    pub handle: VkHandle,
    /// Byte offset into GPU VRAM where this image lives
    pub vram_offset: u64,
    /// Size in bytes
    pub size: u64,
}

/// A VkSwapchainKHR — double/triple buffered present target
pub struct VkSwapchain {
    pub handle: VkHandle,
    pub surface: VkHandle,
    pub format: VkFormat,
    pub extent: VkExtent2D,
    pub present_mode: VkPresentModeKHR,
    pub images: Vec<SwapchainImage>,
    /// Index of the currently acquired image (-1 if none)
    pub current_image: i32,
    /// Monotonic frame counter
    pub frame_count: u64,
}

/// Present info for vkQueuePresentKHR
#[derive(Debug)]
pub struct VkPresentInfoKHR {
    pub s_type: VkStructureType,
    pub wait_semaphores: Vec<VkHandle>,
    pub swapchains: Vec<VkHandle>,
    pub image_indices: Vec<u32>,
}

/// Global swapchain registry
static SWAPCHAINS: Mutex<Vec<VkSwapchain>> = Mutex::new(Vec::new());

/// vkCreateSwapchainKHR — create a swapchain targeting the ClaudioOS framebuffer
pub fn vk_create_swapchain_khr(
    _device: VkHandle,
    create_info: &VkSwapchainCreateInfoKHR,
) -> Result<VkHandle, VkResult> {
    info!(
        "vulkan: vkCreateSwapchainKHR {}x{} format={:?} present={:?} images={}",
        create_info.image_extent.width, create_info.image_extent.height,
        create_info.image_format, create_info.present_mode,
        create_info.min_image_count
    );

    let swapchain_handle = alloc_handle();
    let image_count = create_info.min_image_count.max(2).min(4); // 2-4 images

    let bytes_per_pixel = match create_info.image_format {
        VkFormat::B8G8R8A8Unorm | VkFormat::B8G8R8A8Srgb
        | VkFormat::R8G8B8A8Unorm | VkFormat::R8G8B8A8Srgb => 4u64,
        VkFormat::R16G16B16A16Sfloat => 8,
        _ => 4,
    };

    let image_size = create_info.image_extent.width as u64
        * create_info.image_extent.height as u64
        * bytes_per_pixel;

    // Allocate swapchain images in VRAM — sequential layout
    let mut images = Vec::with_capacity(image_count as usize);
    for i in 0..image_count {
        images.push(SwapchainImage {
            handle: alloc_handle(),
            vram_offset: i as u64 * image_size, // TODO: real VRAM allocator
            size: image_size,
        });
    }

    // If there's an old swapchain, retire it
    if !create_info.old_swapchain.is_null() {
        debug!("vulkan: retiring old swapchain {:?}", create_info.old_swapchain);
        vk_destroy_swapchain_khr(_device, create_info.old_swapchain);
    }

    let swapchain = VkSwapchain {
        handle: swapchain_handle,
        surface: create_info.surface,
        format: create_info.image_format,
        extent: create_info.image_extent,
        present_mode: create_info.present_mode,
        images,
        current_image: -1,
        frame_count: 0,
    };

    SWAPCHAINS.lock().push(swapchain);

    info!("vulkan: swapchain created, handle={:?}, {} images of {} bytes each",
          swapchain_handle, image_count, image_size);

    Ok(swapchain_handle)
}

/// vkDestroySwapchainKHR — destroy a swapchain
pub fn vk_destroy_swapchain_khr(_device: VkHandle, swapchain: VkHandle) -> VkResult {
    debug!("vulkan: vkDestroySwapchainKHR handle={:?}", swapchain);
    let mut chains = SWAPCHAINS.lock();
    if let Some(pos) = chains.iter().position(|s| s.handle == swapchain) {
        chains.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorSurfaceLostKhr
    }
}

/// vkGetSwapchainImagesKHR — get handles to swapchain images
pub fn vk_get_swapchain_images_khr(
    _device: VkHandle,
    swapchain: VkHandle,
) -> Result<Vec<VkHandle>, VkResult> {
    let chains = SWAPCHAINS.lock();
    let sc = chains.iter()
        .find(|s| s.handle == swapchain)
        .ok_or(VkResult::ErrorSurfaceLostKhr)?;

    Ok(sc.images.iter().map(|img| img.handle).collect())
}

/// vkAcquireNextImageKHR — acquire the next available swapchain image
///
/// Returns the index of the acquired image. The associated semaphore/fence
/// will be signaled when the image is ready for rendering.
pub fn vk_acquire_next_image_khr(
    _device: VkHandle,
    swapchain: VkHandle,
    _timeout: u64,
    _semaphore: VkHandle,
    _fence: VkHandle,
) -> Result<u32, VkResult> {
    let mut chains = SWAPCHAINS.lock();
    let sc = chains.iter_mut()
        .find(|s| s.handle == swapchain)
        .ok_or(VkResult::ErrorSurfaceLostKhr)?;

    // Simple round-robin — advance to next image
    let next = ((sc.current_image + 1) as u32) % sc.images.len() as u32;
    sc.current_image = next as i32;

    debug!("vulkan: vkAcquireNextImageKHR -> image index {}", next);

    // TODO: signal the semaphore/fence via GPU engine
    Ok(next)
}

/// vkQueuePresentKHR — present a rendered image to the display
///
/// This triggers a blit from the swapchain image's VRAM region to the
/// ClaudioOS GOP framebuffer. The copy is submitted to the GPU's copy engine.
pub fn vk_queue_present_khr(
    _queue: VkHandle,
    present_info: &VkPresentInfoKHR,
) -> VkResult {
    let mut chains = SWAPCHAINS.lock();

    for (i, &sc_handle) in present_info.swapchains.iter().enumerate() {
        let image_index = present_info.image_indices.get(i).copied().unwrap_or(0);

        if let Some(sc) = chains.iter_mut().find(|s| s.handle == sc_handle) {
            sc.frame_count += 1;

            debug!(
                "vulkan: present swapchain {:?} image {} (frame #{})",
                sc_handle, image_index, sc.frame_count
            );

            // TODO: submit GPU blit from swapchain image VRAM to framebuffer
            // This would use claudio-gpu's copy engine:
            //   1. Wait on present_info.wait_semaphores
            //   2. DMA copy from sc.images[image_index].vram_offset to framebuffer base
            //   3. Flip scanout pointer (if hardware supports it)
        }
    }

    VkResult::Success
}
