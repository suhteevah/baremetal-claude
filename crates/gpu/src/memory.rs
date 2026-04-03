//! GPU memory management — VRAM allocation, GPU page tables, DMA mapping.
//!
//! NVIDIA GPUs have their own MMU with multi-level page tables, separate from
//! the x86 CPU page tables. GPU virtual addresses must be translated through
//! these page tables before accessing VRAM or system memory via DMA.
//!
//! ## GPU Page Table Structure (Ampere / GA104)
//!
//! NVIDIA GPUs use a 4-level page table hierarchy:
//! - **PD3** (Page Directory level 3) — top level, 512 entries
//! - **PD2** — second level
//! - **PD1** — third level (small page) or large page (2 MiB) entry
//! - **PD0/PT** — final level, 4 KiB pages
//!
//! Each PTE contains: Valid bit, Aperture (VRAM=0, SysMem coherent=2, SysMem
//! non-coherent=4), and the physical address (4K-aligned in bits [63:12]).
//!
//! ## VRAM Allocation
//!
//! The current allocator is a simple first-fit bump allocator. The first 1 MiB
//! of VRAM is reserved for GPU internal use (page tables, firmware structures).
//! GPU virtual addresses start at 0x0001_0000_0000 to avoid collisions.
//!
//! ## BAR1 CPU Mapping
//!
//! The BAR1 aperture is a PCI BAR that provides a window for the CPU to
//! read/write VRAM. BAR1 has its own page table in VRAM. Currently, we assume
//! an identity mapping where BAR1 maps the first `bar1_size` bytes of VRAM.
//!
//! Reference:
//! - envytools VM: https://envytools.readthedocs.io/en/latest/hw/memory/vm.html
//! - nouveau vm.c: https://github.com/skeggsb/nouveau/blob/master/drm/nouveau/nvkm/subdev/mmu/

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use core::fmt;
use spin::Mutex;

use crate::mmio::{GpuRegs, NV_PFB, NV_PFB_CFG0, NV_PFB_VRAM_SIZE, NV_PFB_NPARTS};

/// GPU memory types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    /// GDDR6 — used by RTX 30xx series (Ampere)
    Gddr6,
    /// GDDR6X — used by RTX 3080/3090 (high-end Ampere)
    Gddr6x,
    /// HBM2/HBM2e — used by datacenter GPUs (A100, H100)
    Hbm2,
    /// Unknown memory type
    Unknown,
}

impl fmt::Display for MemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryType::Gddr6 => write!(f, "GDDR6"),
            MemoryType::Gddr6x => write!(f, "GDDR6X"),
            MemoryType::Hbm2 => write!(f, "HBM2"),
            MemoryType::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Information about the GPU's VRAM.
#[derive(Debug)]
pub struct VramInfo {
    /// Total VRAM size in bytes.
    pub total_bytes: u64,
    /// Memory type (GDDR6, HBM2, etc.).
    pub mem_type: MemoryType,
    /// Memory bus width in bits.
    pub bus_width: u32,
    /// Number of memory partitions.
    pub partitions: u32,
    /// Human-readable description.
    pub description: String,
}

/// A GPU page table entry (PDE or PTE).
///
/// NVIDIA GPU page tables use a multi-level structure:
/// - PD3 (Page Directory level 3) — top level, 512 entries
/// - PD2 — second level
/// - PD1 — third level (small page) or large page entry
/// - PD0/PT — final level, 4 KiB pages
///
/// On Ampere, the page table format supports 4 KiB, 64 KiB, and 2 MiB pages.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuPte {
    /// Raw PTE value. Format depends on level:
    /// - Bits [0]: Valid
    /// - Bits [4:1]: Aperture (VRAM, system memory, peer)
    /// - Bits [63:12]: Physical address (4K aligned)
    pub raw: u64,
}

impl GpuPte {
    /// PTE aperture: VRAM
    pub const APERTURE_VRAM: u64 = 0x0;
    /// PTE aperture: System memory (accessed via PCIe)
    pub const APERTURE_SYS_MEM_COHERENT: u64 = 0x2;
    /// PTE aperture: System memory non-coherent
    pub const APERTURE_SYS_MEM_NONCOHERENT: u64 = 0x4;

    /// Create a valid PTE pointing to a VRAM physical address.
    pub fn vram(phys_addr: u64) -> Self {
        // Valid bit + VRAM aperture + physical address
        let raw = 1 | Self::APERTURE_VRAM | (phys_addr & !0xFFF);
        Self { raw }
    }

    /// Create a valid PTE pointing to a system memory address (for DMA).
    pub fn system_memory(phys_addr: u64) -> Self {
        let raw = 1 | Self::APERTURE_SYS_MEM_COHERENT | (phys_addr & !0xFFF);
        Self { raw }
    }

    /// Create an invalid (unmapped) PTE.
    pub fn invalid() -> Self {
        Self { raw: 0 }
    }

    /// Check if this PTE is valid.
    pub fn is_valid(&self) -> bool {
        (self.raw & 1) != 0
    }
}

/// A block of allocated GPU memory.
#[derive(Debug)]
pub struct GpuAllocation {
    /// GPU virtual address of this allocation.
    pub gpu_va: u64,
    /// Physical address in VRAM.
    pub phys_addr: u64,
    /// Size in bytes (always page-aligned).
    pub size: u64,
    /// Whether this allocation is mapped to CPU via BAR1.
    pub cpu_mapped: bool,
    /// CPU virtual address if mapped (via BAR1 aperture), otherwise 0.
    pub cpu_va: u64,
}

/// Free region in VRAM for the allocator.
#[derive(Debug, Clone)]
struct FreeRegion {
    start: u64,
    size: u64,
}

/// Simple bump allocator for VRAM.
///
/// TODO: Replace with a proper buddy allocator or slab allocator for
/// production use. This bump allocator never actually frees memory.
pub struct VramAllocator {
    /// VRAM info (size, type, etc.)
    info: VramInfo,
    /// Free regions (initially one big region = all of VRAM)
    free_list: Mutex<Vec<FreeRegion>>,
    /// Next GPU virtual address to assign
    next_gpu_va: Mutex<u64>,
    /// BAR1 base address (CPU virtual address for VRAM access)
    bar1_cpu_base: u64,
    /// BAR1 aperture size
    bar1_size: u64,
    /// Current BAR1 mapping offset (how far into VRAM BAR1 is pointing)
    bar1_vram_offset: Mutex<u64>,
}

impl VramAllocator {
    /// Detect VRAM parameters from GPU registers and create an allocator.
    ///
    /// # Safety
    /// `regs` must be a valid GPU register accessor.
    pub unsafe fn detect_and_init(
        regs: &GpuRegs,
        bar1_cpu_base: u64,
        bar1_size: u64,
    ) -> Self {
        log::info!("gpu/memory: detecting VRAM configuration...");

        // Read memory config registers
        let cfg0 = regs.read32(NV_PFB_CFG0);
        log::debug!("gpu/memory: NV_PFB_CFG0 = 0x{:08X}", cfg0);

        let vram_size_reg = regs.read32(NV_PFB_VRAM_SIZE);
        log::debug!("gpu/memory: NV_PFB_VRAM_SIZE = 0x{:08X}", vram_size_reg);

        let nparts = regs.read32(NV_PFB_NPARTS);
        log::debug!("gpu/memory: NV_PFB_NPARTS = 0x{:08X}", nparts);

        // TODO: The actual VRAM size decoding is generation-specific and complex.
        // On Ampere, the size is often reported via the GSP-RM firmware or must
        // be probed by writing patterns to VRAM at increasing addresses.
        //
        // For now, we try to decode from the register and fall back to probing.
        let total_bytes = Self::decode_vram_size(vram_size_reg);
        let partitions = (nparts & 0xFF) as u32;

        // Determine memory type from config register
        // TODO: This decoding is approximate — real nouveau checks multiple regs
        let mem_type = match (cfg0 >> 8) & 0xF {
            0x0..=0x3 => MemoryType::Gddr6,
            0x4..=0x7 => MemoryType::Gddr6x,
            0x8..=0xB => MemoryType::Hbm2,
            _ => MemoryType::Unknown,
        };

        // Bus width: typically partitions * 32 bits (GDDR6) or partitions * 128 bits (HBM)
        let bus_width = match mem_type {
            MemoryType::Hbm2 => partitions * 128,
            _ => partitions * 32,
        };

        let description = format!(
            "{} MiB {} ({}-bit, {} partitions)",
            total_bytes / (1024 * 1024),
            mem_type,
            bus_width,
            partitions,
        );

        let info = VramInfo {
            total_bytes,
            mem_type,
            bus_width,
            partitions,
            description,
        };

        log::info!("gpu/memory: VRAM detected: {}", info.description);

        // Initialize free list: reserve first 1 MiB for GPU internal use,
        // rest is available for allocation.
        let reserved = 1024 * 1024; // 1 MiB reserved for page tables, firmware, etc.
        let free_start = reserved;
        let free_size = total_bytes.saturating_sub(reserved);

        let free_list = Mutex::new(alloc::vec![FreeRegion {
            start: free_start,
            size: free_size,
        }]);

        // GPU virtual address space starts at 0x0001_0000_0000 to avoid conflicts
        // with identity-mapped low memory regions.
        let next_gpu_va = Mutex::new(0x0001_0000_0000u64);

        log::info!(
            "gpu/memory: VRAM allocator initialized — {} MiB available, BAR1 aperture {} MiB",
            free_size / (1024 * 1024),
            bar1_size / (1024 * 1024),
        );

        Self {
            info,
            free_list,
            next_gpu_va,
            bar1_cpu_base,
            bar1_size,
            bar1_vram_offset: Mutex::new(0),
        }
    }

    /// Decode VRAM size from the PFB register.
    /// TODO: This is a simplistic decode — generation-specific logic needed.
    fn decode_vram_size(reg: u32) -> u64 {
        // Common encoding: register holds size in 128 KiB units or as a shift value.
        // Try direct interpretation first.
        if reg == 0 {
            // If register reads 0, fall back to a default.
            // RTX 3070 Ti has 8 GiB GDDR6X.
            log::warn!("gpu/memory: VRAM size register reads 0, assuming 8 GiB (RTX 3070 Ti default)");
            return 8 * 1024 * 1024 * 1024;
        }

        // Some GPUs encode it as raw byte count in upper bits
        let size = (reg as u64) << 20; // Assume MiB units
        if size > 0 && size <= 128 * 1024 * 1024 * 1024 {
            return size;
        }

        // Fallback
        log::warn!(
            "gpu/memory: could not decode VRAM size from register 0x{:08X}, probing needed",
            reg
        );
        0
    }

    /// Get VRAM info.
    pub fn info(&self) -> &VramInfo {
        &self.info
    }

    /// Allocate a block of VRAM.
    ///
    /// Returns a `GpuAllocation` with the GPU virtual address and physical address.
    /// The allocation is page-aligned (4 KiB).
    pub fn allocate(&self, size: u64) -> Option<GpuAllocation> {
        let aligned_size = (size + 0xFFF) & !0xFFF; // Round up to 4 KiB
        log::debug!(
            "gpu/memory: allocating {} bytes ({} bytes aligned) from VRAM",
            size, aligned_size
        );

        let mut free_list = self.free_list.lock();

        // Simple first-fit allocation
        for region in free_list.iter_mut() {
            if region.size >= aligned_size {
                let phys_addr = region.start;
                region.start += aligned_size;
                region.size -= aligned_size;

                // Assign a GPU virtual address
                let mut next_va = self.next_gpu_va.lock();
                let gpu_va = *next_va;
                *next_va += aligned_size;

                log::info!(
                    "gpu/memory: allocated {} KiB at phys=0x{:016X} va=0x{:016X}",
                    aligned_size / 1024, phys_addr, gpu_va
                );

                // TODO: Actually create GPU page table entries mapping gpu_va -> phys_addr.
                // This requires:
                // 1. Allocating PDE/PTE pages from a separate pool
                // 2. Walking/creating the multi-level page table
                // 3. Flushing the GPU TLB (NV_PFIFO or PGRAPH flush)

                return Some(GpuAllocation {
                    gpu_va,
                    phys_addr,
                    size: aligned_size,
                    cpu_mapped: false,
                    cpu_va: 0,
                });
            }
        }

        log::error!(
            "gpu/memory: failed to allocate {} bytes — VRAM exhausted",
            aligned_size
        );
        None
    }

    /// Free a VRAM allocation.
    ///
    /// TODO: This is a no-op in the current bump allocator. Needs proper free list
    /// management with coalescing for production use.
    pub fn free(&self, alloc: &GpuAllocation) {
        log::debug!(
            "gpu/memory: freeing allocation at phys=0x{:016X} ({} KiB) — NOTE: bump allocator, memory not actually reclaimed",
            alloc.phys_addr, alloc.size / 1024
        );

        // TODO: Return the physical region to the free list.
        // TODO: Unmap GPU page table entries.
        // TODO: Flush GPU TLB.
    }

    /// Map a VRAM allocation for CPU access via the BAR1 aperture.
    ///
    /// Returns the CPU virtual address that can be used to read/write the allocation,
    /// or None if the allocation is too large for the BAR1 aperture.
    pub fn map_to_cpu(&self, alloc: &mut GpuAllocation) -> Option<u64> {
        if alloc.cpu_mapped {
            log::debug!("gpu/memory: allocation already CPU-mapped at 0x{:016X}", alloc.cpu_va);
            return Some(alloc.cpu_va);
        }

        if alloc.size > self.bar1_size {
            log::error!(
                "gpu/memory: allocation {} MiB exceeds BAR1 aperture {} MiB, cannot map to CPU",
                alloc.size / (1024 * 1024),
                self.bar1_size / (1024 * 1024),
            );
            return None;
        }

        // TODO: Program the BAR1 VM (BAR1 has its own page table in VRAM) to map
        // the allocation's physical VRAM address into the BAR1 aperture.
        //
        // For now, we assume a simple identity mapping where BAR1 maps the first
        // `bar1_size` bytes of VRAM.
        if alloc.phys_addr + alloc.size <= self.bar1_size {
            let cpu_va = self.bar1_cpu_base + alloc.phys_addr;
            alloc.cpu_mapped = true;
            alloc.cpu_va = cpu_va;

            log::info!(
                "gpu/memory: mapped VRAM phys=0x{:016X} to CPU va=0x{:016X} (BAR1 identity map)",
                alloc.phys_addr, cpu_va
            );

            Some(cpu_va)
        } else {
            // TODO: Remap BAR1 page table to point at the desired VRAM region.
            log::error!(
                "gpu/memory: VRAM phys=0x{:016X} is outside BAR1 identity-mapped region (0..0x{:X})",
                alloc.phys_addr, self.bar1_size
            );
            None
        }
    }

    /// Set up a DMA mapping so the GPU can access system memory.
    ///
    /// Returns a GPU virtual address that the GPU can use to read/write `system_phys_addr`.
    ///
    /// # Safety
    /// `system_phys_addr` must be a valid physical address in system RAM, and the
    /// memory must remain allocated for the lifetime of the mapping.
    pub unsafe fn map_system_memory(
        &self,
        system_phys_addr: u64,
        size: u64,
    ) -> Option<u64> {
        let aligned_size = (size + 0xFFF) & !0xFFF;

        let mut next_va = self.next_gpu_va.lock();
        let gpu_va = *next_va;
        *next_va += aligned_size;

        log::info!(
            "gpu/memory: DMA mapping system phys=0x{:016X} ({} KiB) at GPU va=0x{:016X}",
            system_phys_addr, aligned_size / 1024, gpu_va
        );

        // TODO: Create GPU page table entries with APERTURE_SYS_MEM_COHERENT
        // pointing at system_phys_addr.
        // TODO: Ensure IOMMU/SMMU is configured to allow GPU DMA to this address.
        // TODO: Flush GPU TLB.

        Some(gpu_va)
    }
}
