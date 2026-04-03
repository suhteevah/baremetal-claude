//! Descriptor set management — vkCreateDescriptorSetLayout,
//! vkCreateDescriptorPool, vkAllocateDescriptorSets, vkUpdateDescriptorSets.
//!
//! Descriptors bind shader resources (buffers, images, samplers) to pipeline
//! shader stages.

use alloc::vec::Vec;
use spin::Mutex;
use log::{debug};

use crate::{VkResult, VkHandle, VkStructureType, alloc_handle};
use crate::shader::VkShaderStageFlagBits;

/// Descriptor type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkDescriptorType {
    Sampler = 0,
    CombinedImageSampler = 1,
    SampledImage = 2,
    StorageImage = 3,
    UniformTexelBuffer = 4,
    StorageTexelBuffer = 5,
    UniformBuffer = 6,
    StorageBuffer = 7,
    UniformBufferDynamic = 8,
    StorageBufferDynamic = 9,
    InputAttachment = 10,
}

/// Descriptor set layout binding
#[derive(Debug, Clone)]
pub struct VkDescriptorSetLayoutBinding {
    pub binding: u32,
    pub descriptor_type: VkDescriptorType,
    pub descriptor_count: u32,
    pub stage_flags: u32,
}

/// Descriptor set layout creation info
#[derive(Debug, Clone)]
pub struct VkDescriptorSetLayoutCreateInfo {
    pub s_type: VkStructureType,
    pub bindings: Vec<VkDescriptorSetLayoutBinding>,
}

/// Descriptor pool size entry
#[derive(Debug, Clone)]
pub struct VkDescriptorPoolSize {
    pub descriptor_type: VkDescriptorType,
    pub descriptor_count: u32,
}

/// Descriptor pool creation info
#[derive(Debug, Clone)]
pub struct VkDescriptorPoolCreateInfo {
    pub s_type: VkStructureType,
    pub max_sets: u32,
    pub pool_sizes: Vec<VkDescriptorPoolSize>,
}

/// Descriptor set allocation info
#[derive(Debug, Clone)]
pub struct VkDescriptorSetAllocateInfo {
    pub s_type: VkStructureType,
    pub descriptor_pool: VkHandle,
    pub set_layouts: Vec<VkHandle>,
}

/// Write descriptor set — update a descriptor binding
#[derive(Debug, Clone)]
pub struct VkWriteDescriptorSet {
    pub dst_set: VkHandle,
    pub dst_binding: u32,
    pub dst_array_element: u32,
    pub descriptor_type: VkDescriptorType,
    pub buffer_info: Vec<DescriptorBufferInfo>,
    pub image_info: Vec<DescriptorImageInfo>,
}

/// Buffer descriptor info
#[derive(Debug, Clone)]
pub struct DescriptorBufferInfo {
    pub buffer: VkHandle,
    pub offset: u64,
    pub range: u64,
}

/// Image descriptor info
#[derive(Debug, Clone)]
pub struct DescriptorImageInfo {
    pub sampler: VkHandle,
    pub image_view: VkHandle,
    pub image_layout: crate::image::VkImageLayout,
}

/// A descriptor set layout
pub struct VkDescriptorSetLayout {
    pub handle: VkHandle,
    pub bindings: Vec<VkDescriptorSetLayoutBinding>,
}

/// A descriptor pool
pub struct VkDescriptorPool {
    pub handle: VkHandle,
    pub max_sets: u32,
    pub allocated_sets: u32,
}

/// A descriptor set — an instantiation of a layout with bound resources
pub struct VkDescriptorSet {
    pub handle: VkHandle,
    pub layout: VkHandle,
    pub pool: VkHandle,
    pub bindings: Vec<BoundDescriptor>,
}

/// A bound descriptor resource
#[derive(Debug, Clone)]
pub struct BoundDescriptor {
    pub binding: u32,
    pub descriptor_type: VkDescriptorType,
    pub buffer_info: Option<DescriptorBufferInfo>,
    pub image_info: Option<DescriptorImageInfo>,
}

static SET_LAYOUTS: Mutex<Vec<VkDescriptorSetLayout>> = Mutex::new(Vec::new());
static DESCRIPTOR_POOLS: Mutex<Vec<VkDescriptorPool>> = Mutex::new(Vec::new());
static DESCRIPTOR_SETS: Mutex<Vec<VkDescriptorSet>> = Mutex::new(Vec::new());

/// vkCreateDescriptorSetLayout
pub fn vk_create_descriptor_set_layout(
    _device: VkHandle,
    create_info: &VkDescriptorSetLayoutCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();

    debug!(
        "vulkan: vkCreateDescriptorSetLayout {} bindings",
        create_info.bindings.len()
    );

    let layout = VkDescriptorSetLayout {
        handle,
        bindings: create_info.bindings.clone(),
    };

    SET_LAYOUTS.lock().push(layout);
    Ok(handle)
}

/// vkDestroyDescriptorSetLayout
pub fn vk_destroy_descriptor_set_layout(_device: VkHandle, layout: VkHandle) -> VkResult {
    let mut layouts = SET_LAYOUTS.lock();
    if let Some(pos) = layouts.iter().position(|l| l.handle == layout) {
        layouts.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// vkCreateDescriptorPool
pub fn vk_create_descriptor_pool(
    _device: VkHandle,
    create_info: &VkDescriptorPoolCreateInfo,
) -> Result<VkHandle, VkResult> {
    let handle = alloc_handle();

    debug!(
        "vulkan: vkCreateDescriptorPool max_sets={} pool_sizes={}",
        create_info.max_sets, create_info.pool_sizes.len()
    );

    let pool = VkDescriptorPool {
        handle,
        max_sets: create_info.max_sets,
        allocated_sets: 0,
    };

    DESCRIPTOR_POOLS.lock().push(pool);
    Ok(handle)
}

/// vkDestroyDescriptorPool — destroys pool and all sets allocated from it
pub fn vk_destroy_descriptor_pool(_device: VkHandle, pool: VkHandle) -> VkResult {
    // Free all descriptor sets from this pool
    DESCRIPTOR_SETS.lock().retain(|ds| ds.pool != pool);

    let mut pools = DESCRIPTOR_POOLS.lock();
    if let Some(pos) = pools.iter().position(|p| p.handle == pool) {
        pools.swap_remove(pos);
        VkResult::Success
    } else {
        VkResult::ErrorOutOfHostMemory
    }
}

/// vkAllocateDescriptorSets
pub fn vk_allocate_descriptor_sets(
    _device: VkHandle,
    alloc_info: &VkDescriptorSetAllocateInfo,
) -> Result<Vec<VkHandle>, VkResult> {
    let mut handles = Vec::with_capacity(alloc_info.set_layouts.len());

    // Check pool capacity
    {
        let mut pools = DESCRIPTOR_POOLS.lock();
        let pool = pools.iter_mut()
            .find(|p| p.handle == alloc_info.descriptor_pool)
            .ok_or(VkResult::ErrorFragmentedPool)?;

        let new_count = pool.allocated_sets + alloc_info.set_layouts.len() as u32;
        if new_count > pool.max_sets {
            return Err(VkResult::ErrorFragmentedPool);
        }
        pool.allocated_sets = new_count;
    }

    let mut sets = DESCRIPTOR_SETS.lock();
    for &layout in &alloc_info.set_layouts {
        let handle = alloc_handle();

        let ds = VkDescriptorSet {
            handle,
            layout,
            pool: alloc_info.descriptor_pool,
            bindings: Vec::new(),
        };

        sets.push(ds);
        handles.push(handle);
    }

    debug!(
        "vulkan: vkAllocateDescriptorSets {} sets from pool {:?}",
        handles.len(), alloc_info.descriptor_pool
    );

    Ok(handles)
}

/// vkUpdateDescriptorSets — write resource bindings to descriptor sets
pub fn vk_update_descriptor_sets(
    _device: VkHandle,
    writes: &[VkWriteDescriptorSet],
) -> VkResult {
    let mut sets = DESCRIPTOR_SETS.lock();

    for write in writes {
        let ds = match sets.iter_mut().find(|s| s.handle == write.dst_set) {
            Some(ds) => ds,
            None => {
                log::warn!("vulkan: descriptor set {:?} not found", write.dst_set);
                continue;
            }
        };

        // Update or add binding
        let bound = if !write.buffer_info.is_empty() {
            BoundDescriptor {
                binding: write.dst_binding,
                descriptor_type: write.descriptor_type,
                buffer_info: Some(write.buffer_info[0].clone()),
                image_info: None,
            }
        } else if !write.image_info.is_empty() {
            BoundDescriptor {
                binding: write.dst_binding,
                descriptor_type: write.descriptor_type,
                buffer_info: None,
                image_info: Some(write.image_info[0].clone()),
            }
        } else {
            continue;
        };

        // Replace existing binding or add new one
        if let Some(existing) = ds.bindings.iter_mut().find(|b| b.binding == write.dst_binding) {
            *existing = bound;
        } else {
            ds.bindings.push(bound);
        }

        debug!(
            "vulkan: updated descriptor set {:?} binding {} type={:?}",
            write.dst_set, write.dst_binding, write.descriptor_type
        );
    }

    VkResult::Success
}
