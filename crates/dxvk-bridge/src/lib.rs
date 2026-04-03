//! # claudio-dxvk-bridge
//!
//! DirectX to Vulkan translation layer for ClaudioOS.
//!
//! Implements the DXVK approach: D3D9, D3D11, D3D12, and DXGI interfaces are
//! presented as COM objects that translate DirectX API calls into Vulkan commands
//! via the `claudio-vulkan` crate.
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────┐
//! │               Windows Application (PE)                │
//! ├──────────┬──────────┬──────────┬─────────────────────┤
//! │  d3d9    │  d3d11   │  d3d12   │       dxgi          │
//! │  .rs     │  .rs     │  .rs     │       .rs           │
//! ├──────────┴──────────┴──────────┴─────────────────────┤
//! │                    com.rs                              │
//! │            (IUnknown, ref counting, GUID)              │
//! ├──────────────────────────────────────────────────────┤
//! │                claudio-vulkan                          │
//! │          (Vulkan 1.3 implementation)                   │
//! └──────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! When a PE binary imports `d3d9.dll!Direct3DCreate9`, the Win32 dispatcher
//! resolves it to our `d3d9::direct3d_create9` function, which returns a
//! COM object implementing IDirect3D9 that internally creates Vulkan resources.

#![no_std]

extern crate alloc;

pub mod com;
pub mod d3d9;
pub mod d3d11;
pub mod d3d12;
pub mod dxgi;

/// Initialize the DXVK bridge subsystem.
pub fn init() {
    log::info!("[dxvk-bridge] Initializing DirectX to Vulkan translation layer");
    com::init();
    dxgi::init();
    log::info!("[dxvk-bridge] DXVK bridge ready");
}
