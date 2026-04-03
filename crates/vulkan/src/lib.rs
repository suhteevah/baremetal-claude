//! # claudio-vulkan — Vulkan 1.3 implementation for ClaudioOS
//!
//! Provides a Vulkan ICD (Installable Client Driver) that targets the bare-metal
//! NVIDIA GPU driver in `claudio-gpu`. This is not a loader — it IS the driver,
//! implementing Vulkan entry points directly against our GPU hardware abstraction.
//!
//! ## Architecture
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────┐
//! │                    Application / DXVK                       │
//! ├────────────────────────────────────────────────────────────┤
//! │                  dispatch.rs                                │
//! │  (vkGetInstanceProcAddr, vkGetDeviceProcAddr, ICD table)   │
//! ├──────────┬──────────┬──────────┬──────────┬───────────────┤
//! │instance  │ device   │ commands │ pipeline │  swapchain    │
//! │  .rs     │  .rs     │  .rs     │  .rs     │   .rs         │
//! ├──────────┼──────────┼──────────┼──────────┼───────────────┤
//! │renderpass│descriptor│  sync    │  shader  │  surface      │
//! │  .rs     │  .rs     │  .rs     │  .rs     │   .rs         │
//! ├──────────┴──────────┴──────────┴──────────┴───────────────┤
//! │  memory.rs  │  buffer.rs  │  image.rs                      │
//! ├─────────────┴─────────────┴────────────────────────────────┤
//! │                   claudio-gpu (hardware)                    │
//! └────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Vulkan Version
//!
//! We target Vulkan 1.3 core with the following extensions:
//! - `VK_KHR_swapchain` — present to ClaudioOS framebuffer
//! - `VK_KHR_surface` — ClaudioOS surface abstraction
//!
//! ## Status
//!
//! This is the structural foundation for a Vulkan driver. The entry points are
//! defined with correct signatures and dispatch plumbing. Actual GPU command
//! submission flows through `claudio-gpu`'s FIFO/compute infrastructure.

#![no_std]

extern crate alloc;

pub mod instance;
pub mod device;
pub mod swapchain;
pub mod memory;
pub mod buffer;
pub mod image;
pub mod shader;
pub mod pipeline;
pub mod commands;
pub mod renderpass;
pub mod descriptor;
pub mod sync;
pub mod surface;
pub mod dispatch;

use alloc::string::String;

// ============================================================================
// Vulkan core types
// ============================================================================

/// Vulkan API version encoding: variant(3) | major(7) | minor(10) | patch(12)
pub const fn vk_make_api_version(variant: u32, major: u32, minor: u32, patch: u32) -> u32 {
    (variant << 29) | (major << 22) | (minor << 12) | patch
}

/// Our advertised Vulkan version: 1.3.0
pub const VK_API_VERSION_1_3: u32 = vk_make_api_version(0, 1, 3, 0);

/// Vulkan result codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum VkResult {
    Success = 0,
    NotReady = 1,
    Timeout = 2,
    EventSet = 3,
    EventReset = 4,
    Incomplete = 5,
    ErrorOutOfHostMemory = -1,
    ErrorOutOfDeviceMemory = -2,
    ErrorInitializationFailed = -3,
    ErrorDeviceLost = -4,
    ErrorMemoryMapFailed = -5,
    ErrorLayerNotPresent = -6,
    ErrorExtensionNotPresent = -7,
    ErrorFeatureNotPresent = -8,
    ErrorIncompatibleDriver = -9,
    ErrorTooManyObjects = -10,
    ErrorFormatNotSupported = -11,
    ErrorFragmentedPool = -12,
    ErrorSurfaceLostKhr = -1000000000,
    ErrorOutOfDateKhr = -1000001003,
    SuboptimalKhr = 1000001003,
}

/// Opaque handle type — all Vulkan handles are u64 internally
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct VkHandle(pub u64);

impl VkHandle {
    pub const NULL: VkHandle = VkHandle(0);

    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

/// Vulkan structure type tags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkStructureType {
    ApplicationInfo = 0,
    InstanceCreateInfo = 1,
    DeviceQueueCreateInfo = 2,
    DeviceCreateInfo = 3,
    SubmitInfo = 4,
    MemoryAllocateInfo = 5,
    FenceCreateInfo = 8,
    SemaphoreCreateInfo = 9,
    BufferCreateInfo = 12,
    ImageCreateInfo = 14,
    ImageViewCreateInfo = 15,
    ShaderModuleCreateInfo = 16,
    PipelineLayoutCreateInfo = 30,
    RenderPassCreateInfo = 38,
    GraphicsPipelineCreateInfo = 28,
    ComputePipelineCreateInfo = 29,
    DescriptorSetLayoutCreateInfo = 32,
    DescriptorPoolCreateInfo = 33,
    FramebufferCreateInfo = 37,
    CommandPoolCreateInfo = 39,
    CommandBufferAllocateInfo = 40,
    CommandBufferBeginInfo = 42,
    RenderPassBeginInfo = 43,
    SwapchainCreateInfoKhr = 1000001000,
    PresentInfoKhr = 1000001001,
    SurfaceCreateInfoClaudio = 1000900000,
}

/// Vulkan format enumeration (subset of commonly used formats)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkFormat {
    Undefined = 0,
    R8G8B8A8Unorm = 37,
    R8G8B8A8Srgb = 43,
    B8G8R8A8Unorm = 44,
    B8G8R8A8Srgb = 50,
    R16G16B16A16Sfloat = 97,
    R32Sfloat = 100,
    R32G32Sfloat = 103,
    R32G32B32Sfloat = 106,
    R32G32B32A32Sfloat = 109,
    D16Unorm = 124,
    D32Sfloat = 126,
    D24UnormS8Uint = 129,
    D32SfloatS8Uint = 130,
}

/// Extent2D
#[derive(Debug, Clone, Copy)]
pub struct VkExtent2D {
    pub width: u32,
    pub height: u32,
}

/// Extent3D
#[derive(Debug, Clone, Copy)]
pub struct VkExtent3D {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
}

/// Viewport
#[derive(Debug, Clone, Copy)]
pub struct VkViewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

/// Scissor rectangle
#[derive(Debug, Clone, Copy)]
pub struct VkRect2D {
    pub offset_x: i32,
    pub offset_y: i32,
    pub extent: VkExtent2D,
}

/// Physical device type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkPhysicalDeviceType {
    Other = 0,
    IntegratedGpu = 1,
    DiscreteGpu = 2,
    VirtualGpu = 3,
    Cpu = 4,
}

/// Physical device properties
#[derive(Debug, Clone)]
pub struct VkPhysicalDeviceProperties {
    pub api_version: u32,
    pub driver_version: u32,
    pub vendor_id: u32,
    pub device_id: u32,
    pub device_type: VkPhysicalDeviceType,
    pub device_name: String,
    pub pipeline_cache_uuid: [u8; 16],
}

/// Physical device memory properties
#[derive(Debug, Clone)]
pub struct VkPhysicalDeviceMemoryProperties {
    pub memory_type_count: u32,
    pub memory_types: [VkMemoryType; 32],
    pub memory_heap_count: u32,
    pub memory_heaps: [VkMemoryHeap; 16],
}

/// Memory type
#[derive(Debug, Clone, Copy, Default)]
pub struct VkMemoryType {
    pub property_flags: u32,
    pub heap_index: u32,
}

/// Memory heap
#[derive(Debug, Clone, Copy, Default)]
pub struct VkMemoryHeap {
    pub size: u64,
    pub flags: u32,
}

// Memory property flags
pub const VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT: u32 = 0x01;
pub const VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT: u32 = 0x02;
pub const VK_MEMORY_PROPERTY_HOST_COHERENT_BIT: u32 = 0x04;
pub const VK_MEMORY_PROPERTY_HOST_CACHED_BIT: u32 = 0x08;

// Memory heap flags
pub const VK_MEMORY_HEAP_DEVICE_LOCAL_BIT: u32 = 0x01;

/// Queue family properties
#[derive(Debug, Clone, Copy)]
pub struct VkQueueFamilyProperties {
    pub queue_flags: u32,
    pub queue_count: u32,
    pub timestamp_valid_bits: u32,
    pub min_image_transfer_granularity: VkExtent3D,
}

// Queue flags
pub const VK_QUEUE_GRAPHICS_BIT: u32 = 0x01;
pub const VK_QUEUE_COMPUTE_BIT: u32 = 0x02;
pub const VK_QUEUE_TRANSFER_BIT: u32 = 0x04;

/// Pipeline stage flags
pub const VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT: u32 = 0x00000001;
pub const VK_PIPELINE_STAGE_VERTEX_INPUT_BIT: u32 = 0x00000004;
pub const VK_PIPELINE_STAGE_VERTEX_SHADER_BIT: u32 = 0x00000008;
pub const VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT: u32 = 0x00000080;
pub const VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT: u32 = 0x00000400;
pub const VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT: u32 = 0x00000800;
pub const VK_PIPELINE_STAGE_TRANSFER_BIT: u32 = 0x00001000;
pub const VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT: u32 = 0x00002000;
pub const VK_PIPELINE_STAGE_ALL_GRAPHICS_BIT: u32 = 0x00008000;
pub const VK_PIPELINE_STAGE_ALL_COMMANDS_BIT: u32 = 0x00010000;

/// Buffer usage flags
pub const VK_BUFFER_USAGE_TRANSFER_SRC_BIT: u32 = 0x01;
pub const VK_BUFFER_USAGE_TRANSFER_DST_BIT: u32 = 0x02;
pub const VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT: u32 = 0x10;
pub const VK_BUFFER_USAGE_STORAGE_BUFFER_BIT: u32 = 0x20;
pub const VK_BUFFER_USAGE_INDEX_BUFFER_BIT: u32 = 0x40;
pub const VK_BUFFER_USAGE_VERTEX_BUFFER_BIT: u32 = 0x80;

/// Image usage flags
pub const VK_IMAGE_USAGE_TRANSFER_SRC_BIT: u32 = 0x01;
pub const VK_IMAGE_USAGE_TRANSFER_DST_BIT: u32 = 0x02;
pub const VK_IMAGE_USAGE_SAMPLED_BIT: u32 = 0x04;
pub const VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT: u32 = 0x10;
pub const VK_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT_BIT: u32 = 0x20;

/// Global handle counter for generating unique handles
static NEXT_HANDLE: spin::Mutex<u64> = spin::Mutex::new(1);

/// Generate a unique non-null Vulkan handle
pub fn alloc_handle() -> VkHandle {
    let mut counter = NEXT_HANDLE.lock();
    let h = VkHandle(*counter);
    *counter += 1;
    h
}

/// Re-export key types for consumers
pub use instance::VkInstance;
pub use device::VkDevice;
pub use swapchain::VkSwapchain;
pub use memory::DeviceMemory;
pub use buffer::VkBuffer;
pub use image::{VkImage, VkImageView};
pub use shader::VkShaderModule;
pub use pipeline::{VkPipeline, VkPipelineLayout};
pub use commands::{VkCommandPool, VkCommandBuffer};
pub use renderpass::{VkRenderPass, VkFramebuffer};
pub use descriptor::{VkDescriptorSetLayout, VkDescriptorPool, VkDescriptorSet};
pub use sync::{VkFence, VkSemaphore};
pub use surface::VkSurface;
pub use dispatch::DispatchTable;
