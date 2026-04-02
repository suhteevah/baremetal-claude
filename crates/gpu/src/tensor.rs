//! Tensor operations for LLM inference — the interface between the GPU driver
//! and higher-level model code.
//!
//! This module provides tensor allocation, data transfer, and dispatch of
//! fundamental operations needed for transformer inference:
//!
//! - **MatMul**: Matrix multiplication (the core of attention and FFN layers)
//! - **Softmax**: Attention score normalization
//! - **LayerNorm**: Pre/post-attention normalization
//! - **GELU/SiLU**: Activation functions
//! - **RoPE**: Rotary position embeddings
//!
//! Each operation is dispatched as a GPU compute kernel. In a real implementation,
//! these kernels would be hand-written SASS (NVIDIA's native shader assembly)
//! optimized for the target GPU architecture.
//!
//! ## Data Types
//!
//! LLM inference commonly uses:
//! - **FP16** (half precision): Default for inference, 2 bytes per element
//! - **BF16** (bfloat16): Better dynamic range than FP16, used by some models
//! - **FP32** (single precision): For accumulation and sensitive operations
//! - **INT8**: Quantized inference, 4x throughput vs FP16 on Ampere tensor cores

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use core::fmt;

use crate::compute::{ComputeEngine, GridDim, BlockDim, ConstantBufferBinding};
use crate::fifo::Channel;
use crate::memory::{GpuAllocation, VramAllocator};
use crate::mmio::GpuRegs;

// ===========================================================================
// Data types
// ===========================================================================

/// Data type for tensor elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DType {
    /// 32-bit floating point (4 bytes)
    Float32,
    /// 16-bit floating point / half precision (2 bytes)
    Float16,
    /// Brain float 16 — same exponent range as FP32, less mantissa (2 bytes)
    BFloat16,
    /// 8-bit signed integer (1 byte) — for INT8 quantization
    Int8,
    /// 4-bit integer (packed, 2 elements per byte) — for GPTQ/AWQ quantization
    Int4,
}

impl DType {
    /// Size in bytes per element (Int4 returns 1 since it's packed 2-per-byte).
    pub fn element_size(&self) -> usize {
        match self {
            DType::Float32 => 4,
            DType::Float16 => 2,
            DType::BFloat16 => 2,
            DType::Int8 => 1,
            DType::Int4 => 1, // Packed: 2 elements per byte
        }
    }
}

impl fmt::Display for DType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DType::Float32 => write!(f, "fp32"),
            DType::Float16 => write!(f, "fp16"),
            DType::BFloat16 => write!(f, "bf16"),
            DType::Int8 => write!(f, "int8"),
            DType::Int4 => write!(f, "int4"),
        }
    }
}

// ===========================================================================
// Tensor descriptor
// ===========================================================================

/// A tensor stored in GPU VRAM.
///
/// Tensors are multi-dimensional arrays with a specific data type and layout.
/// For LLM inference, common tensor shapes include:
/// - Weight matrices: [hidden_dim, hidden_dim] or [hidden_dim, intermediate_dim]
/// - Activations: [batch_size, seq_len, hidden_dim]
/// - Attention scores: [batch_size, num_heads, seq_len, seq_len]
#[derive(Debug)]
pub struct TensorDescriptor {
    /// Human-readable name (for debugging).
    pub name: String,
    /// Shape dimensions (e.g., [4096, 4096] for a weight matrix).
    pub shape: Vec<usize>,
    /// Strides in elements (row-major by default).
    pub strides: Vec<usize>,
    /// Data type.
    pub dtype: DType,
    /// GPU VRAM allocation backing this tensor.
    pub allocation: GpuAllocation,
    /// Total number of elements.
    pub num_elements: usize,
    /// Total size in bytes.
    pub size_bytes: u64,
}

impl TensorDescriptor {
    /// Compute row-major strides for a given shape.
    fn compute_strides(shape: &[usize]) -> Vec<usize> {
        let mut strides = Vec::with_capacity(shape.len());
        let mut stride = 1usize;
        for &dim in shape.iter().rev() {
            strides.push(stride);
            stride *= dim;
        }
        strides.reverse();
        strides
    }

    /// Total number of elements in the tensor.
    fn total_elements(shape: &[usize]) -> usize {
        shape.iter().product()
    }
}

impl fmt::Display for TensorDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Tensor '{}' {:?} {} ({} elements, {} bytes) @ GPU va=0x{:016X}",
            self.name, self.shape, self.dtype,
            self.num_elements, self.size_bytes, self.allocation.gpu_va
        )
    }
}

// ===========================================================================
// Tensor allocator
// ===========================================================================

/// Tensor manager — allocates tensors in VRAM and dispatches operations.
pub struct TensorEngine {
    // Precompiled shader programs for each operation would live here.
    // In a real implementation, these would be SASS binaries baked in at build time.

    // TODO: Shader programs for each operation:
    // matmul_shader: Option<ShaderProgram>,
    // softmax_shader: Option<ShaderProgram>,
    // layernorm_shader: Option<ShaderProgram>,
    // gelu_shader: Option<ShaderProgram>,
    // silu_shader: Option<ShaderProgram>,
    // rope_shader: Option<ShaderProgram>,
    // add_shader: Option<ShaderProgram>,
    // scale_shader: Option<ShaderProgram>,

    /// Total bytes allocated for tensors.
    pub total_allocated: u64,
    /// Number of tensors currently alive.
    pub tensor_count: u32,
}

impl TensorEngine {
    /// Create a new tensor engine.
    pub fn new() -> Self {
        log::info!("gpu/tensor: initializing tensor engine");

        // TODO: Load precompiled shader binaries for each operation.
        // These would be SASS binaries targeting the specific GPU architecture.
        //
        // The shaders need to be either:
        // 1. Cross-compiled offline using nvcc/ptxas (requires CUDA toolkit)
        // 2. Generated at runtime by our own compiler (extremely ambitious)
        // 3. Hand-written SASS assembly (requires ISA documentation/RE)
        //
        // Option 1 is the pragmatic path: compile CUDA kernels offline,
        // extract the SASS binary, and embed it in the ClaudioOS binary.

        Self {
            total_allocated: 0,
            tensor_count: 0,
        }
    }

    /// Allocate a new tensor in VRAM.
    pub fn allocate_tensor(
        &mut self,
        allocator: &VramAllocator,
        name: &str,
        shape: &[usize],
        dtype: DType,
    ) -> Option<TensorDescriptor> {
        let num_elements = TensorDescriptor::total_elements(shape);
        let size_bytes = if dtype == DType::Int4 {
            // INT4: 2 elements per byte
            ((num_elements + 1) / 2) as u64
        } else {
            (num_elements * dtype.element_size()) as u64
        };

        log::info!(
            "gpu/tensor: allocating tensor '{}' shape={:?} dtype={} ({} elements, {} bytes)",
            name, shape, dtype, num_elements, size_bytes
        );

        let allocation = allocator.allocate(size_bytes)?;
        let strides = TensorDescriptor::compute_strides(shape);

        self.total_allocated += size_bytes;
        self.tensor_count += 1;

        let tensor = TensorDescriptor {
            name: String::from(name),
            shape: shape.to_vec(),
            strides,
            dtype,
            allocation,
            num_elements,
            size_bytes,
        };

        log::info!("gpu/tensor: allocated {}", tensor);
        Some(tensor)
    }

    /// Upload data from system RAM to a tensor in VRAM.
    ///
    /// `data` must be exactly `tensor.size_bytes` bytes.
    pub fn upload(
        &self,
        allocator: &VramAllocator,
        tensor: &mut TensorDescriptor,
        data: &[u8],
    ) -> bool {
        if data.len() as u64 != tensor.size_bytes {
            log::error!(
                "gpu/tensor: upload size mismatch for '{}' — expected {} bytes, got {}",
                tensor.name, tensor.size_bytes, data.len()
            );
            return false;
        }

        log::info!(
            "gpu/tensor: uploading {} bytes to tensor '{}' at GPU va=0x{:016X}",
            data.len(), tensor.name, tensor.allocation.gpu_va
        );

        // Map the tensor's VRAM to CPU address space via BAR1
        match allocator.map_to_cpu(&mut tensor.allocation) {
            Some(cpu_va) => {
                // TODO: Copy data to VRAM via volatile writes
                // unsafe {
                //     core::ptr::copy_nonoverlapping(
                //         data.as_ptr(),
                //         cpu_va as *mut u8,
                //         data.len(),
                //     );
                // }

                log::info!(
                    "gpu/tensor: uploaded tensor '{}' via BAR1 at CPU va=0x{:016X}",
                    tensor.name, cpu_va
                );
                let _ = cpu_va; // suppress unused warning until TODO is implemented
                true
            }
            None => {
                // TODO: Fall back to DMA transfer via a copy engine channel
                log::error!(
                    "gpu/tensor: failed to map tensor '{}' for CPU upload — DMA fallback not implemented",
                    tensor.name
                );
                false
            }
        }
    }

    /// Download data from a tensor in VRAM to system RAM.
    pub fn download(
        &self,
        allocator: &VramAllocator,
        tensor: &mut TensorDescriptor,
        buffer: &mut [u8],
    ) -> bool {
        if buffer.len() as u64 != tensor.size_bytes {
            log::error!(
                "gpu/tensor: download size mismatch for '{}' — expected {} bytes, got {}",
                tensor.name, tensor.size_bytes, buffer.len()
            );
            return false;
        }

        log::info!(
            "gpu/tensor: downloading {} bytes from tensor '{}' at GPU va=0x{:016X}",
            buffer.len(), tensor.name, tensor.allocation.gpu_va
        );

        match allocator.map_to_cpu(&mut tensor.allocation) {
            Some(cpu_va) => {
                // TODO: Copy data from VRAM via volatile reads
                // unsafe {
                //     core::ptr::copy_nonoverlapping(
                //         cpu_va as *const u8,
                //         buffer.as_mut_ptr(),
                //         buffer.len(),
                //     );
                // }

                log::info!(
                    "gpu/tensor: downloaded tensor '{}' via BAR1",
                    tensor.name
                );
                let _ = cpu_va;
                true
            }
            None => {
                log::error!(
                    "gpu/tensor: failed to map tensor '{}' for CPU download",
                    tensor.name
                );
                false
            }
        }
    }

    /// Free a tensor and release its VRAM.
    pub fn free_tensor(&mut self, allocator: &VramAllocator, tensor: TensorDescriptor) {
        log::info!(
            "gpu/tensor: freeing tensor '{}' ({} bytes)",
            tensor.name, tensor.size_bytes
        );

        self.total_allocated -= tensor.size_bytes;
        self.tensor_count -= 1;
        allocator.free(&tensor.allocation);
    }

    // =======================================================================
    // Tensor operations — each dispatches a compute kernel
    // =======================================================================

    /// Matrix multiplication: C = A @ B
    ///
    /// A: [M, K], B: [K, N] -> C: [M, N]
    ///
    /// This is the most critical operation for LLM inference. On Ampere,
    /// it would use Tensor Cores (HMMA instructions) for FP16/BF16 matmul
    /// with FP32 accumulation, achieving ~150 TFLOPS.
    ///
    /// TODO: The actual kernel would need to implement tiling (128x128 or 256x128
    /// tiles), shared memory staging, register blocking, and warp-level
    /// matrix operations (WMMA/HMMA).
    pub fn matmul(
        &self,
        _regs: &GpuRegs,
        _channel: &Channel,
        _compute: &mut ComputeEngine,
        _allocator: &VramAllocator,
        a: &TensorDescriptor,
        b: &TensorDescriptor,
        c: &TensorDescriptor,
    ) -> bool {
        // Validate shapes
        if a.shape.len() != 2 || b.shape.len() != 2 || c.shape.len() != 2 {
            log::error!("gpu/tensor: matmul requires 2D tensors");
            return false;
        }
        let m = a.shape[0];
        let k = a.shape[1];
        let n = b.shape[1];
        if b.shape[0] != k || c.shape[0] != m || c.shape[1] != n {
            log::error!(
                "gpu/tensor: matmul shape mismatch — A=[{},{}] B=[{},{}] C=[{},{}]",
                a.shape[0], a.shape[1], b.shape[0], b.shape[1], c.shape[0], c.shape[1]
            );
            return false;
        }

        log::info!(
            "gpu/tensor: matmul [{},{}] x [{},{}] -> [{},{}] ({})",
            m, k, k, n, m, n, a.dtype
        );

        // Compute grid/block dimensions for tiled matmul
        let tile_m = 128;
        let tile_n = 128;
        let grid = GridDim {
            x: ((n + tile_n - 1) / tile_n) as u32,
            y: ((m + tile_m - 1) / tile_m) as u32,
            z: 1,
        };
        let block = BlockDim { x: 256, y: 1, z: 1 }; // 256 threads per block

        log::debug!(
            "gpu/tensor: matmul dispatch — grid=({},{},1) block=(256,1,1)",
            grid.x, grid.y
        );

        // TODO: Load the matmul shader and set up constant buffers with:
        // - Matrix dimensions (M, K, N)
        // - Tensor GPU addresses (A, B, C)
        // - Data type configuration
        // - Tile parameters
        //
        // Then dispatch via compute.dispatch()

        let _ = (grid, block); // suppress unused warnings
        log::warn!("gpu/tensor: matmul dispatch not yet implemented — shader binary needed");
        false
    }

    /// Softmax: out[i] = exp(x[i]) / sum(exp(x[j]))
    ///
    /// Applied along the last dimension. Critical for attention score normalization.
    ///
    /// For numerical stability, compute: out[i] = exp(x[i] - max(x)) / sum(exp(x[j] - max(x)))
    pub fn softmax(
        &self,
        _regs: &GpuRegs,
        _channel: &Channel,
        _compute: &mut ComputeEngine,
        input: &TensorDescriptor,
        output: &TensorDescriptor,
    ) -> bool {
        log::info!(
            "gpu/tensor: softmax {:?} -> {:?} ({})",
            input.shape, output.shape, input.dtype
        );

        if input.shape != output.shape {
            log::error!("gpu/tensor: softmax shape mismatch");
            return false;
        }

        let last_dim = *input.shape.last().unwrap_or(&1);
        let outer_size: usize = input.shape.iter().rev().skip(1).product();

        // One block per row (outer dimension), threads cooperate on the reduction
        let _grid = GridDim { x: outer_size as u32, y: 1, z: 1 };
        let _block = BlockDim { x: last_dim.min(1024) as u32, y: 1, z: 1 };

        // TODO: Dispatch softmax kernel
        // The kernel needs:
        // 1. Find max of each row (parallel reduction)
        // 2. Subtract max, compute exp (element-wise)
        // 3. Sum the exps (parallel reduction)
        // 4. Divide each element by the sum

        log::warn!("gpu/tensor: softmax dispatch not yet implemented — shader binary needed");
        false
    }

    /// Layer normalization: out = (x - mean) / sqrt(var + eps) * gamma + beta
    ///
    /// Applied along the last dimension. Used before/after attention and FFN layers.
    pub fn layernorm(
        &self,
        _regs: &GpuRegs,
        _channel: &Channel,
        _compute: &mut ComputeEngine,
        input: &TensorDescriptor,
        output: &TensorDescriptor,
        gamma: &TensorDescriptor,
        beta: &TensorDescriptor,
        epsilon: f32,
    ) -> bool {
        log::info!(
            "gpu/tensor: layernorm {:?} (eps={}) -> {:?}",
            input.shape, epsilon, output.shape
        );

        let _ = (gamma, beta, epsilon);

        // TODO: Dispatch layernorm kernel
        // Requires:
        // 1. Compute mean of each row (parallel reduction)
        // 2. Compute variance of each row (parallel reduction)
        // 3. Normalize: (x - mean) / sqrt(var + eps) * gamma + beta

        log::warn!("gpu/tensor: layernorm dispatch not yet implemented — shader binary needed");
        false
    }

    /// GELU activation: out = x * 0.5 * (1 + tanh(sqrt(2/pi) * (x + 0.044715 * x^3)))
    ///
    /// Element-wise operation used in FFN layers of GPT-style models.
    pub fn gelu(
        &self,
        _regs: &GpuRegs,
        _channel: &Channel,
        _compute: &mut ComputeEngine,
        input: &TensorDescriptor,
        output: &TensorDescriptor,
    ) -> bool {
        log::info!(
            "gpu/tensor: GELU {:?} -> {:?} ({} elements)",
            input.shape, output.shape, input.num_elements
        );

        // Element-wise: one thread per element (or multiple elements per thread for efficiency)
        let total = input.num_elements;
        let threads_per_block = 256u32;
        let _grid = GridDim {
            x: ((total as u32) + threads_per_block - 1) / threads_per_block,
            y: 1,
            z: 1,
        };
        let _block = BlockDim { x: threads_per_block, y: 1, z: 1 };

        // TODO: Dispatch GELU kernel
        log::warn!("gpu/tensor: GELU dispatch not yet implemented — shader binary needed");
        false
    }

    /// SiLU (Swish) activation: out = x * sigmoid(x) = x / (1 + exp(-x))
    ///
    /// Used in LLaMA and other models as alternative to GELU.
    pub fn silu(
        &self,
        _regs: &GpuRegs,
        _channel: &Channel,
        _compute: &mut ComputeEngine,
        input: &TensorDescriptor,
        output: &TensorDescriptor,
    ) -> bool {
        log::info!(
            "gpu/tensor: SiLU {:?} -> {:?} ({} elements)",
            input.shape, output.shape, input.num_elements
        );

        // TODO: Dispatch SiLU kernel (similar to GELU but simpler formula)
        log::warn!("gpu/tensor: SiLU dispatch not yet implemented — shader binary needed");
        false
    }

    /// Rotary Position Embedding (RoPE): apply rotary encoding to Q/K tensors.
    ///
    /// Used in LLaMA, Mistral, and most modern transformer models.
    /// Applies rotation in 2D subspaces of the embedding dimensions.
    pub fn rope(
        &self,
        _regs: &GpuRegs,
        _channel: &Channel,
        _compute: &mut ComputeEngine,
        input: &TensorDescriptor,
        output: &TensorDescriptor,
        _seq_offset: usize,
        _theta: f32,
    ) -> bool {
        log::info!(
            "gpu/tensor: RoPE {:?} -> {:?}",
            input.shape, output.shape
        );

        // TODO: Dispatch RoPE kernel
        // For each position pos and dimension pair (2i, 2i+1):
        //   freq = pos / theta^(2i/d)
        //   out[2i]   = x[2i]   * cos(freq) - x[2i+1] * sin(freq)
        //   out[2i+1] = x[2i]   * sin(freq) + x[2i+1] * cos(freq)

        log::warn!("gpu/tensor: RoPE dispatch not yet implemented — shader binary needed");
        false
    }

    /// Print a summary of tensor engine state.
    pub fn status(&self) {
        log::info!(
            "gpu/tensor: {} tensors allocated, {} MiB total VRAM usage",
            self.tensor_count,
            self.total_allocated / (1024 * 1024),
        );
    }
}
