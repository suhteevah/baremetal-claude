//! Font rendering using noto-sans-mono-bitmap.
//!
//! Provides character-level rendering to any [`DrawTarget`](super::DrawTarget)
//! using pre-rasterised glyphs from the Noto Sans Mono font.

use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight};

/// Height of each character cell in pixels.
pub const FONT_HEIGHT: usize = 16;

/// Width of each character cell in pixels (monospace — constant for all glyphs).
pub const FONT_WIDTH: usize = get_raster_width(FontWeight::Regular, RasterHeight::Size16);

/// An RGB colour triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    // Standard 8-colour palette (SGR 30–37) — CGA-ish values.
    pub const BLACK: Self = Self::new(0, 0, 0);
    pub const RED: Self = Self::new(204, 0, 0);
    pub const GREEN: Self = Self::new(0, 204, 0);
    pub const YELLOW: Self = Self::new(204, 204, 0);
    pub const BLUE: Self = Self::new(0, 0, 204);
    pub const MAGENTA: Self = Self::new(204, 0, 204);
    pub const CYAN: Self = Self::new(0, 204, 204);
    pub const WHITE: Self = Self::new(204, 204, 204);

    // Bright variants (SGR 90–97).
    pub const BRIGHT_BLACK: Self = Self::new(128, 128, 128);
    pub const BRIGHT_RED: Self = Self::new(255, 85, 85);
    pub const BRIGHT_GREEN: Self = Self::new(85, 255, 85);
    pub const BRIGHT_YELLOW: Self = Self::new(255, 255, 85);
    pub const BRIGHT_BLUE: Self = Self::new(85, 85, 255);
    pub const BRIGHT_MAGENTA: Self = Self::new(255, 85, 255);
    pub const BRIGHT_CYAN: Self = Self::new(85, 255, 255);
    pub const BRIGHT_WHITE: Self = Self::new(255, 255, 255);

    // Semantic aliases used as defaults.
    pub const DEFAULT_FG: Self = Self::WHITE;
    pub const DEFAULT_BG: Self = Self::new(16, 16, 16);
}

/// Render a single character glyph into `target` at pixel position (`x`, `y`).
///
/// Missing glyphs are replaced with `'?'`.
pub fn render_char<D: super::DrawTarget>(
    target: &mut D,
    x: usize,
    y: usize,
    c: char,
    fg: Color,
    bg: Color,
) {
    let raster = get_raster(c, FontWeight::Regular, RasterHeight::Size16)
        .unwrap_or_else(|| {
            get_raster('?', FontWeight::Regular, RasterHeight::Size16)
                .expect("fallback glyph '?' must exist")
        });

    for (row_idx, row) in raster.raster().iter().enumerate() {
        for (col_idx, &intensity) in row.iter().enumerate() {
            let px = x + col_idx;
            let py = y + row_idx;
            // Blend foreground and background based on glyph intensity.
            // For speed we use a simple threshold; a full blend would be:
            //   out = bg + (fg - bg) * intensity / 255
            // but the threshold keeps the hot path branchless-friendly.
            if intensity > 128 {
                target.put_pixel(px, py, fg.r, fg.g, fg.b);
            } else {
                target.put_pixel(px, py, bg.r, bg.g, bg.b);
            }
        }
    }
}

/// Fill a rectangular region with a solid colour.
pub fn fill_rect<D: super::DrawTarget>(
    target: &mut D,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: Color,
) {
    for py in y..y + h {
        for px in x..x + w {
            target.put_pixel(px, py, color.r, color.g, color.b);
        }
    }
}
