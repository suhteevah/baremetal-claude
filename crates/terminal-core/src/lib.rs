//! terminal-core — shared grammar for terminal multiplexers.
//!
//! This crate is `#![no_std]` + `alloc`. It knows nothing about pixels,
//! framebuffers, ConPTY, or ANSI escape output. It defines the abstract
//! brain of a terminal multiplexer: key events, commands, input routing,
//! pane grids, and layout trees.

#![no_std]
extern crate alloc;

pub mod key;
pub mod command;
pub mod viewport;
pub mod color;
pub mod cell;
pub mod input;
pub mod pane;
pub mod layout;

/// Stable pane identifier. u64 so it survives serialization across process
/// boundaries (v2 session persistence, v3 named-pipe IPC).
pub type PaneId = u64;

pub use key::{KeyEvent, KeyCode, Modifiers};
pub use command::DashboardCommand;
pub use input::{InputRouter, RouterOutcome};
pub use viewport::CellViewport;
pub use color::Color;
pub use cell::Cell;
pub use pane::Pane;
pub use layout::{Layout, LayoutNode, SplitDirection};
