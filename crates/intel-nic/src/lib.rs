//! Intel Ethernet NIC driver family for ClaudioOS.
//!
//! Supports:
//! - **e1000** (82540EM) — QEMU's default emulated NIC (`-device e1000`)
//! - **e1000e** / **I219-V** — Consumer Intel GbE on i9-11900K and similar
//! - **igc** / **I225-V** / **I226-V** — Intel 2.5GbE on newer consumer boards
//!
//! All three families share the same basic register layout (MMIO via PCI BAR0),
//! legacy descriptor format, and EEPROM/MAC address scheme. Differences are
//! isolated to PHY initialization and a handful of register offsets.
//!
//! # Integration
//!
//! The [`E1000`] struct provides `transmit()`, `receive()`, and `mac_address()`
//! methods matching the `NicDriver` trait defined in `claudio-net`. The kernel
//! wires this into smoltcp the same way it does the VirtIO-net driver.
//!
//! # Usage
//!
//! ```ignore
//! let nic = unsafe { E1000::init(bar0_mmio_base, irq_line, phys_mem_offset, variant) };
//! // nic.transmit(frame), nic.receive(&mut buf), nic.mac_address()
//! ```

#![no_std]

extern crate alloc;

pub mod regs;
pub mod descriptors;
pub mod e1000;
pub mod i225;
pub mod phy;

pub use e1000::E1000;
pub use i225::I225Quirks;
pub use phy::PhyManager;

/// NIC variant for selecting family-specific initialization paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NicVariant {
    /// Classic e1000 (82540EM, 82545EM) — QEMU emulated NIC.
    E1000,
    /// e1000e / I219-V — Intel GbE on consumer boards (Comet Lake, Rocket Lake).
    I219V,
    /// igc / I225-V — Intel 2.5GbE on newer consumer boards.
    I225V,
    /// igc / I226-V — Intel 2.5GbE (Alder Lake+).
    I226V,
}

impl NicVariant {
    /// Identify variant from PCI vendor/device ID.
    ///
    /// Returns `None` if the device is not a supported Intel NIC.
    pub fn from_pci_ids(vendor: u16, device: u16) -> Option<Self> {
        if vendor != 0x8086 {
            return None;
        }
        match device {
            // e1000 family (QEMU)
            0x100E => Some(NicVariant::E1000),  // 82540EM
            0x100F => Some(NicVariant::E1000),  // 82545EM (copper)
            0x1015 => Some(NicVariant::E1000),  // 82540EM (LOM)

            // I219-V family (e1000e)
            0x15B8 => Some(NicVariant::I219V),  // I219-V (Skylake)
            0x15D8 => Some(NicVariant::I219V),  // I219-V (Cannon Lake)
            0x15BE => Some(NicVariant::I219V),  // I219-LM (Cannon Lake)
            0x0D4F => Some(NicVariant::I219V),  // I219-V (Comet Lake)
            0x0D4E => Some(NicVariant::I219V),  // I219-LM (Comet Lake)
            0x15FB => Some(NicVariant::I219V),  // I219-V (Comet Lake-S)
            0x15FC => Some(NicVariant::I219V),  // I219-LM (Comet Lake-S)
            0x1A1E => Some(NicVariant::I219V),  // I219-V (Rocket Lake)
            0x1A1F => Some(NicVariant::I219V),  // I219-LM (Rocket Lake)
            0x550A => Some(NicVariant::I219V),  // I219-V (Alder Lake)
            0x550B => Some(NicVariant::I219V),  // I219-LM (Alder Lake)

            // I225-V / I226-V family (igc)
            0x15F2 => Some(NicVariant::I225V),  // I225-V
            0x15F3 => Some(NicVariant::I225V),  // I225-LM
            0x3100 => Some(NicVariant::I225V),  // I225-V (rev 03)
            0x125B => Some(NicVariant::I226V),  // I226-V
            0x125C => Some(NicVariant::I226V),  // I226-LM
            0x125D => Some(NicVariant::I226V),  // I226-IT
            0x3101 => Some(NicVariant::I226V),  // I226-V (rev 04)

            _ => None,
        }
    }

    /// Human-readable name for logging.
    pub fn name(&self) -> &'static str {
        match self {
            NicVariant::E1000 => "e1000 (82540EM)",
            NicVariant::I219V => "I219-V (e1000e)",
            NicVariant::I225V => "I225-V (igc 2.5GbE)",
            NicVariant::I226V => "I226-V (igc 2.5GbE)",
        }
    }
}

/// Errors from Intel NIC driver initialization or operation.
#[derive(Debug)]
pub enum IntelNicError {
    /// Software reset timed out.
    ResetTimeout,
    /// EEPROM read timed out or returned invalid data.
    EepromError,
    /// PHY initialization or auto-negotiation failed.
    PhyError,
    /// Transmit ring is full.
    TxRingFull,
    /// Received frame exceeds buffer size.
    RxBufferTooSmall,
    /// Link is down.
    LinkDown,
    /// Generic device error.
    DeviceError,
}
