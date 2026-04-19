//! Realtek RTL8168 / RTL8111 GigE NIC driver for ClaudioOS.
//!
//! PCI: vendor 0x10EC, device 0x8168 — HP Victus 15-fa2 at bus 04:00.0.
//! MMIO via BAR2 (256-byte PCIe config space).

#![no_std]
extern crate alloc;

pub mod descriptors;
pub mod driver;
pub mod regs;

pub use driver::{DiagRegs, Rtl8168, Rtl8168InitError};

pub const REALTEK_VENDOR_ID: u16 = 0x10EC;
pub const RTL8168_DEVICE_ID: u16 = 0x8168;
