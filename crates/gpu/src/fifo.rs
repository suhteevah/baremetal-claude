//! Command submission via PFIFO -- GPU command channels and push buffers.
//!
//! NVIDIA GPUs execute commands through a FIFO (First In, First Out) engine.
//! The host CPU constructs commands in a "push buffer" in memory, then notifies
//! the GPU via a doorbell mechanism. The GPU's PFIFO scheduler reads commands
//! from multiple channels according to a "runlist" and dispatches them to the
//! appropriate engines (PGRAPH for compute, CE for copies, etc.).
//!
//! ## GPFIFO (Gather Push FIFO) Command Submission
//!
//! Rather than having the GPU read the push buffer directly, an indirect layer
//! called the GPFIFO is used. The GPFIFO is a ring buffer of 8-byte entries,
//! each pointing to a segment of the push buffer (GPU VA + length in dwords).
//! This allows non-contiguous command submission and reduces doorbell traffic.
//!
//! Submission flow:
//! 1. Write GPU method commands to the push buffer (method header + data dwords)
//! 2. Write a GPFIFO entry pointing to the new push buffer segment
//! 3. Ring the channel's doorbell (MMIO write) with the new GPFIFO put index
//! 4. The GPU's PBDMA (Push Buffer DMA) engine fetches the GPFIFO entry,
//!    then fetches and executes the push buffer commands
//!
//! ## Architecture (Ampere / GA104)
//!
//! ```text
//! CPU                          GPU
//! ┌──────────┐     doorbell    ┌──────────────┐
//! │ Push     │ ──────────────> │ PFIFO        │
//! │ Buffer   │                 │  ├─ Runlist   │
//! │ (VRAM or │   GPFIFO       │  ├─ Channel   │
//! │  sysmem) │ ─ ─ ─ ─ ─ ─ > │  │  Scheduler │
//! └──────────┘                 │  └─> PGRAPH  │
//!                              │      (compute)│
//!                              └──────────────┘
//! ```
//!
//! - **Push buffer**: Ring buffer of GPU method commands (class + method + data).
//! - **GPFIFO**: "Gather Push FIFO" — an indirect buffer of (address, length)
//!   entries pointing into the push buffer. The GPU reads GPFIFO entries to
//!   find push buffer segments to execute.
//! - **Channel**: A hardware context with its own GPFIFO, push buffer, and state.
//! - **Runlist**: A list of channels that the PFIFO scheduler cycles through.
//! - **Doorbell**: MMIO write that tells the GPU "new GPFIFO entries are available."
//!
//! Reference:
//! - envytools FIFO: https://envytools.readthedocs.io/en/latest/hw/fifo/
//! - nouveau chan.c / runl.c

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::mmio::GpuRegs;
use crate::memory::{GpuAllocation, VramAllocator};

// ===========================================================================
// GPU method command encoding
// ===========================================================================

/// Encode a non-incrementing method command header.
///
/// In non-incrementing mode, all `count` data dwords that follow this header
/// are written to the same method address (useful for FIFO data ports).
///
/// Encoding: `[29:28]=0b10 (non-incr) | [28:16]=count | [15:13]=subchannel | [12:0]=method>>2`
pub fn gpu_method_nonincr(subchannel: u32, method: u32, count: u32) -> u32 {
    (0x2 << 28) | ((count & 0x1FFF) << 16) | ((subchannel & 0x7) << 13) | (method >> 2)
}

/// Encode an incrementing method command header.
///
/// In incrementing mode, the first data dword is written to `method`, the second
/// to `method+4`, and so on. This is the standard mode for programming
/// consecutive GPU registers.
///
/// Encoding: `[29:28]=0b01 (incr) | [28:16]=count | [15:13]=subchannel | [12:0]=method>>2`
pub fn gpu_method_incr(subchannel: u32, method: u32, count: u32) -> u32 {
    (0x1 << 28) | ((count & 0x1FFF) << 16) | ((subchannel & 0x7) << 13) | (method >> 2)
}

/// Encode a single method + data (inline, 1 count, incrementing).
pub fn gpu_method_1(subchannel: u32, method: u32) -> u32 {
    gpu_method_incr(subchannel, method, 1)
}

// ===========================================================================
// GPFIFO entry
// ===========================================================================

/// A GPFIFO (Gather Push FIFO) entry — points to a segment of the push buffer.
///
/// Each entry is 8 bytes:
/// - Bits [39:2] of low dword + high dword bits: GPU virtual address of push buffer segment
/// - Bits [30:0] of high dword: length in dwords
/// - Bit [31] of high dword: various flags
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy)]
pub struct GpfifoEntry {
    /// Low 32 bits: address bits [31:0] (4-byte aligned, so bits [1:0] = 0)
    pub lo: u32,
    /// High 32 bits: address bits [39:32] in [7:0], length in [30:8], flags in [31]
    pub hi: u32,
}

impl GpfifoEntry {
    /// Create a GPFIFO entry pointing to a push buffer segment.
    pub fn new(gpu_va: u64, length_dwords: u32) -> Self {
        let lo = gpu_va as u32;
        let hi = ((gpu_va >> 32) as u32 & 0xFF) | ((length_dwords & 0x7FFFFF) << 8);
        Self { lo, hi }
    }

    /// Create a GPFIFO entry with the "no prefetch" flag set.
    pub fn new_no_prefetch(gpu_va: u64, length_dwords: u32) -> Self {
        let lo = gpu_va as u32;
        let hi = ((gpu_va >> 32) as u32 & 0xFF)
            | ((length_dwords & 0x7FFFFF) << 8)
            | (1 << 31); // No prefetch flag
        Self { lo, hi }
    }
}

// ===========================================================================
// Channel
// ===========================================================================

/// A GPU FIFO channel — one submission context with its own push buffer.
pub struct Channel {
    /// Channel ID (assigned during allocation).
    pub id: u32,
    /// GPFIFO allocation in VRAM — the ring buffer of indirect entries.
    pub gpfifo: GpuAllocation,
    /// Push buffer allocation in VRAM — actual GPU commands go here.
    pub push_buffer: GpuAllocation,
    /// Number of GPFIFO entries (power of 2, typically 512 or 1024).
    pub gpfifo_entries: u32,
    /// Current GPFIFO write index (host-side).
    pub gpfifo_put: AtomicU32,
    /// Current push buffer write offset in bytes.
    pub push_offset: AtomicU32,
    /// Push buffer size in bytes.
    pub push_size: u32,
    /// GPU engine this channel is bound to (0 = PGRAPH/compute).
    pub engine_id: u32,
    /// Doorbell register offset for this channel.
    pub doorbell_offset: u32,
}

/// Number of GPFIFO entries per channel (must be power of 2).
pub const DEFAULT_GPFIFO_ENTRIES: u32 = 1024;

/// Push buffer size per channel (256 KiB should be plenty for compute dispatches).
pub const DEFAULT_PUSH_BUFFER_SIZE: u64 = 256 * 1024;

impl Channel {
    /// Allocate a new channel with GPFIFO and push buffer in VRAM.
    pub fn allocate(
        allocator: &VramAllocator,
        channel_id: u32,
        engine_id: u32,
    ) -> Option<Self> {
        log::info!(
            "gpu/fifo: allocating channel {} for engine {}",
            channel_id, engine_id
        );

        // Allocate GPFIFO ring buffer (8 bytes per entry, 1024 entries = 8 KiB)
        let gpfifo_size = (DEFAULT_GPFIFO_ENTRIES as u64) * 8;
        let gpfifo = allocator.allocate(gpfifo_size)?;
        log::info!(
            "gpu/fifo: channel {} GPFIFO at GPU va=0x{:016X} ({} entries)",
            channel_id, gpfifo.gpu_va, DEFAULT_GPFIFO_ENTRIES
        );

        // Allocate push buffer
        let push_buffer = allocator.allocate(DEFAULT_PUSH_BUFFER_SIZE)?;
        log::info!(
            "gpu/fifo: channel {} push buffer at GPU va=0x{:016X} ({} KiB)",
            channel_id, push_buffer.gpu_va, DEFAULT_PUSH_BUFFER_SIZE / 1024
        );

        // TODO: Program the channel descriptor in the GPU's channel table.
        // The channel table is a region of VRAM indexed by channel ID, each entry
        // containing pointers to the GPFIFO, page table root, engine binding, etc.
        //
        // On Ampere, channel descriptors are 512 bytes each and include:
        // - GPFIFO base address (GPU VA)
        // - GPFIFO size (log2)
        // - Instance block address (GPU page table root)
        // - Engine binding (which engine this channel submits to)

        // TODO: Compute the doorbell offset for this channel.
        // On Ampere, the doorbell is at BAR0 + 0x90000 + (channel_id * 8).
        let doorbell_offset = 0x90000 + channel_id * 8;

        Some(Channel {
            id: channel_id,
            gpfifo,
            push_buffer,
            gpfifo_entries: DEFAULT_GPFIFO_ENTRIES,
            gpfifo_put: AtomicU32::new(0),
            push_offset: AtomicU32::new(0),
            push_size: DEFAULT_PUSH_BUFFER_SIZE as u32,
            engine_id,
            doorbell_offset,
        })
    }

    /// Write GPU commands to the push buffer and submit a GPFIFO entry.
    ///
    /// `commands` is a slice of 32-bit GPU method words to write.
    /// Returns the GPU VA of the submitted command segment.
    pub fn submit(&self, regs: &GpuRegs, commands: &[u32]) -> Option<u64> {
        let byte_len = (commands.len() * 4) as u32;
        let dword_len = commands.len() as u32;

        // Check push buffer space
        let current_offset = self.push_offset.load(Ordering::Relaxed);
        if current_offset + byte_len > self.push_size {
            // TODO: Wrap around or wait for GPU to catch up
            log::error!(
                "gpu/fifo: channel {} push buffer full ({}/{} bytes used)",
                self.id, current_offset, self.push_size
            );
            return None;
        }

        let segment_gpu_va = self.push_buffer.gpu_va + current_offset as u64;

        log::debug!(
            "gpu/fifo: channel {} writing {} dwords to push buffer at GPU va=0x{:016X}",
            self.id, dword_len, segment_gpu_va
        );

        // TODO: Write commands to the push buffer via CPU mapping (BAR1).
        // For now, we assume the push buffer is CPU-mapped and we can write directly.
        //
        // In reality:
        // 1. Get CPU VA of push buffer via BAR1 mapping
        // 2. Write commands as volatile u32s
        // 3. Memory barrier to ensure writes are visible to GPU

        self.push_offset.fetch_add(byte_len, Ordering::Relaxed);

        // Write GPFIFO entry
        let gpfifo_idx = self.gpfifo_put.load(Ordering::Relaxed);
        let _gpfifo_entry = GpfifoEntry::new(segment_gpu_va, dword_len);

        // TODO: Write the GPFIFO entry to the GPFIFO ring buffer via CPU mapping.
        // gpfifo_cpu_va[gpfifo_idx] = gpfifo_entry;

        let new_idx = (gpfifo_idx + 1) % self.gpfifo_entries;
        self.gpfifo_put.store(new_idx, Ordering::Release);

        // Ring the doorbell — write the new GPFIFO put index to the doorbell register
        log::debug!(
            "gpu/fifo: channel {} doorbell kick, GPFIFO put = {}",
            self.id, new_idx
        );
        regs.write32(self.doorbell_offset, new_idx);

        Some(segment_gpu_va)
    }

    /// Reset the push buffer write offset (for reuse after GPU has consumed all commands).
    pub fn reset_push_buffer(&self) {
        log::debug!("gpu/fifo: channel {} resetting push buffer", self.id);
        self.push_offset.store(0, Ordering::Relaxed);
    }
}

// ===========================================================================
// Runlist
// ===========================================================================

/// A runlist — the set of channels that the PFIFO scheduler cycles through.
///
/// Each GPU engine has one or more runlists. The scheduler reads the runlist
/// from VRAM and time-slices between the listed channels.
pub struct Runlist {
    /// Runlist ID.
    pub id: u32,
    /// Channels in this runlist, ordered by scheduling priority.
    pub channels: Vec<u32>,
    /// Runlist allocation in VRAM.
    pub allocation: Option<GpuAllocation>,
}

impl Runlist {
    /// Create a new empty runlist.
    pub fn new(id: u32) -> Self {
        log::info!("gpu/fifo: creating runlist {}", id);
        Self {
            id,
            channels: Vec::new(),
            allocation: None,
        }
    }

    /// Add a channel to this runlist.
    pub fn add_channel(&mut self, channel_id: u32) {
        log::info!(
            "gpu/fifo: adding channel {} to runlist {}",
            channel_id, self.id
        );
        self.channels.push(channel_id);
    }

    /// Remove a channel from this runlist.
    pub fn remove_channel(&mut self, channel_id: u32) {
        log::info!(
            "gpu/fifo: removing channel {} from runlist {}",
            channel_id, self.id
        );
        self.channels.retain(|&c| c != channel_id);
    }

    /// Commit the runlist to VRAM and tell the GPU to use it.
    ///
    /// This constructs the runlist data structure in VRAM and writes
    /// the runlist base address + length to the PFIFO registers.
    pub fn commit(&mut self, regs: &GpuRegs, allocator: &VramAllocator) {
        log::info!(
            "gpu/fifo: committing runlist {} with {} channels",
            self.id, self.channels.len()
        );

        // Each runlist entry is 8 bytes on Ampere:
        // Bits [11:0]: channel ID
        // Bits [27:12]: TSG (Time Slice Group) ID
        // Bits [31:28]: type (0 = channel)
        let entry_count = self.channels.len();
        let alloc_size = (entry_count as u64) * 8;

        if alloc_size > 0 {
            if let Some(alloc) = allocator.allocate(alloc_size) {
                // TODO: Write runlist entries to VRAM via CPU mapping:
                // for (i, &channel_id) in self.channels.iter().enumerate() {
                //     let entry = (channel_id & 0xFFF) as u64;  // Type=0 (channel)
                //     write_volatile(alloc_cpu_va + i*8, entry);
                // }

                // TODO: Program PFIFO runlist registers:
                // NV_PFIFO_RUNLIST_BASE + runlist_id * stride = alloc.gpu_va
                // NV_PFIFO_RUNLIST_SIZE + runlist_id * stride = entry_count
                // Trigger runlist update

                let _runlist_base_reg = 0x002270 + self.id * 0x10;

                log::info!(
                    "gpu/fifo: runlist {} committed at GPU va=0x{:016X} ({} entries)",
                    self.id, alloc.gpu_va, entry_count
                );

                self.allocation = Some(alloc);
            } else {
                log::error!("gpu/fifo: failed to allocate VRAM for runlist {}", self.id);
            }
        }
    }
}

// ===========================================================================
// FIFO engine initialization
// ===========================================================================

/// Initialize the PFIFO engine.
///
/// This resets PFIFO, sets up basic configuration, and prepares for channel allocation.
pub fn init_fifo(regs: &GpuRegs) {
    log::info!("gpu/fifo: initializing PFIFO engine...");

    // Read PFIFO status
    let intr = regs.read32(crate::mmio::NV_PFIFO_INTR_0);
    log::debug!("gpu/fifo: PFIFO interrupt status = 0x{:08X}", intr);

    // Clear any pending PFIFO interrupts
    regs.write32(crate::mmio::NV_PFIFO_INTR_0, 0xFFFF_FFFF);
    log::debug!("gpu/fifo: cleared PFIFO interrupts");

    // Enable PFIFO interrupts
    regs.write32(crate::mmio::NV_PFIFO_INTR_EN_0, 0xFFFF_FFFF);
    log::debug!("gpu/fifo: enabled PFIFO interrupts");

    // TODO: On Ampere, PFIFO initialization involves:
    // 1. Program channel table base address in VRAM
    // 2. Set number of channels (up to 512 or 4096 depending on config)
    // 3. Configure runlist engine bindings
    // 4. Set up PBDMA (Push Buffer DMA) engines
    // 5. Enable the FIFO scheduler
    //
    // Much of this is handled by the GSP-RM firmware on Turing+.

    log::info!("gpu/fifo: PFIFO engine initialized (basic config only)");
}
