//! Render helpers — re-exported from terminal-fb.
//!
//! Previously this module defined Color, DrawTarget, render_char, fill_rect,
//! FONT_WIDTH, and FONT_HEIGHT directly.  They now come from terminal-fb so
//! that claudio-terminal and claudio-mux share exactly the same trait objects.

pub use terminal_fb::render::{
    FONT_HEIGHT,
    FONT_WIDTH,
    DrawTarget,
    fill_rect,
    render_char,
};
pub use terminal_core::Color;
