//! Vulkan pipeline management — vkCreateGraphicsPipelines,
//! vkCreateComputePipelines, pipeline layout, descriptor set layouts.

use alloc::vec::Vec;
use spin::Mutex;
use log::{info, debug};

use crate::{
    VkResult, VkHandle, VkFormat, VkStructureType, VkViewport, VkRect2D,
    alloc_handle,
};
use crate::shader::VkShaderStageFlagBits;

// ============================================================================
// Pipeline layout
// ============================================================================

/// Push constant range
#[derive(Debug, Clone)]
pub struct VkPushConstantRange {
    pub stage_flags: u32,
    pub offset: u32,
    pub size: u32,
}

/// Pipeline layout creation info
#[derive(Debug, Clone)]
pub struct VkPipelineLayoutCreateInfo {
    pub s_type: VkStructureType,
    pub set_layouts: Vec<VkHandle>,
    pub push_constant_ranges: Vec<VkPushConstantRange>,
}

/// A pipeline layout — defines the interface between shader stages and resources
pub struct VkPipelineLayout {
    pub handle: VkHandle,
    pub set_layouts: Vec<VkHandle>,
    pub push_constant_ranges: Vec<VkPushConstantRange>,
}

// ============================================================================
// Shader stage
// ============================================================================

/// Pipeline shader stage create info
#[derive(Debug, Clone)]
pub struct VkPipelineShaderStageCreateInfo {
    pub stage: VkShaderStageFlagBits,
    pub module: VkHandle,
    pub entry_point: alloc::string::String,
}

// ============================================================================
// Vertex input
// ============================================================================

/// Vertex input binding description
#[derive(Debug, Clone)]
pub struct VkVertexInputBindingDescription {
    pub binding: u32,
    pub stride: u32,
    pub input_rate: VkVertexInputRate,
}

/// Vertex input rate
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkVertexInputRate {
    Vertex = 0,
    Instance = 1,
}

/// Vertex input attribute description
#[derive(Debug, Clone)]
pub struct VkVertexInputAttributeDescription {
    pub location: u32,
    pub binding: u32,
    pub format: VkFormat,
    pub offset: u32,
}

// ============================================================================
// Input assembly
// ============================================================================

/// Primitive topology
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkPrimitiveTopology {
    PointList = 0,
    LineList = 1,
    LineStrip = 2,
    TriangleList = 3,
    TriangleStrip = 4,
    TriangleFan = 5,
    PatchList = 10,
}

// ============================================================================
// Rasterization
// ============================================================================

/// Polygon mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkPolygonMode {
    Fill = 0,
    Line = 1,
    Point = 2,
}

/// Cull mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkCullMode {
    None = 0,
    Front = 1,
    Back = 2,
    FrontAndBack = 3,
}

/// Front face winding order
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkFrontFace {
    CounterClockwise = 0,
    Clockwise = 1,
}

// ============================================================================
// Blend state
// ============================================================================

/// Blend factor
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkBlendFactor {
    Zero = 0,
    One = 1,
    SrcColor = 2,
    OneMinusSrcColor = 3,
    DstColor = 4,
    OneMinusDstColor = 5,
    SrcAlpha = 6,
    OneMinusSrcAlpha = 7,
    DstAlpha = 8,
    OneMinusDstAlpha = 9,
}

/// Blend operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkBlendOp {
    Add = 0,
    Subtract = 1,
    ReverseSubtract = 2,
    Min = 3,
    Max = 4,
}

/// Compare operation (for depth/stencil)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkCompareOp {
    Never = 0,
    Less = 1,
    Equal = 2,
    LessOrEqual = 3,
    Greater = 4,
    NotEqual = 5,
    GreaterOrEqual = 6,
    Always = 7,
}

// ============================================================================
// Graphics pipeline
// ============================================================================

/// Graphics pipeline creation info
#[derive(Debug, Clone)]
pub struct VkGraphicsPipelineCreateInfo {
    pub s_type: VkStructureType,
    pub stages: Vec<VkPipelineShaderStageCreateInfo>,
    pub vertex_bindings: Vec<VkVertexInputBindingDescription>,
    pub vertex_attributes: Vec<VkVertexInputAttributeDescription>,
    pub topology: VkPrimitiveTopology,
    pub primitive_restart_enable: bool,
    pub viewports: Vec<VkViewport>,
    pub scissors: Vec<VkRect2D>,
    pub polygon_mode: VkPolygonMode,
    pub cull_mode: VkCullMode,
    pub front_face: VkFrontFace,
    pub depth_bias_enable: bool,
    pub line_width: f32,
    pub depth_test_enable: bool,
    pub depth_write_enable: bool,
    pub depth_compare_op: VkCompareOp,
    pub layout: VkHandle,
    pub render_pass: VkHandle,
    pub subpass: u32,
}

/// Compute pipeline creation info
#[derive(Debug, Clone)]
pub struct VkComputePipelineCreateInfo {
    pub s_type: VkStructureType,
    pub stage: VkPipelineShaderStageCreateInfo,
    pub layout: VkHandle,
}

/// A compiled pipeline (graphics or compute)
pub struct VkPipeline {
    pub handle: VkHandle,
    pub is_compute: bool,
    pub layout: VkHandle,
    /// Compiled GPU-native shader microcode (would be produced by SPIR-V translator)
    pub compiled_shaders: Vec<CompiledShader>,
}

/// A compiled shader ready for GPU execution
#[derive(Debug, Clone)]
pub struct CompiledShader {
    pub stage: VkShaderStageFlagBits,
    /// GPU-native microcode bytes
    pub microcode: Vec<u8>,
}

static PIPELINE_LAYOUTS: Mutex<Vec<VkPipelineLayout>> = Mutex::new(Vec::new());
static PIPELINES: Mutex<Vec<VkPipeline>> = Mutex::new(Vec::new());

/// vkCreatePipelineLayout
pub fn vk_create_pipeline_layout(
    _device: VkHandle,
    create_info: &VkPipelineLayoutCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();

    debug!(
        "vulkan: vkCreatePipelineLayout {} set layouts, {} push constant ranges",
        create_info.set_layouts.len(), create_info.push_constant_ranges.len()
    );

    let layout = VkPipelineLayout {
        handle,
        set_layouts: create_info.set_layouts.clone(),
        push_constant_ranges: create_info.push_constant_ranges.clone(),
    };

    PIPELINE_LAYOUTS.lock().push(layout);
    Ok(handle)
}

/// vkDestroyPipelineLayout
pub fn vk_destroy_pipeline_layout(_device: VkHandle, layout: VkHandle) -> VkResult {
    let mut layouts = PIPELINE_LAYOUTS.lock();
    if let Some(pos) = layouts.iter().position(|l| l.handle == layout) {
        layouts.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// vkCreateGraphicsPipelines — compile and create graphics pipeline(s)
///
/// This is where SPIR-V gets translated to NVIDIA GPU microcode via the
/// shader compiler. In a full implementation, each shader stage's SPIR-V
/// would be compiled to NV ISA targeting the specific SM version (e.g., SM 8.6
/// for GA104).
pub fn vk_create_graphics_pipelines(
    _device: VkHandle,
    create_infos: &[VkGraphicsPipelineCreateInfo],
) -> Result<Vec<VkHandle>, VkResult> {
    let mut handles = Vec::with_capacity(create_infos.len());

    for (i, ci) in create_infos.iter().enumerate() {
        let handle = alloc_handle();

        info!(
            "vulkan: vkCreateGraphicsPipelines[{}] {} stages, topology={:?}, cull={:?}",
            i, ci.stages.len(), ci.topology, ci.cull_mode
        );

        // TODO: compile SPIR-V to GPU microcode here
        // For each stage:
        //   1. Fetch VkShaderModule by ci.stages[n].module
        //   2. Run SPIR-V -> NIR -> NV ISA compiler
        //   3. Store compiled microcode
        let compiled_shaders = Vec::new();

        let pipeline = VkPipeline {
            handle,
            is_compute: false,
            layout: ci.layout,
            compiled_shaders,
        };

        PIPELINES.lock().push(pipeline);
        handles.push(handle);
    }

    Ok(handles)
}

/// vkCreateComputePipelines — compile and create compute pipeline(s)
pub fn vk_create_compute_pipelines(
    _device: VkHandle,
    create_infos: &[VkComputePipelineCreateInfo],
) -> Result<Vec<VkHandle>, VkResult> {
    let mut handles = Vec::with_capacity(create_infos.len());

    for (i, ci) in create_infos.iter().enumerate() {
        let handle = alloc_handle();

        info!(
            "vulkan: vkCreateComputePipelines[{}] entry='{}'",
            i, ci.stage.entry_point
        );

        let pipeline = VkPipeline {
            handle,
            is_compute: true,
            layout: ci.layout,
            compiled_shaders: Vec::new(),
        };

        PIPELINES.lock().push(pipeline);
        handles.push(handle);
    }

    Ok(handles)
}

/// vkDestroyPipeline
pub fn vk_destroy_pipeline(_device: VkHandle, pipeline: VkHandle) -> VkResult {
    let mut pipelines = PIPELINES.lock();
    if let Some(pos) = pipelines.iter().position(|p| p.handle == pipeline) {
        pipelines.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}
