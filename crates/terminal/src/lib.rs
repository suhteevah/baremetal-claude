//! Split-pane terminal renderer for GOP framebuffer.
//!
//! Layout tree of viewports, each with independent VTE parser + scroll state.
//! This crate is `#![no_std]` and depends only on `alloc`.
//!
//! ## Shim layer
//!
//! `render`, `pane`, and `layout` previously lived entirely in this crate.
//! They now delegate to `terminal-core` (cell logic) and `terminal-fb`
//! (pixel rendering) for the shared types, while keeping the pixel-based
//! `Viewport`, `LayoutNode`, `Layout`, and `Pane` wrappers that the kernel
//! uses directly.

#![no_std]
extern crate alloc;

pub mod layout;
pub mod pane;
pub mod render;
pub mod terminus;
pub mod unicode_font;

// Re-export the main public types for convenient use from the kernel.
pub use layout::Layout;
pub use pane::{Cell, Pane};
pub use render::{fill_rect, render_char, Color, FONT_HEIGHT, FONT_WIDTH};

// DrawTarget is defined in terminal-fb; re-export it as the canonical trait
// so the kernel's `impl claudio_terminal::DrawTarget` still compiles.
pub use terminal_fb::render::DrawTarget;

// SplitDirection and DashboardCommand come from terminal-core.
pub use terminal_core::{SplitDirection, DashboardCommand};

/// A rectangular pixel region within the framebuffer.
#[derive(Debug, Clone)]
pub struct Viewport {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// A node in the binary layout tree.
#[derive(Debug)]
pub enum LayoutNode {
    /// A terminal pane occupying this viewport.
    Leaf {
        pane_id: usize,
        viewport: Viewport,
    },
    /// Two children separated by a divider.
    Split {
        direction: SplitDirection,
        /// Fraction of space given to `first` (0.0–1.0).
        ratio: f32,
        first: alloc::boxed::Box<LayoutNode>,
        second: alloc::boxed::Box<LayoutNode>,
    },
}
