//! Falcon microcontroller interface.
//!
//! Falcon (FAst Logic CONtroller) is NVIDIA's proprietary embedded processor
//! found in multiple instances within each GPU. Different Falcon units handle:
//!
//! - **PMU** (Power Management Unit) at 0x10a000 — clocks, thermals, voltage
//! - **SEC2** (Security Engine 2) at 0x840000 — secure boot, firmware auth
//! - **NVDEC** — video decode
//! - **NVENC** — video encode
//! - **CE** (Copy Engine) — memory copy DMA
//! - **GSP** (GPU System Processor, Turing+) — runs the GSP-RM firmware blob
//!
//! Each Falcon has its own IMEM (instruction memory), DMEM (data memory),
//! interrupt system, and mailbox registers for communication with the host.
//!
//! On Turing and later, NVIDIA requires signed firmware (GSP-RM) to initialize
//! the GPU. Without it, compute engines cannot be used. This is the single
//! biggest obstacle to bare-metal GPU compute without proprietary blobs.
//!
//! Reference:
//! - envytools Falcon: https://envytools.readthedocs.io/en/latest/hw/falcon/
//! - nouveau falcon.h: https://github.com/skeggsb/nouveau/blob/master/drm/nouveau/include/nvkm/falcon.h

use crate::mmio::GpuRegs;

// ===========================================================================
// Falcon register offsets (relative to each Falcon's base address)
// ===========================================================================

/// Interrupt status (read). Each bit = one interrupt source.
pub const FALCON_IRQSRD: u32 = 0x020;

/// Interrupt set — write 1 to bits to manually trigger interrupts (testing).
pub const FALCON_IRQSSET: u32 = 0x024;

/// Interrupt clear — write 1 to bits to acknowledge/clear interrupts.
pub const FALCON_IRQSCLR: u32 = 0x028;

/// Interrupt mask set — enable interrupts for specific sources.
pub const FALCON_IRQMSET: u32 = 0x024;

/// Interrupt mask clear — disable interrupts for specific sources.
pub const FALCON_IRQMCLR: u32 = 0x028;

/// Interrupt destination — routes interrupts to Falcon or host.
pub const FALCON_IRQDEST: u32 = 0x02C;

/// Mailbox 0 — bidirectional communication between host and Falcon firmware.
pub const FALCON_MAILBOX0: u32 = 0x040;

/// Mailbox 1 — second mailbox register.
pub const FALCON_MAILBOX1: u32 = 0x044;

/// Falcon OS register — firmware writes a magic value here when ready.
pub const FALCON_OS: u32 = 0x080;

/// IMEM (Instruction Memory) configuration.
pub const FALCON_IMEMC: u32 = 0x180;

/// IMEM data port — write firmware instructions here.
pub const FALCON_IMEMD: u32 = 0x184;

/// IMEM tag — authentication tag for secure Falcon.
pub const FALCON_IMEMT: u32 = 0x188;

/// DMEM (Data Memory) configuration.
pub const FALCON_DMEMC: u32 = 0x1C0;

/// DMEM data port — write firmware data here.
pub const FALCON_DMEMD: u32 = 0x1C4;

/// CPU control register. Bit 1: start execution. Bit 2: halt/stop.
pub const FALCON_CPUCTL: u32 = 0x100;

/// Boot vector — entry point address for Falcon code.
pub const FALCON_BOOTVEC: u32 = 0x104;

/// Falcon hardware/firmware status.
pub const FALCON_HWCFG: u32 = 0x108;

/// DMA control — for DMA transfers between system memory and Falcon IMEM/DMEM.
pub const FALCON_DMACTL: u32 = 0x10C;

/// DMA transfer trigger.
pub const FALCON_DMATRFBASE: u32 = 0x110;

/// DMA transfer mode.
pub const FALCON_DMATRFMOFFS: u32 = 0x114;

/// DMA transfer command — initiates a DMA operation.
pub const FALCON_DMATRFCMD: u32 = 0x118;

/// DMA transfer status — poll until transfer completes.
pub const FALCON_DMATRFFBOFFS: u32 = 0x11C;

// CPUCTL bits
pub const FALCON_CPUCTL_START: u32 = 1 << 1;
pub const FALCON_CPUCTL_HALT: u32 = 1 << 2;

/// Which Falcon engine we're talking to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FalconEngine {
    /// Power Management Unit at NV_PMU (0x10a000)
    Pmu,
    /// Security Engine 2 at 0x840000 (Turing+)
    Sec2,
    /// GPU System Processor — runs GSP-RM firmware (Turing+)
    Gsp,
}

impl FalconEngine {
    /// Return the BAR0 base offset for this Falcon engine.
    pub fn base_offset(&self) -> u32 {
        match self {
            FalconEngine::Pmu => 0x10a000,
            FalconEngine::Sec2 => 0x840000,
            FalconEngine::Gsp => 0x110000,
        }
    }
}

/// Interface to a single Falcon microcontroller instance.
pub struct Falcon {
    /// Which engine this Falcon belongs to.
    engine: FalconEngine,
    /// BAR0 base offset for this Falcon's registers.
    base: u32,
}

impl Falcon {
    /// Create a new Falcon interface for the given engine.
    pub fn new(engine: FalconEngine) -> Self {
        let base = engine.base_offset();
        log::info!(
            "gpu/falcon: creating {:?} interface at BAR0 + 0x{:06X}",
            engine, base
        );
        Self { engine, base }
    }

    /// Read a Falcon register.
    fn read(&self, regs: &GpuRegs, offset: u32) -> u32 {
        regs.read32(self.base + offset)
    }

    /// Write a Falcon register.
    fn write(&self, regs: &GpuRegs, offset: u32, value: u32) {
        regs.write32(self.base + offset, value);
    }

    /// Check if this Falcon is halted (not running).
    pub fn is_halted(&self, regs: &GpuRegs) -> bool {
        let cpuctl = self.read(regs, FALCON_CPUCTL);
        (cpuctl & FALCON_CPUCTL_HALT) != 0
    }

    /// Halt this Falcon (stop execution).
    pub fn halt(&self, regs: &GpuRegs) {
        log::info!("gpu/falcon: halting {:?}", self.engine);
        self.write(regs, FALCON_CPUCTL, FALCON_CPUCTL_HALT);

        // Read back to confirm
        let cpuctl = self.read(regs, FALCON_CPUCTL);
        log::debug!(
            "gpu/falcon: {:?} CPUCTL after halt = 0x{:08X}",
            self.engine, cpuctl
        );
    }

    /// Upload firmware to IMEM (Instruction Memory).
    ///
    /// Firmware is loaded as 32-bit words. On secure Falcon (Turing+), the firmware
    /// must be signed by NVIDIA — unsigned code will not execute.
    ///
    /// # Arguments
    /// - `regs`: GPU register accessor
    /// - `firmware`: firmware binary (must be 4-byte aligned)
    /// - `imem_offset`: offset within IMEM to load at (usually 0)
    pub fn upload_imem(&self, regs: &GpuRegs, firmware: &[u8], imem_offset: u32) {
        log::info!(
            "gpu/falcon: uploading {} bytes to {:?} IMEM at offset 0x{:04X}",
            firmware.len(), self.engine, imem_offset
        );

        if firmware.len() % 4 != 0 {
            log::error!("gpu/falcon: firmware size {} is not 4-byte aligned", firmware.len());
            return;
        }

        // Configure IMEM write: auto-increment, offset
        // Bits [25]: auto-increment enable
        // Bits [15:2]: block offset (in 256-byte blocks)
        let imemc_val = (1 << 25) | (imem_offset & 0xFFFC);
        self.write(regs, FALCON_IMEMC, imemc_val);

        // Write firmware words
        let word_count = firmware.len() / 4;
        for i in 0..word_count {
            let offset = i * 4;
            let word = u32::from_le_bytes([
                firmware[offset],
                firmware[offset + 1],
                firmware[offset + 2],
                firmware[offset + 3],
            ]);
            self.write(regs, FALCON_IMEMD, word);
        }

        log::info!(
            "gpu/falcon: uploaded {} words to {:?} IMEM",
            word_count, self.engine
        );
    }

    /// Upload data to DMEM (Data Memory).
    ///
    /// # Arguments
    /// - `regs`: GPU register accessor
    /// - `data`: data to upload (must be 4-byte aligned)
    /// - `dmem_offset`: offset within DMEM to load at (usually 0)
    pub fn upload_dmem(&self, regs: &GpuRegs, data: &[u8], dmem_offset: u32) {
        log::info!(
            "gpu/falcon: uploading {} bytes to {:?} DMEM at offset 0x{:04X}",
            data.len(), self.engine, dmem_offset
        );

        if data.len() % 4 != 0 {
            log::error!("gpu/falcon: data size {} is not 4-byte aligned", data.len());
            return;
        }

        // Configure DMEM write: auto-increment, offset
        let dmemc_val = (1 << 25) | (dmem_offset & 0xFFFC);
        self.write(regs, FALCON_DMEMC, dmemc_val);

        let word_count = data.len() / 4;
        for i in 0..word_count {
            let offset = i * 4;
            let word = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            self.write(regs, FALCON_DMEMD, word);
        }

        log::info!(
            "gpu/falcon: uploaded {} words to {:?} DMEM",
            word_count, self.engine
        );
    }

    /// Boot the Falcon from the given entry point.
    ///
    /// This sets the boot vector and starts execution. After booting, firmware
    /// should write a magic value to FALCON_OS to signal readiness, and
    /// communicate via MAILBOX0/MAILBOX1.
    pub fn boot(&self, regs: &GpuRegs, entry_point: u32) {
        log::info!(
            "gpu/falcon: booting {:?} at entry point 0x{:08X}",
            self.engine, entry_point
        );

        // Set boot vector
        self.write(regs, FALCON_BOOTVEC, entry_point);

        // Start execution
        self.write(regs, FALCON_CPUCTL, FALCON_CPUCTL_START);

        log::info!("gpu/falcon: {:?} boot command issued", self.engine);
    }

    /// Wait for the Falcon to signal readiness by writing to FALCON_OS.
    ///
    /// Returns the value written to FALCON_OS by the firmware, or None on timeout.
    pub fn wait_ready(&self, regs: &GpuRegs, timeout_iters: u32) -> Option<u32> {
        log::info!(
            "gpu/falcon: waiting for {:?} to become ready (timeout={} iters)...",
            self.engine, timeout_iters
        );

        for i in 0..timeout_iters {
            let os_val = self.read(regs, FALCON_OS);
            if os_val != 0 {
                log::info!(
                    "gpu/falcon: {:?} ready after {} iterations, FALCON_OS = 0x{:08X}",
                    self.engine, i, os_val
                );
                return Some(os_val);
            }

            // TODO: Replace with actual timer-based delay (use NV_PTIMER or PIT)
            // For now, the loop itself provides some delay.
            core::hint::spin_loop();
        }

        log::error!(
            "gpu/falcon: {:?} did not become ready within {} iterations",
            self.engine, timeout_iters
        );
        None
    }

    /// Read mailbox 0 (host <- Falcon communication).
    pub fn read_mailbox0(&self, regs: &GpuRegs) -> u32 {
        let val = self.read(regs, FALCON_MAILBOX0);
        log::trace!("gpu/falcon: {:?} MAILBOX0 = 0x{:08X}", self.engine, val);
        val
    }

    /// Write mailbox 0 (host -> Falcon communication).
    pub fn write_mailbox0(&self, regs: &GpuRegs, value: u32) {
        log::trace!("gpu/falcon: {:?} MAILBOX0 <- 0x{:08X}", self.engine, value);
        self.write(regs, FALCON_MAILBOX0, value);
    }

    /// Read mailbox 1.
    pub fn read_mailbox1(&self, regs: &GpuRegs) -> u32 {
        let val = self.read(regs, FALCON_MAILBOX1);
        log::trace!("gpu/falcon: {:?} MAILBOX1 = 0x{:08X}", self.engine, val);
        val
    }

    /// Write mailbox 1.
    pub fn write_mailbox1(&self, regs: &GpuRegs, value: u32) {
        log::trace!("gpu/falcon: {:?} MAILBOX1 <- 0x{:08X}", self.engine, value);
        self.write(regs, FALCON_MAILBOX1, value);
    }

    /// Clear all pending interrupts on this Falcon.
    pub fn clear_interrupts(&self, regs: &GpuRegs) {
        let pending = self.read(regs, FALCON_IRQSRD);
        if pending != 0 {
            log::debug!(
                "gpu/falcon: {:?} clearing pending interrupts: 0x{:08X}",
                self.engine, pending
            );
            self.write(regs, FALCON_IRQSCLR, pending);
        }
    }

    /// Perform a full Falcon initialization sequence.
    ///
    /// This is the high-level boot flow:
    /// 1. Halt the Falcon
    /// 2. Upload firmware to IMEM
    /// 3. Upload initial data to DMEM
    /// 4. Set boot vector and start
    /// 5. Wait for firmware to signal readiness
    ///
    /// Returns true if the Falcon booted successfully.
    ///
    /// # Warning
    /// On Turing+ GPUs, the PMU and SEC2 Falcon require NVIDIA-signed firmware.
    /// Loading unsigned firmware will fail silently or trigger a security fault.
    /// The GSP Falcon requires the proprietary GSP-RM firmware blob (~30 MiB).
    pub fn init_with_firmware(
        &self,
        regs: &GpuRegs,
        imem_firmware: &[u8],
        dmem_data: &[u8],
    ) -> bool {
        log::info!(
            "gpu/falcon: beginning {:?} initialization (imem={} bytes, dmem={} bytes)",
            self.engine, imem_firmware.len(), dmem_data.len()
        );

        // Step 1: Halt
        self.halt(regs);
        self.clear_interrupts(regs);

        // Step 2: Upload IMEM
        self.upload_imem(regs, imem_firmware, 0);

        // Step 3: Upload DMEM
        if !dmem_data.is_empty() {
            self.upload_dmem(regs, dmem_data, 0);
        }

        // Step 4: Boot
        self.boot(regs, 0);

        // Step 5: Wait for readiness
        match self.wait_ready(regs, 1_000_000) {
            Some(os_val) => {
                log::info!(
                    "gpu/falcon: {:?} initialized successfully, OS=0x{:08X}",
                    self.engine, os_val
                );
                true
            }
            None => {
                log::error!(
                    "gpu/falcon: {:?} failed to initialize — firmware did not signal readiness",
                    self.engine
                );
                false
            }
        }
    }
}
