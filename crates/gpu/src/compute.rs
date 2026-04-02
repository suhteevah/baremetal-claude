//! Compute shader dispatch — loading and executing compute kernels on the GPU.
//!
//! NVIDIA GPUs execute compute workloads through "compute classes" — hardware
//! interfaces with specific method registers. Each GPU generation defines its
//! own compute class (e.g., AMPERE_COMPUTE_A for GA10x).
//!
//! The flow for dispatching a compute kernel:
//! 1. Bind the compute class to a FIFO subchannel
//! 2. Load the shader program (compiled PTX/SASS binary) to VRAM
//! 3. Set constant buffers (kernel arguments, uniforms)
//! 4. Set grid/block dimensions
//! 5. Issue the LAUNCH command
//! 6. Wait for completion via fence/semaphore
//!
//! ## Compute Classes (from envytools)
//!
//! | Generation | Class ID | Name |
//! |-----------|----------|------|
//! | Kepler    | 0xA0C0   | KEPLER_COMPUTE_A |
//! | Maxwell   | 0xB0C0   | MAXWELL_COMPUTE_A |
//! | Pascal    | 0xC0C0   | PASCAL_COMPUTE_A |
//! | Volta     | 0xC3C0   | VOLTA_COMPUTE_A |
//! | Turing    | 0xC5C0   | TURING_COMPUTE_A |
//! | Ampere    | 0xC6C0   | AMPERE_COMPUTE_A |
//! | Ada       | 0xC9C0   | ADA_COMPUTE_A |
//!
//! Reference:
//! - envytools classes: https://envytools.readthedocs.io/en/latest/hw/graph/
//! - nouveau compute.c

use crate::fifo::Channel;
use crate::memory::{GpuAllocation, VramAllocator};
use crate::mmio::GpuRegs;
use crate::pci_config::GpuFamily;

// ===========================================================================
// Compute class IDs
// ===========================================================================

pub const KEPLER_COMPUTE_A: u32 = 0xA0C0;
pub const MAXWELL_COMPUTE_A: u32 = 0xB0C0;
pub const PASCAL_COMPUTE_A: u32 = 0xC0C0;
pub const VOLTA_COMPUTE_A: u32 = 0xC3C0;
pub const TURING_COMPUTE_A: u32 = 0xC5C0;
pub const AMPERE_COMPUTE_A: u32 = 0xC6C0;
pub const ADA_COMPUTE_A: u32 = 0xC9C0;

/// Get the compute class ID for a given GPU family.
pub fn compute_class_for_family(family: GpuFamily) -> u32 {
    match family {
        GpuFamily::Kepler => KEPLER_COMPUTE_A,
        GpuFamily::Maxwell => MAXWELL_COMPUTE_A,
        GpuFamily::Pascal => PASCAL_COMPUTE_A,
        GpuFamily::Volta => VOLTA_COMPUTE_A,
        GpuFamily::Turing => TURING_COMPUTE_A,
        GpuFamily::Ampere => AMPERE_COMPUTE_A,
        GpuFamily::Ada => ADA_COMPUTE_A,
        GpuFamily::Unknown => {
            log::warn!("gpu/compute: unknown GPU family, defaulting to AMPERE_COMPUTE_A");
            AMPERE_COMPUTE_A
        }
    }
}

// ===========================================================================
// Compute class method offsets (Ampere / AMPERE_COMPUTE_A = 0xC6C0)
// These are method offsets within the compute object, submitted via push buffer.
// ===========================================================================

/// Set the shader program address (low 32 bits).
pub const NVC6C0_SET_SHADER_LOCAL_MEMORY_A: u32 = 0x0790;
/// Set the shader program address (high 32 bits).
pub const NVC6C0_SET_SHADER_LOCAL_MEMORY_B: u32 = 0x0794;

/// Set constant buffer base address.
pub const NVC6C0_SET_CONSTANT_BUFFER_BIND: u32 = 0x0AB0;

/// Constant buffer selector — which CB slot (0-17) to configure.
pub const NVC6C0_CB_SELECTOR: u32 = 0x2380;

/// Constant buffer position within the selected CB.
pub const NVC6C0_CB_POS: u32 = 0x2384;

/// Constant buffer data — write constant data here (auto-increments).
pub const NVC6C0_CB_DATA: u32 = 0x2388;

/// Program region address (shader binary in VRAM).
pub const NVC6C0_SET_PROGRAM_REGION_A: u32 = 0x1608;
pub const NVC6C0_SET_PROGRAM_REGION_B: u32 = 0x160C;

/// Shader program offset within the program region.
pub const NVC6C0_SET_SHADER_PROGRAM: u32 = 0x2000;

/// Grid dimensions (number of blocks in X, Y, Z).
pub const NVC6C0_LAUNCH_DIM_X: u32 = 0x2074;
pub const NVC6C0_LAUNCH_DIM_Y: u32 = 0x2078;
pub const NVC6C0_LAUNCH_DIM_Z: u32 = 0x207C;

/// Block dimensions (number of threads per block in X, Y, Z).
pub const NVC6C0_SET_BLOCK_DIM_X: u32 = 0x2040;
pub const NVC6C0_SET_BLOCK_DIM_Y: u32 = 0x2044;
pub const NVC6C0_SET_BLOCK_DIM_Z: u32 = 0x2048;

/// Shared memory size per block.
pub const NVC6C0_SET_SHARED_MEMORY_SIZE: u32 = 0x204C;

/// Launch the compute kernel — writing any value here triggers dispatch.
pub const NVC6C0_LAUNCH: u32 = 0x2080;

/// Semaphore address (for synchronization / fences).
pub const NVC6C0_SET_REPORT_SEMAPHORE_A: u32 = 0x1B00;
pub const NVC6C0_SET_REPORT_SEMAPHORE_B: u32 = 0x1B04;
pub const NVC6C0_SET_REPORT_SEMAPHORE_C: u32 = 0x1B08;
pub const NVC6C0_SET_REPORT_SEMAPHORE_D: u32 = 0x1B0C;

// ===========================================================================
// Shader binary format
// ===========================================================================

/// A compiled shader program loaded in VRAM.
pub struct ShaderProgram {
    /// Allocation in VRAM holding the compiled shader binary (SASS/PTX).
    pub allocation: GpuAllocation,
    /// Size of the shader binary in bytes.
    pub size: u64,
    /// Shader entry point offset within the binary.
    pub entry_offset: u32,
    /// Shared memory size required by this shader (bytes).
    pub shared_mem_size: u32,
    /// Number of registers used per thread.
    pub num_registers: u32,
    /// Number of constant buffer slots used.
    pub num_const_buffers: u32,
}

// ===========================================================================
// Compute dispatch parameters
// ===========================================================================

/// Grid dimensions (number of thread blocks to launch).
#[derive(Debug, Clone, Copy)]
pub struct GridDim {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

/// Block dimensions (threads per block).
#[derive(Debug, Clone, Copy)]
pub struct BlockDim {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

impl BlockDim {
    /// Total number of threads per block.
    pub fn total_threads(&self) -> u32 {
        self.x * self.y * self.z
    }
}

/// A constant buffer binding — maps a GPU VA to a CB slot.
#[derive(Debug, Clone)]
pub struct ConstantBufferBinding {
    /// Constant buffer slot index (0-17).
    pub slot: u32,
    /// GPU virtual address of the constant buffer data.
    pub gpu_va: u64,
    /// Size of the constant buffer in bytes.
    pub size: u32,
}

// ===========================================================================
// Fence / Semaphore for synchronization
// ===========================================================================

/// A GPU fence for synchronization between CPU and GPU.
pub struct GpuFence {
    /// Allocation in VRAM for the semaphore value.
    pub allocation: GpuAllocation,
    /// Expected value when the GPU signals completion.
    pub signal_value: u32,
}

impl GpuFence {
    /// Allocate a new fence.
    pub fn new(allocator: &VramAllocator, signal_value: u32) -> Option<Self> {
        log::debug!("gpu/compute: allocating fence (signal_value={})", signal_value);

        let allocation = allocator.allocate(64)?; // Semaphore needs to be cache-line aligned

        // TODO: Initialize the semaphore memory to 0 via CPU mapping.

        Some(Self {
            allocation,
            signal_value,
        })
    }

    /// Check if the GPU has signaled this fence (non-blocking).
    ///
    /// Returns true if the semaphore value matches the signal value.
    pub fn is_signaled(&self) -> bool {
        // TODO: Read the semaphore value from VRAM via CPU mapping (BAR1).
        // let current = volatile_read(self.allocation.cpu_va as *const u32);
        // current == self.signal_value

        log::trace!(
            "gpu/compute: checking fence at GPU va=0x{:016X} (signal_value={})",
            self.allocation.gpu_va, self.signal_value
        );

        // TODO: Implement actual semaphore read
        false
    }

    /// Spin-wait until the GPU signals this fence.
    ///
    /// WARNING: This will block the CPU. For production, use interrupt-driven waiting.
    pub fn wait(&self, timeout_iters: u64) -> bool {
        log::info!(
            "gpu/compute: waiting for fence (signal_value={}, timeout={})",
            self.signal_value, timeout_iters
        );

        for i in 0..timeout_iters {
            if self.is_signaled() {
                log::info!("gpu/compute: fence signaled after {} iterations", i);
                return true;
            }
            core::hint::spin_loop();
        }

        log::error!(
            "gpu/compute: fence timeout after {} iterations (signal_value={})",
            timeout_iters, self.signal_value
        );
        false
    }
}

// ===========================================================================
// Compute engine
// ===========================================================================

/// The compute engine — manages shader dispatch on the GPU.
pub struct ComputeEngine {
    /// Compute class ID for this GPU.
    pub class_id: u32,
    /// GPU family.
    pub family: GpuFamily,
    /// FIFO subchannel bound to the compute class (typically 0).
    pub subchannel: u32,
    /// Next fence value to use.
    next_fence_value: u32,
}

impl ComputeEngine {
    /// Create a new compute engine for the given GPU family.
    pub fn new(family: GpuFamily) -> Self {
        let class_id = compute_class_for_family(family);
        log::info!(
            "gpu/compute: creating compute engine — family={}, class=0x{:04X}",
            family, class_id
        );
        Self {
            class_id,
            family,
            subchannel: 0,
            next_fence_value: 1,
        }
    }

    /// Bind the compute class to a FIFO subchannel.
    ///
    /// This must be done once per channel before dispatching compute kernels.
    /// It tells the GPU which hardware class to use for commands on this subchannel.
    pub fn bind_to_channel(&self, channel: &Channel, regs: &GpuRegs) {
        log::info!(
            "gpu/compute: binding class 0x{:04X} to channel {} subchannel {}",
            self.class_id, channel.id, self.subchannel
        );

        // Submit a SET_OBJECT command to bind the compute class
        // Method 0x0000 on any subchannel = SET_OBJECT
        let commands = [
            crate::fifo::gpu_method_1(self.subchannel, 0x0000),
            self.class_id,
        ];

        // TODO: Actually submit via channel.submit()
        let _ = channel.submit(regs, &commands);

        log::info!("gpu/compute: compute class bound to channel {}", channel.id);
    }

    /// Load a compiled shader binary into VRAM.
    ///
    /// The shader must be in NVIDIA's SASS (Shader ASSembly) format — the native
    /// GPU instruction set. PTX (Parallel Thread eXecution) would need to be
    /// compiled to SASS first, which requires ptxas (proprietary).
    ///
    /// For bare-metal operation, we would need to either:
    /// 1. Ship pre-compiled SASS binaries for our target GPU
    /// 2. Implement our own PTX-to-SASS compiler (extremely ambitious)
    /// 3. Generate SASS directly (requires undocumented ISA knowledge)
    pub fn load_shader(
        &self,
        allocator: &VramAllocator,
        binary: &[u8],
        entry_offset: u32,
        shared_mem_size: u32,
        num_registers: u32,
    ) -> Option<ShaderProgram> {
        log::info!(
            "gpu/compute: loading shader binary ({} bytes, entry=0x{:X}, shmem={}, regs={})",
            binary.len(), entry_offset, shared_mem_size, num_registers
        );

        let size = binary.len() as u64;
        let mut allocation = allocator.allocate(size)?;

        // TODO: Upload binary to VRAM via CPU mapping (BAR1).
        // let cpu_va = allocator.map_to_cpu(&mut allocation)?;
        // unsafe { core::ptr::copy_nonoverlapping(binary.as_ptr(), cpu_va as *mut u8, binary.len()); }

        // For now, just request the mapping
        let _cpu_va = allocator.map_to_cpu(&mut allocation);

        log::info!(
            "gpu/compute: shader loaded at GPU va=0x{:016X}",
            allocation.gpu_va
        );

        Some(ShaderProgram {
            allocation,
            size,
            entry_offset,
            shared_mem_size,
            num_registers,
            num_const_buffers: 0,
        })
    }

    /// Build push buffer commands to dispatch a compute kernel.
    ///
    /// Returns the command words that should be submitted via a FIFO channel.
    pub fn build_dispatch_commands(
        &self,
        shader: &ShaderProgram,
        grid: GridDim,
        block: BlockDim,
        const_buffers: &[ConstantBufferBinding],
        fence: Option<&GpuFence>,
    ) -> alloc::vec::Vec<u32> {
        let mut cmds = alloc::vec::Vec::with_capacity(64);
        let sc = self.subchannel;

        log::info!(
            "gpu/compute: building dispatch — grid=({},{},{}), block=({},{},{}) = {} threads/block",
            grid.x, grid.y, grid.z,
            block.x, block.y, block.z,
            block.total_threads()
        );

        // --- Set program region (where shader binary lives in VRAM) ---
        cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_SET_PROGRAM_REGION_A));
        cmds.push((shader.allocation.gpu_va >> 32) as u32);
        cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_SET_PROGRAM_REGION_B));
        cmds.push(shader.allocation.gpu_va as u32);

        // --- Set shader program (entry point offset) ---
        cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_SET_SHADER_PROGRAM));
        cmds.push(shader.entry_offset);

        // --- Set shared memory size ---
        cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_SET_SHARED_MEMORY_SIZE));
        cmds.push(shader.shared_mem_size);

        // --- Bind constant buffers ---
        for cb in const_buffers {
            log::debug!(
                "gpu/compute: binding CB slot {} at GPU va=0x{:016X} ({} bytes)",
                cb.slot, cb.gpu_va, cb.size
            );
            cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_CB_SELECTOR));
            cmds.push(cb.slot);
            cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_SET_CONSTANT_BUFFER_BIND));
            // Encoding: valid | size | address
            cmds.push(1 | ((cb.size & 0x1FFFF) << 4)); // valid + size
            // TODO: Also need to set the actual address — this requires additional methods
        }

        // --- Set block dimensions ---
        cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_SET_BLOCK_DIM_X));
        cmds.push(block.x);
        cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_SET_BLOCK_DIM_Y));
        cmds.push(block.y);
        cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_SET_BLOCK_DIM_Z));
        cmds.push(block.z);

        // --- Set grid dimensions ---
        cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_LAUNCH_DIM_X));
        cmds.push(grid.x);
        cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_LAUNCH_DIM_Y));
        cmds.push(grid.y);
        cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_LAUNCH_DIM_Z));
        cmds.push(grid.z);

        // --- Launch! ---
        cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_LAUNCH));
        cmds.push(0); // Any value triggers launch

        log::info!(
            "gpu/compute: dispatch command built — {} dwords, launching {} blocks x {} threads",
            cmds.len(), grid.x * grid.y * grid.z, block.total_threads()
        );

        // --- Optional fence (semaphore release after kernel completion) ---
        if let Some(fence) = fence {
            cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_SET_REPORT_SEMAPHORE_A));
            cmds.push((fence.allocation.gpu_va >> 32) as u32);
            cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_SET_REPORT_SEMAPHORE_B));
            cmds.push(fence.allocation.gpu_va as u32);
            cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_SET_REPORT_SEMAPHORE_C));
            cmds.push(fence.signal_value);
            cmds.push(crate::fifo::gpu_method_1(sc, NVC6C0_SET_REPORT_SEMAPHORE_D));
            cmds.push(0x10004); // Release operation + 4-byte payload

            log::debug!(
                "gpu/compute: fence release added — GPU va=0x{:016X}, value={}",
                fence.allocation.gpu_va, fence.signal_value
            );
        }

        cmds
    }

    /// Dispatch a compute kernel and optionally wait for completion.
    ///
    /// This is the high-level "just run it" API.
    pub fn dispatch(
        &mut self,
        regs: &GpuRegs,
        channel: &Channel,
        shader: &ShaderProgram,
        grid: GridDim,
        block: BlockDim,
        const_buffers: &[ConstantBufferBinding],
        allocator: &VramAllocator,
        wait: bool,
    ) -> bool {
        log::info!("gpu/compute: dispatching compute kernel...");

        let fence = if wait {
            let val = self.next_fence_value;
            self.next_fence_value += 1;
            GpuFence::new(allocator, val)
        } else {
            None
        };

        let cmds = self.build_dispatch_commands(
            shader,
            grid,
            block,
            const_buffers,
            fence.as_ref(),
        );

        match channel.submit(regs, &cmds) {
            Some(va) => {
                log::info!(
                    "gpu/compute: kernel submitted at GPU va=0x{:016X}",
                    va
                );
            }
            None => {
                log::error!("gpu/compute: failed to submit kernel to channel");
                return false;
            }
        }

        if wait {
            if let Some(ref fence) = fence {
                // TODO: Use a real timeout based on NV_PTIMER
                return fence.wait(10_000_000);
            }
        }

        true
    }
}
