//! # claudio-winrt
//!
//! A `no_std` Windows Runtime (WinRT) implementation for ClaudioOS.
//!
//! Provides WinRT activation factories, type projections, .winmd metadata
//! parsing, and IAsyncOperation/IAsyncAction patterns. Enables running
//! UWP/WinRT-based applications on bare metal.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │          WinRtRuntime (driver)           │
//! ├──────────┬──────────┬───────────────────┤
//! │Activation│ Metadata │    Async Ops      │
//! │ Factory  │ (.winmd) │ IAsyncOperation   │
//! ├──────────┴──────────┴───────────────────┤
//! │         Type Projections                 │
//! │ Windows.Foundation, Windows.Storage, ... │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## How it works
//!
//! 1. **Activation**: `RoActivateInstance` creates WinRT objects by class name.
//! 2. **Metadata**: Parse .winmd files (same ECMA-335 format as .NET metadata).
//! 3. **Projections**: Map WinRT types to Rust-friendly wrappers.
//! 4. **Async**: IAsyncOperation<T> and IAsyncAction wrap async operations.

#![no_std]

extern crate alloc;

pub mod activation;
pub mod metadata;
pub mod projections;
pub mod async_ops;
pub mod driver;
