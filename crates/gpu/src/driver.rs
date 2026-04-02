//! High-level GPU driver API — initialization, query, and compute dispatch.
//!
//! This module ties together PCI detection, MMIO registers, Falcon firmware,
//! FIFO channels, memory management, compute dispatch, and tensor operations
//! into a single `GpuDevice` API.
//!
//! ## Initialization Sequence
//!
//! ```text
//! 1. PCI scan for NVIDIA GPU (vendor 0x10DE)
//! 2. Enable bus mastering + memory space
//! 3. Map BAR0 (MMIO registers) into kernel virtual address space
//! 4. Map BAR1 (VRAM aperture) into kernel virtual address space
//! 5. Read chip ID, determine GPU family
//! 6. Disable all interrupts
//! 7. Initialize PMU Falcon (power management)
//! 8. Initialize VRAM allocator (detect VRAM size, create free list)
//! 9. Initialize PFIFO engine
//! 10. Allocate a compute channel
//! 11. Bind compute class to channel
//! 12. Ready for compute dispatch
//! ```
//!
//! ## WARNING — GSP-RM Requirement (Turing+)
//!
//! Starting with Turing (RTX 20xx), NVIDIA moved most driver logic into a
//! proprietary firmware blob called GSP-RM that runs on an internal RISC-V
//! core (the GPU System Processor). Without this firmware:
//!
//! - GPU clocks won't be configured (stuck at boot clocks)
//! - VRAM won't be fully initialized
//! - Compute engines won't be usable
//! - The GPU is essentially a brick
//!
//! The GSP-RM firmware is ~30 MiB and NVIDIA distributes it as part of their
//! Linux driver package. The nouveau project is working on loading it via
//! their open-source kernel module (nova), but it's not yet complete.
//!
//! For ClaudioOS, our options are:
//! 1. **Load GSP-RM from a FAT32 partition** (requires extracting from NVIDIA driver)
//! 2. **Target pre-Turing GPUs** (Kepler-Pascal) which don't need GSP-RM
//! 3. **Wait for nova/nouveau** to fully document the GSP-RM loading protocol
//! 4. **Run inference on CPU** via the existing python-lite/rustc-lite path

use alloc::string::String;
use alloc::format;

use crate::pci_config::{self, GpuFamily, GpuPciDevice};
use crate::mmio::GpuRegs;
use crate::memory::VramAllocator;
use crate::falcon::{Falcon, FalconEngine};
use crate::fifo::{self, Channel};
use crate::compute::ComputeEngine;
use crate::tensor::TensorEngine;

/// GPU information returned by `query_info()`.
#[derive(Debug)]
pub struct GpuInfo {
    /// Human-readable GPU name (e.g., "NVIDIA GeForce RTX 3070 Ti").
    pub name: String,
    /// GPU architecture family.
    pub family: GpuFamily,
    /// PCI device ID.
    pub device_id: u16,
    /// Total VRAM in bytes.
    pub vram_bytes: u64,
    /// Number of Graphics Processing Clusters (GPCs).
    pub gpc_count: u32,
    /// Number of Texture Processing Clusters per GPC.
    pub tpc_per_gpc: u32,
    /// Number of Streaming Multiprocessors (estimated: GPCs * TPCs * SMs_per_TPC).
    pub sm_count: u32,
    /// Whether GSP-RM firmware is required (Turing+).
    pub needs_gsp_rm: bool,
    /// Whether the GPU is fully initialized and ready for compute.
    pub compute_ready: bool,
}

impl core::fmt::Display for GpuInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} ({}, {} MiB VRAM, {} SMs, compute_ready={})",
            self.name,
            self.family,
            self.vram_bytes / (1024 * 1024),
            self.sm_count,
            self.compute_ready,
        )
    }
}

/// The main GPU device — owns all GPU resources.
pub struct GpuDevice {
    /// PCI device info (BARs, device ID, etc.).
    pub pci: GpuPciDevice,
    /// MMIO register accessor.
    pub regs: GpuRegs,
    /// VRAM allocator.
    pub vram: VramAllocator,
    /// Compute engine (manages shader dispatch).
    pub compute: ComputeEngine,
    /// Tensor engine (high-level tensor ops).
    pub tensors: TensorEngine,
    /// Compute FIFO channel.
    pub compute_channel: Option<Channel>,
    /// Whether the GPU is fully initialized for compute.
    pub compute_ready: bool,
    /// GPU chip ID (from NV_PMC_BOOT_0).
    pub chip_id: u32,
    /// GPC count.
    pub gpc_count: u32,
    /// TPC per GPC.
    pub tpc_per_gpc: u32,
}

impl GpuDevice {
    /// Initialize the GPU.
    ///
    /// This is the main entry point. It scans PCI, maps BARs, and attempts
    /// full initialization. If the GPU requires GSP-RM firmware (Turing+),
    /// initialization will be partial — PCI and MMIO will work but compute
    /// engines won't be functional.
    ///
    /// # Safety
    /// This function programs hardware directly. The caller must ensure:
    /// - BAR addresses can be safely mapped into the kernel address space
    /// - No other driver is concurrently accessing this GPU
    /// - The kernel's page tables can accommodate the BAR mappings
    pub unsafe fn init() -> Option<Self> {
        log::info!("gpu: beginning GPU initialization sequence...");

        // Step 1: PCI scan
        let pci = pci_config::scan_for_nvidia_gpu()?;

        // Step 2: Map BAR0 (MMIO registers)
        // TODO: In a real kernel, we would set up page table entries to map
        // the BAR0 physical address into kernel virtual address space as
        // uncacheable (UC) memory.
        //
        // For now, we use the BAR0 physical address directly (identity mapping).
        // This works if the kernel has set up identity mapping for MMIO regions.
        let bar0_vaddr = pci.bar0_base as *mut u8;
        log::info!(
            "gpu: mapping BAR0 at phys=0x{:016X} as vaddr={:p} ({} MiB)",
            pci.bar0_base, bar0_vaddr, pci.bar0_size / (1024 * 1024)
        );
        let regs = GpuRegs::new(bar0_vaddr);

        // Step 3: Read chip ID
        let chip_id = regs.read_chip_id();
        log::info!("gpu: chip ID = 0x{:03X}", chip_id);

        // Step 4: Disable interrupts during init
        regs.disable_interrupts();

        // Step 5: Determine if GSP-RM is needed
        let needs_gsp_rm = matches!(
            pci.family,
            GpuFamily::Turing | GpuFamily::Ampere | GpuFamily::Ada
        );

        if needs_gsp_rm {
            log::warn!(
                "gpu: {} (family={}) REQUIRES GSP-RM firmware for full initialization",
                pci.name, pci.family
            );
            log::warn!("gpu: compute engines will NOT be available without GSP-RM");
            log::warn!("gpu: to use GPU compute, place the GSP-RM firmware on the FAT32 boot partition");

            // TODO: Attempt to load GSP-RM firmware from FAT32:
            // 1. Read /nvidia/gsp.bin from FAT32
            // 2. Upload to GSP Falcon via DMA
            // 3. Boot GSP Falcon
            // 4. Communicate with GSP-RM via RPC messages
            // 5. Let GSP-RM handle VRAM init, clocking, engine init
        } else {
            log::info!("gpu: {} (family={}) does not require GSP-RM — direct init possible", pci.name, pci.family);
        }

        // Step 6: Initialize PMU Falcon (power management)
        // On pre-Turing, we can load open-source PMU firmware.
        // On Turing+, the PMU is managed by GSP-RM.
        if !needs_gsp_rm {
            let pmu = Falcon::new(FalconEngine::Pmu);
            if pmu.is_halted(&regs) {
                log::info!("gpu: PMU Falcon is halted — needs firmware");
                // TODO: Load PMU firmware (from FAT32 or embedded in binary).
                // For Kepler-Pascal, nouveau provides open-source PMU firmware
                // that handles clock management and thermal monitoring.
            } else {
                log::info!("gpu: PMU Falcon is already running");
            }
        }

        // Step 7: Initialize VRAM allocator
        let vram = VramAllocator::detect_and_init(
            &regs,
            pci.bar1_base, // CPU base address for BAR1 aperture
            pci.bar1_size,
        );

        // Step 8: Read GPU topology
        let gpc_count = regs.gpc_count();
        let tpc_per_gpc = regs.tpc_per_gpc();
        // On Ampere, each TPC has 2 SMs
        let sms_per_tpc = match pci.family {
            GpuFamily::Ampere | GpuFamily::Ada => 2u32,
            _ => 1,
        };
        let sm_count = gpc_count * tpc_per_gpc * sms_per_tpc;
        log::info!(
            "gpu: topology — {} GPCs, {} TPCs/GPC, {} SMs/TPC = {} SMs total",
            gpc_count, tpc_per_gpc, sms_per_tpc, sm_count
        );

        // Step 9: Initialize PFIFO
        fifo::init_fifo(&regs);

        // Step 10: Create compute engine
        let compute = ComputeEngine::new(pci.family);

        // Step 11: Allocate compute channel
        let compute_channel = Channel::allocate(&vram, 0, 0); // channel 0, engine 0 (PGRAPH)
        let compute_ready = !needs_gsp_rm && compute_channel.is_some();

        if let Some(ref ch) = compute_channel {
            if compute_ready {
                compute.bind_to_channel(ch, &regs);
                log::info!("gpu: compute channel {} ready for dispatch", ch.id);
            }
        }

        // Step 12: Create tensor engine
        let tensors = TensorEngine::new();

        // Step 13: Enable interrupts
        regs.enable_interrupts();

        log::info!("gpu: initialization complete — compute_ready={}", compute_ready);

        Some(GpuDevice {
            pci,
            regs,
            vram,
            compute,
            tensors,
            compute_channel,
            compute_ready,
            chip_id,
            gpc_count,
            tpc_per_gpc,
        })
    }

    /// Query GPU information.
    pub fn query_info(&self) -> GpuInfo {
        let needs_gsp_rm = matches!(
            self.pci.family,
            GpuFamily::Turing | GpuFamily::Ampere | GpuFamily::Ada
        );

        let sms_per_tpc = match self.pci.family {
            GpuFamily::Ampere | GpuFamily::Ada => 2u32,
            _ => 1,
        };

        GpuInfo {
            name: self.pci.name.clone(),
            family: self.pci.family,
            device_id: self.pci.device_id,
            vram_bytes: self.vram.info().total_bytes,
            gpc_count: self.gpc_count,
            tpc_per_gpc: self.tpc_per_gpc,
            sm_count: self.gpc_count * self.tpc_per_gpc * sms_per_tpc,
            needs_gsp_rm,
            compute_ready: self.compute_ready,
        }
    }

    /// Print a detailed GPU status report to the log.
    pub fn status(&self) {
        let info = self.query_info();
        log::info!("=== GPU Status ===");
        log::info!("  Name: {}", info.name);
        log::info!("  Family: {}", info.family);
        log::info!("  Chip ID: 0x{:03X}", self.chip_id);
        log::info!("  Device ID: 0x{:04X}", info.device_id);
        log::info!("  VRAM: {} MiB", info.vram_bytes / (1024 * 1024));
        log::info!("  Topology: {} GPCs, {} TPCs/GPC, {} SMs", info.gpc_count, info.tpc_per_gpc, info.sm_count);
        log::info!("  Needs GSP-RM: {}", info.needs_gsp_rm);
        log::info!("  Compute Ready: {}", info.compute_ready);
        log::info!("  BAR0: 0x{:016X} ({} MiB)", self.pci.bar0_base, self.pci.bar0_size / (1024 * 1024));
        log::info!("  BAR1: 0x{:016X} ({} MiB)", self.pci.bar1_base, self.pci.bar1_size / (1024 * 1024));

        // Tensor engine status
        self.tensors.status();

        // Read GPU timer
        let timer_ns = self.regs.read_timer_ns();
        log::info!("  GPU Timer: {} ms", timer_ns / 1_000_000);

        // Check for pending interrupts
        let intr = self.regs.read_interrupts();
        if intr != 0 {
            log::warn!("  Pending Interrupts: 0x{:08X}", intr);
        }

        log::info!("==================");
    }

    /// Handle a GPU interrupt.
    ///
    /// Called from the kernel's interrupt handler when the GPU's PCI interrupt fires.
    pub fn handle_interrupt(&self) {
        let intr = self.regs.read_interrupts();
        if intr == 0 {
            return; // Spurious interrupt
        }

        log::debug!("gpu: interrupt fired, status=0x{:08X}", intr);

        if intr & crate::mmio::NV_PMC_INTR_PFIFO != 0 {
            log::debug!("gpu: PFIFO interrupt");
            // TODO: Read and handle PFIFO-specific interrupt status
            // Clear PFIFO interrupt
            self.regs.write32(crate::mmio::NV_PFIFO_INTR_0, 0xFFFF_FFFF);
        }

        if intr & crate::mmio::NV_PMC_INTR_PGRAPH != 0 {
            log::debug!("gpu: PGRAPH interrupt");
            // TODO: Read and handle PGRAPH-specific interrupt status
            // This fires on compute kernel completion, errors, etc.
            self.regs.write32(crate::mmio::NV_PGRAPH_INTR, 0xFFFF_FFFF);
        }

        if intr & crate::mmio::NV_PMC_INTR_PTIMER != 0 {
            log::trace!("gpu: PTIMER interrupt");
            self.regs.write32(crate::mmio::NV_PTIMER_INTR_0, 0xFFFF_FFFF);
        }
    }
}
