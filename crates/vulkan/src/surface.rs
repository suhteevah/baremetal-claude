//! VkSurfaceKHR — ClaudioOS surface abstraction mapping to the GOP framebuffer.
//!
//! In a windowed OS, surfaces represent windows. In ClaudioOS, a surface IS the
//! framebuffer — fullscreen, single-output. This is our custom surface extension
//! (VK_CLAUDIO_surface) that replaces VK_KHR_xcb_surface / VK_KHR_wayland_surface.

use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;
use log::{info, debug};

use crate::{
    VkResult, VkHandle, VkFormat, VkExtent2D, VkStructureType, alloc_handle,
};
use crate::swapchain::{VkColorSpaceKHR, VkPresentModeKHR};

/// Surface creation info for ClaudioOS
#[derive(Debug, Clone)]
pub struct VkClaudioSurfaceCreateInfo {
    pub s_type: VkStructureType,
    /// Framebuffer base physical address (from UEFI GOP)
    pub framebuffer_base: u64,
    /// Framebuffer width in pixels
    pub width: u32,
    /// Framebuffer height in pixels
    pub height: u32,
    /// Stride in bytes per scanline
    pub stride: u32,
    /// Pixel format (typically BGRA8)
    pub format: VkFormat,
}

/// Surface capabilities — what the surface supports
#[derive(Debug, Clone)]
pub struct VkSurfaceCapabilitiesKHR {
    pub min_image_count: u32,
    pub max_image_count: u32,
    pub current_extent: VkExtent2D,
    pub min_image_extent: VkExtent2D,
    pub max_image_extent: VkExtent2D,
    pub max_image_array_layers: u32,
    pub supported_transforms: u32,
    pub current_transform: u32,
    pub supported_composite_alpha: u32,
    pub supported_usage_flags: u32,
}

/// Surface format
#[derive(Debug, Clone)]
pub struct VkSurfaceFormatKHR {
    pub format: VkFormat,
    pub color_space: VkColorSpaceKHR,
}

/// A ClaudioOS surface — the framebuffer as a Vulkan surface
pub struct VkSurface {
    pub handle: VkHandle,
    pub framebuffer_base: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: VkFormat,
}

static SURFACES: Mutex<Vec<VkSurface>> = Mutex::new(Vec::new());

/// vkCreateClaudioSurfaceKHR — create a surface from the GOP framebuffer
pub fn vk_create_claudio_surface(
    _instance: VkHandle,
    create_info: &VkClaudioSurfaceCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();

    info!(
        "vulkan: vkCreateClaudioSurface {}x{} stride={} fb_base={:#x} format={:?}",
        create_info.width, create_info.height,
        create_info.stride, create_info.framebuffer_base,
        create_info.format
    );

    let surface = VkSurface {
        handle,
        framebuffer_base: create_info.framebuffer_base,
        width: create_info.width,
        height: create_info.height,
        stride: create_info.stride,
        format: create_info.format,
    };

    SURFACES.lock().push(surface);
    Ok(handle)
}

/// vkDestroySurfaceKHR
pub fn vk_destroy_surface(_instance: VkHandle, surface: VkHandle) -> VkResult {
    let mut surfaces = SURFACES.lock();
    if let Some(pos) = surfaces.iter().position(|s| s.handle == surface) {
        surfaces.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorSurfaceLostKhr
    }
}

/// vkGetPhysicalDeviceSurfaceCapabilitiesKHR
pub fn vk_get_physical_device_surface_capabilities(
    _physical_device: VkHandle,
    surface: VkHandle,
) -> Result<VkSurfaceCapabilitiesKHR, VkResult> {
    let surfaces = SURFACES.lock();
    let surf = surfaces.iter()
        .find(|s| s.handle == surface)
        .ok_or(VkResult::ErrorSurfaceLostKhr)?;

    Ok(VkSurfaceCapabilitiesKHR {
        min_image_count: 2,
        max_image_count: 4,
        current_extent: VkExtent2D {
            width: surf.width,
            height: surf.height,
        },
        min_image_extent: VkExtent2D {
            width: surf.width,
            height: surf.height,
        },
        max_image_extent: VkExtent2D {
            width: surf.width,
            height: surf.height,
        },
        max_image_array_layers: 1,
        supported_transforms: 0x01, // IDENTITY
        current_transform: 0x01,
        supported_composite_alpha: 0x01, // OPAQUE
        supported_usage_flags: crate::VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT
            | crate::VK_IMAGE_USAGE_TRANSFER_DST_BIT,
    })
}

/// vkGetPhysicalDeviceSurfaceFormatsKHR
pub fn vk_get_physical_device_surface_formats(
    _physical_device: VkHandle,
    _surface: VkHandle,
) -> Result<Vec<VkSurfaceFormatKHR>, VkResult> {
    // ClaudioOS framebuffer supports BGRA8 (native GOP format) and RGBA8
    Ok(vec![
        VkSurfaceFormatKHR {
            format: VkFormat::B8G8R8A8Srgb,
            color_space: VkColorSpaceKHR::SrgbNonlinear,
        },
        VkSurfaceFormatKHR {
            format: VkFormat::B8G8R8A8Unorm,
            color_space: VkColorSpaceKHR::SrgbNonlinear,
        },
        VkSurfaceFormatKHR {
            format: VkFormat::R8G8B8A8Srgb,
            color_space: VkColorSpaceKHR::SrgbNonlinear,
        },
    ])
}

/// vkGetPhysicalDeviceSurfacePresentModesKHR
pub fn vk_get_physical_device_surface_present_modes(
    _physical_device: VkHandle,
    _surface: VkHandle,
) -> Result<Vec<VkPresentModeKHR>, VkResult> {
    // We support immediate (no vsync) and FIFO (vsync)
    Ok(vec![
        VkPresentModeKHR::Immediate,
        VkPresentModeKHR::Fifo,
        VkPresentModeKHR::Mailbox,
    ])
}

/// vkGetPhysicalDeviceSurfaceSupportKHR — check if a queue family supports present
pub fn vk_get_physical_device_surface_support(
    _physical_device: VkHandle,
    queue_family_index: u32,
    _surface: VkHandle,
) -> Result<bool, VkResult> {
    // Queue family 0 (universal) supports presentation
    Ok(queue_family_index == 0)
}
