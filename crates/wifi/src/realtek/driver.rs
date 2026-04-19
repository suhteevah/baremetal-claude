//! Realtek RTL8852BE driver — incremental bring-up.
//!
//! This module is NOT a finished driver. It's the foundation we're bringing
//! up one register at a time, because our only debug channel on real hardware
//! is a photograph of the framebuffer. Every step here must be safe (no
//! writes to hardware until we're sure MMIO decoding works), produce a
//! readable probe result for the fallback terminal, and be easy to extend.
//!
//! Current scope: map BAR2 (rtw89 PCIe MMIO base) at the HHDM-translated
//! virtual address, then read a short list of well-known registers to confirm
//! the BAR decodes and the device is alive. No writes. No IRQ handling yet.
//!
//! References:
//! - Linux `rtw89/pci.c`: PCIe transport setup
//! - Linux `rtw89/reg.h`: register offsets used below
//! - Linux `rtw89/rtw8852b.c`: chip-specific probe

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ptr::read_volatile;

/// Starting byte offset for the register-dump probe. Bump this between flash
/// cycles to walk the BAR register file in QR-sized chunks — the compact
/// output format skips sentinel values (0, 0xFFFFFFFF, 0xDEADBEEF,
/// 0xEAEAEAEA) so each chunk fits a reasonably sized QR code.
pub const PROBE_DUMP_START: u32 = 0x0100;

/// Number of 32-bit words to dump starting at PROBE_DUMP_START.
pub const PROBE_DUMP_WORDS: usize = 256;

/// One probe reading, for dumping to the fallback terminal.
#[derive(Debug, Clone)]
pub struct ProbeReading {
    pub name: &'static str,
    pub offset: u32,
    pub value: u32,
}

/// Result of probing the chip's register file. Does not imply the chip is
/// functional — merely that MMIO decoded and we got bytes back.
#[derive(Debug, Clone)]
pub struct RealtekProbe {
    pub readings: Vec<ProbeReading>,
    /// True if any register returned a value distinct from both 0x00000000
    /// and 0xFFFFFFFF (bus-not-decoded / device-absent sentinels).
    pub mmio_decoded: bool,
    /// One-line human-readable summary.
    pub summary: String,
}

/// Backend handle. Owns the virtual pointer to the MMIO register file.
pub struct RealtekController {
    mmio_virt: *mut u8,
    /// Physical base (for logging).
    mmio_phys: u64,
    /// Stored probe reading from init.
    initial_probe: RealtekProbe,
}

// The raw pointer is only used with volatile read/write. Safe to send across
// task boundaries because we serialize access through &mut self.
unsafe impl Send for RealtekController {}
unsafe impl Sync for RealtekController {}

impl RealtekController {
    /// Map BAR2 into the kernel via HHDM and perform read-only probe.
    ///
    /// # Safety
    ///
    /// - `mmio_phys` must be the physical base address of a decoded BAR owned
    ///   by the Realtek device. Passing any other address invites a hard
    ///   fault or silent corruption.
    /// - `phys_mem_offset` must be the Limine HHDM offset.
    /// - PCI Command register bits 1 (Memory Space Enable) and 2 (Bus Master)
    ///   must already be set by the caller.
    pub unsafe fn init_readonly(
        mmio_phys: u64,
        phys_mem_offset: u64,
    ) -> Result<Self, &'static str> {
        if mmio_phys == 0 {
            return Err("realtek: BAR2 is zero — is the device decoded?");
        }
        let virt = (mmio_phys + phys_mem_offset) as *mut u8;

        let mut readings = Vec::new();
        let mut any_real = false;
        let mut last_real_offset: i32 = -1;
        let mut total_read = 0usize;
        for i in 0..PROBE_DUMP_WORDS {
            let offset = PROBE_DUMP_START + (i as u32) * 4;
            let ptr = unsafe { virt.add(offset as usize) as *const u32 };
            let value = unsafe { read_volatile(ptr) };
            total_read += 1;
            let is_sentinel = value == 0x0000_0000
                || value == 0xFFFF_FFFF
                || value == 0xDEAD_BEEF
                || value == 0xEAEA_EAEA;
            if !is_sentinel {
                any_real = true;
                last_real_offset = offset as i32;
                readings.push(ProbeReading {
                    name: "r",
                    offset,
                    value,
                });
            }
        }
        let _ = total_read;

        let summary = if any_real {
            alloc::format!(
                "MMIO decoded; {} live regs in 0x{:03x}..0x{:03x}, last live @0x{:03x}",
                readings.len(),
                PROBE_DUMP_START,
                PROBE_DUMP_START + (PROBE_DUMP_WORDS as u32) * 4 - 4,
                last_real_offset.max(0),
            )
        } else {
            alloc::format!(
                "no live regs in 0x{:03x}..0x{:03x} — BAR end or chip powered down",
                PROBE_DUMP_START,
                PROBE_DUMP_START + (PROBE_DUMP_WORDS as u32) * 4 - 4,
            )
        };

        Ok(Self {
            mmio_virt: virt,
            mmio_phys,
            initial_probe: RealtekProbe {
                readings,
                mmio_decoded: any_real,
                summary,
            },
        })
    }

    pub fn mmio_phys(&self) -> u64 {
        self.mmio_phys
    }

    pub fn initial_probe(&self) -> &RealtekProbe {
        &self.initial_probe
    }
}

/// Legacy placeholder kept so the top-level `WifiController::Realtek(Stub)`
/// enum variant keeps compiling for vendors we've not yet brought up.
pub struct Stub;
