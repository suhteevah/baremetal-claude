//! AHCI (Advanced Host Controller Interface) SATA driver for ClaudioOS.
//!
//! This crate provides a bare-metal AHCI driver that can detect SATA drives
//! and perform sector-level read/write I/O. It is `#![no_std]` and uses only
//! volatile MMIO — no Linux kernel, no POSIX, no userspace abstractions.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use claudio_ahci::AhciController;
//!
//! // After PCI enumeration finds an AHCI controller (class 0x01, subclass 0x06),
//! // read BAR5 to get the ABAR (AHCI Base Address Register).
//! let abar: u64 = /* PCI BAR5 */ 0xFEB0_0000;
//! let mut controller = AhciController::init(abar);
//! // Now use controller.disks() to access detected drives.
//! ```

#![no_std]

extern crate alloc;

pub mod hba;
pub mod port;
pub mod command;
pub mod identify;
pub mod driver;

/// Callback to translate virtual addresses to physical addresses for DMA.
/// The kernel provides this at init time by walking CR3 page tables.
/// Heap addresses are NOT identity-mapped, so `ptr as u64` is wrong for DMA.
pub type VirtToPhys = fn(usize) -> u64;

pub use driver::{AhciController, AhciDisk, AhciError};
