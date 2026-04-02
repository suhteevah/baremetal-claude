//! # claudio-gpu — Bare-metal NVIDIA GPU compute driver
//!
//! This crate provides direct GPU access for ClaudioOS without any proprietary
//! CUDA runtime or Linux kernel drivers. We talk to the hardware through MMIO
//! registers, following the architecture documented by the nouveau project and
//! envytools.
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │                    tensor.rs                              │
//! │  (TensorDescriptor, matmul, softmax, layernorm, GELU)    │
//! ├──────────────────────────────────────────────────────────┤
//! │                   compute.rs                              │
//! │  (Compute class setup, shader load, grid dispatch)        │
//! ├──────────────────────────────────────────────────────────┤
//! │                    fifo.rs                                │
//! │  (GPFIFO channels, push buffers, runlists, doorbells)     │
//! ├──────────────────────────────────────────────────────────┤
//! │                   falcon.rs                               │
//! │  (Falcon microcontroller: PMU, SEC2, GSP-RM firmware)     │
//! ├────────────────┬─────────────────────────────────────────┤
//! │  memory.rs     │              mmio.rs                     │
//! │  (VRAM, GPU    │  (NV_PMC, PFIFO, PFB, PGRAPH, etc.)     │
//! │   page tables, │                                          │
//! │   DMA mapping) │                                          │
//! ├────────────────┴─────────────────────────────────────────┤
//! │                  pci_config.rs                             │
//! │  (PCI vendor 0x10DE detect, BAR mapping, bus mastering)   │
//! ├──────────────────────────────────────────────────────────┤
//! │                   driver.rs                               │
//! │  (GpuDevice: high-level init, query, compute API)         │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Status
//!
//! This is scaffolding for an extraordinarily ambitious bare-metal GPU driver.
//! Real GPU initialization requires uploading signed firmware to Falcon
//! microcontrollers, constructing GPU page tables, programming the FIFO engine,
//! and speaking the compute class protocol — all of which NVIDIA keeps largely
//! undocumented. The nouveau project has reverse-engineered much of this over
//! 15+ years. We stand on their shoulders.

#![no_std]

extern crate alloc;

pub mod pci_config;
pub mod mmio;
pub mod memory;
pub mod falcon;
pub mod fifo;
pub mod compute;
pub mod tensor;
pub mod driver;

pub use driver::GpuDevice;
pub use pci_config::{GpuFamily, GpuPciDevice};
pub use memory::VramAllocator;
pub use tensor::{TensorDescriptor, DType};
pub use compute::ComputeEngine;
