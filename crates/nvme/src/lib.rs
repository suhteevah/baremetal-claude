//! # claudio-nvme — Bare-metal NVMe driver for ClaudioOS
//!
//! A `#![no_std]` NVM Express driver that speaks directly to NVMe controllers
//! via PCI BAR0 memory-mapped registers. Implements admin commands (Identify,
//! Create I/O Queue) and I/O commands (Read, Write, Flush) per NVMe 1.4+ spec.
//!
//! ## Architecture
//!
//! ```text
//! NvmeController          — owns BAR0, admin queue, controller identity
//!   └─ NvmeDisk           — namespace handle with read/write/flush + BlockDevice
//!        └─ QueuePair     — submission + completion queue with doorbell access
//! ```
//!
//! ## Usage
//!
//! ```rust,no_run
//! use claudio_nvme::NvmeController;
//!
//! // BAR0 physical address from PCI enumeration (must be identity-mapped)
//! let mut ctrl = NvmeController::init(bar0_addr).expect("nvme init failed");
//! let mut disk = ctrl.namespace(1).expect("namespace 1 not found");
//! let mut buf = [0u8; 512];
//! disk.read_sectors(0, 1, &mut buf).expect("read failed");
//! ```

#![no_std]

extern crate alloc;

pub mod registers;
pub mod queue;
pub mod admin;
pub mod io;
pub mod driver;

pub use driver::{NvmeController, NvmeDisk, NvmeError};
