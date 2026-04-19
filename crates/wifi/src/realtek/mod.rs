//! Realtek rtw89-equivalent driver stack (RTL8852BE and friends).
//!
//! All modules are stubs pending implementation. Every file here mirrors
//! the shape of `crate::intel::*` so the `VendorBackend` trait can later
//! be implemented against real transports without further restructuring.

pub mod pci;
pub mod firmware;
pub mod commands;
pub mod tx_rx;
pub mod driver;

pub use driver::{ProbeReading, RealtekController, RealtekProbe, Stub, PROBE_DUMP_WORDS};
