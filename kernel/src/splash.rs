//! Boot splash screen -- ASCII art logo with progress bar.
//!
//! Renders to the framebuffer via the terminal crate's Terminus bitmap font
//! and the kernel's `FramebufferDrawTarget`. The splash is shown during early
//! boot and replaced by the dashboard once ready.

use crate::framebuffer;
use crate::terminal::FramebufferDrawTarget;
use claudio_terminal::render::{render_char, fill_rect, Color, FONT_WIDTH, FONT_HEIGHT};

/// Boot stage identifiers for the progress bar.
#[derive(Clone, Copy)]
pub enum BootStage {
    Hardware,       // 25%
    Network,        // 50%
    Authenticating, // 75%
    Ready,          // 100%
}

impl BootStage {
    fn progress(self) -> usize {
        match self {
            BootStage::Hardware => 25,
            BootStage::Network => 50,
            BootStage::Authenticating => 75,
            BootStage::Ready => 100,
        }
    }

    fn message(self) -> &'static str {
        match self {
            BootStage::Hardware => "Initializing hardware...",
            BootStage::Network => "Starting network...",
            BootStage::Authenticating => "Authenticating...",
            BootStage::Ready => "Ready!",
        }
    }
}

// -- ASCII art logo -------------------------------------------------------
// The Terminus bitmap font covers ASCII 32..126, so we use ASCII-art borders
// instead of Unicode box-drawing characters.

const SPLASH_LINES: &[&str] = &[
    "+===================================================+",
    "|                                                   |",
    "|         CCCC  L       AA   U   U                  |",
    "|        C      L      A  A  U   U                  |",
    "|        C      L      AAAA  U   U                  |",
    "|        C      L      A  A  U   U                  |",
    "|         CCCC  LLLLL  A  A   UUU                   |",
    "|                                                   |",
    "|                D I O   O S                         |",
    "|                                                   |",
    "|          Bare Metal AI Agent Platform              |",
    "|                                                   |",
    "+===================================================+",
];

/// Progress bar width in characters.
const BAR_WIDTH: usize = 40;

// -- Colours ---------------------------------------------------------------

const CYAN: Color = Color::new(0, 220, 255);
const WHITE: Color = Color::new(220, 220, 220);
const GRAY: Color = Color::new(140, 140, 140);
const GREEN: Color = Color::new(0, 220, 80);
const DIM_GREEN: Color = Color::new(0, 80, 30);
const BG: Color = Color::new(0, 0, 0);

// -- Helpers ---------------------------------------------------------------

/// Render a string at character grid position (col, row) with fg colour.
fn draw_str(dt: &mut FramebufferDrawTarget, col: usize, row: usize, s: &str, fg: Color) {
    let mut c = col;
    for ch in s.chars() {
        let px = c * FONT_WIDTH;
        let py = row * FONT_HEIGHT;
        render_char(dt, px, py, ch, fg, BG);
        c += 1;
    }
}

/// Show the splash screen with the given boot stage.
///
/// Re-renders the full splash (logo + progress bar + message) and blits
/// to the hardware framebuffer. Call at each boot stage transition.
pub fn show_splash(stage: BootStage) {
    let fb_w = framebuffer::width();
    let fb_h = framebuffer::height();
    if fb_w == 0 || fb_h == 0 {
        return;
    }

    let mut dt = FramebufferDrawTarget;

    // Clear to black.
    fill_rect(&mut dt, 0, 0, fb_w, fb_h, BG);

    // -- Compute centering (in character cells) ---------------------------
    let cols = fb_w / FONT_WIDTH;
    let rows = fb_h / FONT_HEIGHT;

    let logo_width = 53; // characters per splash line
    let logo_height = SPLASH_LINES.len();

    // Total block: logo + 1 blank + progress bar + 1 blank + message = logo + 4
    let total_rows = logo_height + 4;
    let start_row = if rows > total_rows { (rows - total_rows) / 2 } else { 0 };
    let start_col = if cols > logo_width { (cols - logo_width) / 2 } else { 0 };

    // -- Render logo lines ------------------------------------------------
    for (i, line) in SPLASH_LINES.iter().enumerate() {
        let row = start_row + i;
        let is_border = i == 0 || i == SPLASH_LINES.len() - 1;
        let is_title = (2..=6).contains(&i);
        let is_dio = i == 8;
        let is_subtitle = i == 10;

        if is_border {
            draw_str(&mut dt, start_col, row, line, WHITE);
        } else if is_title {
            // Border chars (|) in white, letter chars in cyan
            for (ci, ch) in line.chars().enumerate() {
                let px = (start_col + ci) * FONT_WIDTH;
                let py = row * FONT_HEIGHT;
                if ch == '|' {
                    render_char(&mut dt, px, py, ch, WHITE, BG);
                } else if ch != ' ' {
                    render_char(&mut dt, px, py, ch, CYAN, BG);
                }
                // spaces are left as background
            }
        } else if is_dio {
            for (ci, ch) in line.chars().enumerate() {
                let px = (start_col + ci) * FONT_WIDTH;
                let py = row * FONT_HEIGHT;
                if ch == '|' {
                    render_char(&mut dt, px, py, ch, WHITE, BG);
                } else if ch != ' ' {
                    render_char(&mut dt, px, py, ch, CYAN, BG);
                }
            }
        } else if is_subtitle {
            for (ci, ch) in line.chars().enumerate() {
                let px = (start_col + ci) * FONT_WIDTH;
                let py = row * FONT_HEIGHT;
                if ch == '|' {
                    render_char(&mut dt, px, py, ch, WHITE, BG);
                } else if ch != ' ' {
                    render_char(&mut dt, px, py, ch, GRAY, BG);
                }
            }
        } else {
            draw_str(&mut dt, start_col, row, line, WHITE);
        }
    }

    // -- Progress bar -----------------------------------------------------
    let bar_row = start_row + logo_height + 1;
    let progress = stage.progress();
    let filled = (BAR_WIDTH * progress) / 100;
    let empty = BAR_WIDTH - filled;

    // Center the bar: "[" + BAR_WIDTH chars + "]" = BAR_WIDTH + 2
    let bar_total = BAR_WIDTH + 2;
    let bar_col = if cols > bar_total { (cols - bar_total) / 2 } else { 0 };

    // "[" bracket
    let px = bar_col * FONT_WIDTH;
    let py = bar_row * FONT_HEIGHT;
    render_char(&mut dt, px, py, '[', WHITE, BG);

    // Filled blocks
    for i in 0..filled {
        let px = (bar_col + 1 + i) * FONT_WIDTH;
        render_char(&mut dt, px, py, '#', GREEN, BG);
    }
    // Empty blocks
    for i in 0..empty {
        let px = (bar_col + 1 + filled + i) * FONT_WIDTH;
        render_char(&mut dt, px, py, '-', DIM_GREEN, BG);
    }
    // "]" bracket
    let px = (bar_col + 1 + BAR_WIDTH) * FONT_WIDTH;
    render_char(&mut dt, px, py, ']', WHITE, BG);

    // -- Stage message ----------------------------------------------------
    let msg = stage.message();
    let msg_len = msg.len();
    let msg_col = if cols > msg_len { (cols - msg_len) / 2 } else { 0 };
    let msg_row = bar_row + 2;

    let msg_color = if matches!(stage, BootStage::Ready) { GREEN } else { GRAY };
    draw_str(&mut dt, msg_col, msg_row, msg, msg_color);

    // -- Blit to front buffer ---------------------------------------------
    framebuffer::blit_full();
    log::info!("[splash] boot stage: {} ({}%)", msg, progress);
}

/// Clear the splash screen (fill black) and blit -- hand off to the dashboard.
pub fn hide_splash() {
    let fb_w = framebuffer::width();
    let fb_h = framebuffer::height();
    if fb_w == 0 || fb_h == 0 {
        return;
    }
    let mut dt = FramebufferDrawTarget;
    fill_rect(&mut dt, 0, 0, fb_w, fb_h, BG);
    framebuffer::blit_full();
    log::info!("[splash] splash screen cleared");
}
