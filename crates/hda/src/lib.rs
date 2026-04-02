//! Intel High Definition Audio (HDA) driver for ClaudioOS.
//!
//! Implements the Intel HDA specification 1.0a for bare-metal audio playback.
//! Supports codec discovery, widget enumeration, output path finding, and
//! PCM audio streaming via DMA.
//!
//! This crate is `#![no_std]` and uses `extern crate alloc` for DMA buffer
//! allocation. All hardware access is volatile MMIO through BAR0.

#![no_std]

extern crate alloc;

pub mod registers;
pub mod codec;
pub mod widget;
pub mod stream;
pub mod driver;

pub use driver::{HdaController, HdaOutput};
pub use widget::WidgetType;
pub use stream::StreamFormat;
