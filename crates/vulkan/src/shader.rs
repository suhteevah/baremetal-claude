//! SPIR-V shader module management — vkCreateShaderModule.
//!
//! Parses SPIR-V binaries and prepares them for translation to GPU-native
//! shader microcode. The actual compilation happens at pipeline creation time.

use alloc::vec::Vec;
use spin::Mutex;
use log::{info, debug, warn};

use crate::{VkResult, VkHandle, VkStructureType, alloc_handle};

/// SPIR-V magic number
const SPIRV_MAGIC: u32 = 0x07230203;

/// Shader stage flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkShaderStageFlagBits {
    Vertex = 0x01,
    TessellationControl = 0x02,
    TessellationEvaluation = 0x04,
    Geometry = 0x08,
    Fragment = 0x10,
    Compute = 0x20,
    AllGraphics = 0x1F,
    All = 0x7FFFFFFF,
}

/// Shader module creation info
#[derive(Debug)]
pub struct VkShaderModuleCreateInfo {
    pub s_type: VkStructureType,
    /// SPIR-V bytecode (must be 4-byte aligned, multiple of 4 bytes)
    pub code: Vec<u32>,
}

/// SPIR-V execution model (from SPIR-V spec section 3.3)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpirVExecutionModel {
    Vertex,
    TessellationControl,
    TessellationEvaluation,
    Geometry,
    Fragment,
    GLCompute,
    Kernel,
    Unknown(u32),
}

/// Basic SPIR-V module info extracted during parsing
#[derive(Debug, Clone)]
pub struct SpirVModuleInfo {
    pub version_major: u8,
    pub version_minor: u8,
    pub generator: u32,
    pub bound: u32,
    pub entry_points: Vec<SpirVEntryPoint>,
}

/// SPIR-V entry point
#[derive(Debug, Clone)]
pub struct SpirVEntryPoint {
    pub execution_model: SpirVExecutionModel,
    pub name_id: u32,
}

/// A compiled shader module
pub struct VkShaderModule {
    pub handle: VkHandle,
    /// Raw SPIR-V bytecode
    pub spirv_code: Vec<u32>,
    /// Size in bytes
    pub code_size: usize,
    /// Parsed module info
    pub module_info: Option<SpirVModuleInfo>,
}

static SHADER_MODULES: Mutex<Vec<VkShaderModule>> = Mutex::new(Vec::new());

/// Parse SPIR-V header and extract basic module information
fn parse_spirv_header(code: &[u32]) -> Option<SpirVModuleInfo> {
    if code.len() < 5 {
        warn!("vulkan: SPIR-V too short ({} words)", code.len());
        return None;
    }

    if code[0] != SPIRV_MAGIC {
        warn!("vulkan: invalid SPIR-V magic: {:#010x} (expected {:#010x})", code[0], SPIRV_MAGIC);
        return None;
    }

    let version = code[1];
    let version_major = ((version >> 16) & 0xFF) as u8;
    let version_minor = ((version >> 8) & 0xFF) as u8;
    let generator = code[2];
    let bound = code[3];
    // code[4] is reserved (schema)

    let mut entry_points = Vec::new();

    // Walk instructions looking for OpEntryPoint (opcode 15)
    let mut offset = 5;
    while offset < code.len() {
        let word = code[offset];
        let word_count = (word >> 16) as usize;
        let opcode = word & 0xFFFF;

        if word_count == 0 {
            break; // Malformed
        }

        // OpEntryPoint = 15
        if opcode == 15 && offset + 2 < code.len() {
            let exec_model_raw = code[offset + 1];
            let execution_model = match exec_model_raw {
                0 => SpirVExecutionModel::Vertex,
                1 => SpirVExecutionModel::TessellationControl,
                2 => SpirVExecutionModel::TessellationEvaluation,
                3 => SpirVExecutionModel::Geometry,
                4 => SpirVExecutionModel::Fragment,
                5 => SpirVExecutionModel::GLCompute,
                6 => SpirVExecutionModel::Kernel,
                n => SpirVExecutionModel::Unknown(n),
            };
            let name_id = code[offset + 2];

            entry_points.push(SpirVEntryPoint {
                execution_model,
                name_id,
            });
        }

        offset += word_count;
    }

    Some(SpirVModuleInfo {
        version_major,
        version_minor,
        generator,
        bound,
        entry_points,
    })
}

/// vkCreateShaderModule — create a shader module from SPIR-V bytecode
pub fn vk_create_shader_module(
    _device: VkHandle,
    create_info: &VkShaderModuleCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();
    let code_size = create_info.code.len() * 4;

    info!(
        "vulkan: vkCreateShaderModule {} bytes ({} words)",
        code_size, create_info.code.len()
    );

    // Parse SPIR-V to extract module info
    let module_info = parse_spirv_header(&create_info.code);

    if let Some(ref info) = module_info {
        debug!(
            "vulkan: SPIR-V v{}.{}, generator={:#x}, bound={}, {} entry point(s)",
            info.version_major, info.version_minor,
            info.generator, info.bound, info.entry_points.len()
        );
        for ep in &info.entry_points {
            debug!("vulkan:   entry point: {:?} (id={})", ep.execution_model, ep.name_id);
        }
    } else {
        warn!("vulkan: could not parse SPIR-V header — module may be invalid");
    }

    let module = VkShaderModule {
        handle,
        spirv_code: create_info.code.clone(),
        code_size,
        module_info,
    };

    SHADER_MODULES.lock().push(module);
    Ok(handle)
}

/// vkDestroyShaderModule — destroy a shader module
pub fn vk_destroy_shader_module(_device: VkHandle, module: VkHandle) -> VkResult {
    debug!("vulkan: vkDestroyShaderModule handle={:?}", module);
    let mut modules = SHADER_MODULES.lock();
    if let Some(pos) = modules.iter().position(|m| m.handle == module) {
        modules.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}
