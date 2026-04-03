//! Render pass and framebuffer management — vkCreateRenderPass,
//! vkCreateFramebuffer, vkCmdBeginRenderPass, vkCmdEndRenderPass.

use alloc::vec::Vec;
use spin::Mutex;
use log::{info, debug};

use crate::{
    VkResult, VkHandle, VkFormat, VkStructureType, alloc_handle,
};
use crate::image::VkImageLayout;

/// Attachment load operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkAttachmentLoadOp {
    Load = 0,
    Clear = 1,
    DontCare = 2,
}

/// Attachment store operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkAttachmentStoreOp {
    Store = 0,
    DontCare = 1,
}

/// Attachment description
#[derive(Debug, Clone)]
pub struct VkAttachmentDescription {
    pub format: VkFormat,
    pub samples: u32,
    pub load_op: VkAttachmentLoadOp,
    pub store_op: VkAttachmentStoreOp,
    pub stencil_load_op: VkAttachmentLoadOp,
    pub stencil_store_op: VkAttachmentStoreOp,
    pub initial_layout: VkImageLayout,
    pub final_layout: VkImageLayout,
}

/// Attachment reference within a subpass
#[derive(Debug, Clone, Copy)]
pub struct VkAttachmentReference {
    pub attachment: u32,
    pub layout: VkImageLayout,
}

/// Subpass description
#[derive(Debug, Clone)]
pub struct VkSubpassDescription {
    pub pipeline_bind_point: VkPipelineBindPoint,
    pub input_attachments: Vec<VkAttachmentReference>,
    pub color_attachments: Vec<VkAttachmentReference>,
    pub resolve_attachments: Vec<VkAttachmentReference>,
    pub depth_stencil_attachment: Option<VkAttachmentReference>,
    pub preserve_attachments: Vec<u32>,
}

/// Pipeline bind point
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkPipelineBindPoint {
    Graphics = 0,
    Compute = 1,
}

/// Subpass dependency for execution/memory ordering
#[derive(Debug, Clone)]
pub struct VkSubpassDependency {
    pub src_subpass: u32,
    pub dst_subpass: u32,
    pub src_stage_mask: u32,
    pub dst_stage_mask: u32,
    pub src_access_mask: u32,
    pub dst_access_mask: u32,
}

/// Render pass creation info
#[derive(Debug, Clone)]
pub struct VkRenderPassCreateInfo {
    pub s_type: VkStructureType,
    pub attachments: Vec<VkAttachmentDescription>,
    pub subpasses: Vec<VkSubpassDescription>,
    pub dependencies: Vec<VkSubpassDependency>,
}

/// Framebuffer creation info
#[derive(Debug, Clone)]
pub struct VkFramebufferCreateInfo {
    pub s_type: VkStructureType,
    pub render_pass: VkHandle,
    pub attachments: Vec<VkHandle>,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
}

/// A render pass defines the structure of rendering operations
pub struct VkRenderPass {
    pub handle: VkHandle,
    pub attachments: Vec<VkAttachmentDescription>,
    pub subpasses: Vec<VkSubpassDescription>,
    pub dependencies: Vec<VkSubpassDependency>,
}

/// A framebuffer binds image views to render pass attachments
pub struct VkFramebuffer {
    pub handle: VkHandle,
    pub render_pass: VkHandle,
    pub attachments: Vec<VkHandle>,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
}

static RENDER_PASSES: Mutex<Vec<VkRenderPass>> = Mutex::new(Vec::new());
static FRAMEBUFFERS: Mutex<Vec<VkFramebuffer>> = Mutex::new(Vec::new());

/// vkCreateRenderPass
pub fn vk_create_render_pass(
    _device: VkHandle,
    create_info: &VkRenderPassCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();

    info!(
        "vulkan: vkCreateRenderPass {} attachments, {} subpasses, {} dependencies",
        create_info.attachments.len(),
        create_info.subpasses.len(),
        create_info.dependencies.len()
    );

    for (i, att) in create_info.attachments.iter().enumerate() {
        debug!(
            "vulkan:   attachment[{}]: format={:?} load={:?} store={:?} {:?}->{:?}",
            i, att.format, att.load_op, att.store_op,
            att.initial_layout, att.final_layout
        );
    }

    let rp = VkRenderPass {
        handle,
        attachments: create_info.attachments.clone(),
        subpasses: create_info.subpasses.clone(),
        dependencies: create_info.dependencies.clone(),
    };

    RENDER_PASSES.lock().push(rp);
    Ok(handle)
}

/// vkDestroyRenderPass
pub fn vk_destroy_render_pass(_device: VkHandle, render_pass: VkHandle) -> VkResult {
    let mut passes = RENDER_PASSES.lock();
    if let Some(pos) = passes.iter().position(|r| r.handle == render_pass) {
        passes.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// vkCreateFramebuffer
pub fn vk_create_framebuffer(
    _device: VkHandle,
    create_info: &VkFramebufferCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();

    debug!(
        "vulkan: vkCreateFramebuffer {}x{} layers={} attachments={}",
        create_info.width, create_info.height,
        create_info.layers, create_info.attachments.len()
    );

    let fb = VkFramebuffer {
        handle,
        render_pass: create_info.render_pass,
        attachments: create_info.attachments.clone(),
        width: create_info.width,
        height: create_info.height,
        layers: create_info.layers,
    };

    FRAMEBUFFERS.lock().push(fb);
    Ok(handle)
}

/// vkDestroyFramebuffer
pub fn vk_destroy_framebuffer(_device: VkHandle, framebuffer: VkHandle) -> VkResult {
    let mut fbs = FRAMEBUFFERS.lock();
    if let Some(pos) = fbs.iter().position(|f| f.handle == framebuffer) {
        fbs.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}
