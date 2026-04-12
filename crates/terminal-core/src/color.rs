//! Terminal color representation.

/// RGB color value.
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

    pub const BLACK: Self = Self::new(0, 0, 0);
    pub const RED: Self = Self::new(204, 0, 0);
    pub const GREEN: Self = Self::new(0, 204, 0);
    pub const YELLOW: Self = Self::new(204, 204, 0);
    pub const BLUE: Self = Self::new(0, 0, 204);
    pub const MAGENTA: Self = Self::new(204, 0, 204);
    pub const CYAN: Self = Self::new(0, 204, 204);
    pub const WHITE: Self = Self::new(204, 204, 204);

    pub const BRIGHT_BLACK: Self = Self::new(128, 128, 128);
    pub const BRIGHT_RED: Self = Self::new(255, 85, 85);
    pub const BRIGHT_GREEN: Self = Self::new(85, 255, 85);
    pub const BRIGHT_YELLOW: Self = Self::new(255, 255, 85);
    pub const BRIGHT_BLUE: Self = Self::new(85, 85, 255);
    pub const BRIGHT_MAGENTA: Self = Self::new(255, 85, 255);
    pub const BRIGHT_CYAN: Self = Self::new(85, 255, 255);
    pub const BRIGHT_WHITE: Self = Self::new(255, 255, 255);

    pub const DEFAULT_FG: Self = Self::WHITE;
    pub const DEFAULT_BG: Self = Self::new(16, 16, 16);
}
