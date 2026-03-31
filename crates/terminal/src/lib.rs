//! Split-pane terminal renderer for GOP framebuffer.
//!
//! Layout tree of viewports, each with independent VTE parser + scroll state.
//! This crate is `#![no_std]` and depends only on `alloc`.

#![no_std]
extern crate alloc;

pub mod layout;
pub mod pane;
pub mod render;

// Re-export the main public types for convenient use from the kernel.
pub use layout::Layout;
pub use pane::{Cell, Pane};
pub use render::{fill_rect, render_char, Color, FONT_HEIGHT, FONT_WIDTH};

/// Abstraction over a pixel framebuffer.
///
/// The kernel implements this for its GOP framebuffer so the terminal crate
/// can draw without knowing the specifics of the backing store.
pub trait DrawTarget {
    /// Write a single pixel at (`x`, `y`) with the given RGB values.
    fn put_pixel(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8);
    /// Framebuffer width in pixels.
    fn width(&self) -> usize;
    /// Framebuffer height in pixels.
    fn height(&self) -> usize;
}

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

/// Direction of a split between two panes.
#[derive(Debug, Clone, Copy)]
pub enum SplitDirection {
    /// Left | Right
    Vertical,
    /// Top / Bottom
    Horizontal,
}

/// High-level commands that the keyboard handler can dispatch to the layout.
#[derive(Debug)]
pub enum DashboardCommand {
    SplitVertical,
    SplitHorizontal,
    FocusNext,
    FocusPrev,
    ClosePane,
    NewAgent,
    ToggleStatusBar,
}
