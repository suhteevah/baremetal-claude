//! Vulkan image and image view management — vkCreateImage, vkDestroyImage,
//! vkBindImageMemory, vkCreateImageView. Texture and render target support.

use alloc::vec::Vec;
use spin::Mutex;
use log::{info, debug};

use crate::{
    VkResult, VkHandle, VkFormat, VkExtent3D, VkStructureType, alloc_handle,
    buffer::MemoryRequirements,
};

/// Image type (dimensionality)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkImageType {
    Type1D = 0,
    Type2D = 1,
    Type3D = 2,
}

/// Image tiling mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkImageTiling {
    Optimal = 0,
    Linear = 1,
}

/// Image layout
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkImageLayout {
    Undefined = 0,
    General = 1,
    ColorAttachmentOptimal = 2,
    DepthStencilAttachmentOptimal = 3,
    DepthStencilReadOnlyOptimal = 4,
    ShaderReadOnlyOptimal = 5,
    TransferSrcOptimal = 6,
    TransferDstOptimal = 7,
    PresentSrcKhr = 1000001002,
}

/// Image creation parameters
#[derive(Debug, Clone)]
pub struct VkImageCreateInfo {
    pub s_type: VkStructureType,
    pub image_type: VkImageType,
    pub format: VkFormat,
    pub extent: VkExtent3D,
    pub mip_levels: u32,
    pub array_layers: u32,
    pub samples: u32,
    pub tiling: VkImageTiling,
    pub usage: u32,
    pub initial_layout: VkImageLayout,
}

/// Image view type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkImageViewType {
    Type1D = 0,
    Type2D = 1,
    Type3D = 2,
    Cube = 3,
    Array1D = 4,
    Array2D = 5,
    CubeArray = 6,
}

/// Component swizzle for image views
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkComponentSwizzle {
    Identity = 0,
    Zero = 1,
    One = 2,
    R = 3,
    G = 4,
    B = 5,
    A = 6,
}

/// Component mapping
#[derive(Debug, Clone, Copy)]
pub struct VkComponentMapping {
    pub r: VkComponentSwizzle,
    pub g: VkComponentSwizzle,
    pub b: VkComponentSwizzle,
    pub a: VkComponentSwizzle,
}

impl Default for VkComponentMapping {
    fn default() -> Self {
        Self {
            r: VkComponentSwizzle::Identity,
            g: VkComponentSwizzle::Identity,
            b: VkComponentSwizzle::Identity,
            a: VkComponentSwizzle::Identity,
        }
    }
}

/// Image subresource range
#[derive(Debug, Clone, Copy)]
pub struct VkImageSubresourceRange {
    pub aspect_mask: u32,
    pub base_mip_level: u32,
    pub level_count: u32,
    pub base_array_layer: u32,
    pub layer_count: u32,
}

/// Image aspect flags
pub const VK_IMAGE_ASPECT_COLOR_BIT: u32 = 0x01;
pub const VK_IMAGE_ASPECT_DEPTH_BIT: u32 = 0x02;
pub const VK_IMAGE_ASPECT_STENCIL_BIT: u32 = 0x04;

/// Image view creation parameters
#[derive(Debug, Clone)]
pub struct VkImageViewCreateInfo {
    pub s_type: VkStructureType,
    pub image: VkHandle,
    pub view_type: VkImageViewType,
    pub format: VkFormat,
    pub components: VkComponentMapping,
    pub subresource_range: VkImageSubresourceRange,
}

/// A Vulkan image object
pub struct VkImage {
    pub handle: VkHandle,
    pub image_type: VkImageType,
    pub format: VkFormat,
    pub extent: VkExtent3D,
    pub mip_levels: u32,
    pub array_layers: u32,
    pub samples: u32,
    pub tiling: VkImageTiling,
    pub usage: u32,
    pub current_layout: VkImageLayout,
    /// Bound device memory
    pub bound_memory: Option<VkHandle>,
    pub memory_offset: u64,
}

/// A Vulkan image view — a typed lens into an image
pub struct VkImageView {
    pub handle: VkHandle,
    pub image: VkHandle,
    pub view_type: VkImageViewType,
    pub format: VkFormat,
    pub components: VkComponentMapping,
    pub subresource_range: VkImageSubresourceRange,
}

static IMAGES: Mutex<Vec<VkImage>> = Mutex::new(Vec::new());
static IMAGE_VIEWS: Mutex<Vec<VkImageView>> = Mutex::new(Vec::new());

/// Calculate bytes per pixel for a format
fn bytes_per_pixel(format: VkFormat) -> u64 {
    match format {
        VkFormat::R8G8B8A8Unorm | VkFormat::R8G8B8A8Srgb
        | VkFormat::B8G8R8A8Unorm | VkFormat::B8G8R8A8Srgb => 4,
        VkFormat::R16G16B16A16Sfloat => 8,
        VkFormat::R32Sfloat => 4,
        VkFormat::R32G32Sfloat => 8,
        VkFormat::R32G32B32Sfloat => 12,
        VkFormat::R32G32B32A32Sfloat => 16,
        VkFormat::D16Unorm => 2,
        VkFormat::D32Sfloat => 4,
        VkFormat::D24UnormS8Uint => 4,
        VkFormat::D32SfloatS8Uint => 8,
        _ => 4,
    }
}

/// vkCreateImage — create an image object
pub fn vk_create_image(
    _device: VkHandle,
    create_info: &VkImageCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();

    info!(
        "vulkan: vkCreateImage {:?} {}x{}x{} format={:?} mips={} layers={} samples={}",
        create_info.image_type,
        create_info.extent.width, create_info.extent.height, create_info.extent.depth,
        create_info.format, create_info.mip_levels, create_info.array_layers,
        create_info.samples
    );

    let image = VkImage {
        handle,
        image_type: create_info.image_type,
        format: create_info.format,
        extent: create_info.extent,
        mip_levels: create_info.mip_levels,
        array_layers: create_info.array_layers,
        samples: create_info.samples,
        tiling: create_info.tiling,
        usage: create_info.usage,
        current_layout: create_info.initial_layout,
        bound_memory: None,
        memory_offset: 0,
    };

    IMAGES.lock().push(image);
    Ok(handle)
}

/// vkDestroyImage — destroy an image object
pub fn vk_destroy_image(_device: VkHandle, image: VkHandle) -> VkResult {
    debug!("vulkan: vkDestroyImage handle={:?}", image);
    let mut images = IMAGES.lock();
    if let Some(pos) = images.iter().position(|i| i.handle == image) {
        images.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// vkBindImageMemory — bind device memory to an image
pub fn vk_bind_image_memory(
    _device: VkHandle,
    image: VkHandle,
    memory: VkHandle,
    memory_offset: u64,
) -> VkResult {
    let mut images = IMAGES.lock();
    if let Some(img) = images.iter_mut().find(|i| i.handle == image) {
        img.bound_memory = Some(memory);
        img.memory_offset = memory_offset;
        debug!("vulkan: vkBindImageMemory image={:?} memory={:?} offset={}", image, memory, memory_offset);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// vkCreateImageView — create a view into an image
pub fn vk_create_image_view(
    _device: VkHandle,
    create_info: &VkImageViewCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();

    debug!(
        "vulkan: vkCreateImageView for image {:?}, type={:?}, format={:?}",
        create_info.image, create_info.view_type, create_info.format
    );

    let view = VkImageView {
        handle,
        image: create_info.image,
        view_type: create_info.view_type,
        format: create_info.format,
        components: create_info.components,
        subresource_range: create_info.subresource_range,
    };

    IMAGE_VIEWS.lock().push(view);
    Ok(handle)
}

/// vkDestroyImageView — destroy an image view
pub fn vk_destroy_image_view(_device: VkHandle, view: VkHandle) -> VkResult {
    let mut views = IMAGE_VIEWS.lock();
    if let Some(pos) = views.iter().position(|v| v.handle == view) {
        views.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// Get image memory requirements
pub fn vk_get_image_memory_requirements(
    _device: VkHandle,
    image: VkHandle,
) -> Result<MemoryRequirements, VkResult> {
    let images = IMAGES.lock();
    let img = images.iter()
        .find(|i| i.handle == image)
        .ok_or(VkResult::ErrorOutOfHostMemory)?;

    let bpp = bytes_per_pixel(img.format);
    let mut total_size = 0u64;

    // Calculate size for all mip levels
    for mip in 0..img.mip_levels {
        let w = (img.extent.width >> mip).max(1) as u64;
        let h = (img.extent.height >> mip).max(1) as u64;
        let d = (img.extent.depth >> mip).max(1) as u64;
        total_size += w * h * d * bpp * img.array_layers as u64;
    }

    // MSAA multiplier
    total_size *= img.samples as u64;

    Ok(MemoryRequirements {
        size: (total_size + 4095) & !4095, // Page-aligned
        alignment: 4096,
        memory_type_bits: 0b111,
    })
}
