//! GDI32.dll API implementation — 2D graphics rendering.
//!
//! GDI objects (DCs, bitmaps, fonts, pens, brushes) render to framebuffer
//! back-buffers. BitBlt/StretchBlt composites to the GOP framebuffer.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::unicode::*;
use crate::handles::{self, HandleObject};
use crate::teb_peb;

// =============================================================================
// Device Context (DC) state
// =============================================================================

/// Internal DC state.
struct DcState {
    window: Handle,
    width: u32,
    height: u32,
    /// Framebuffer pixel data (BGRA 32-bit).
    pixels: Vec<u32>,
    /// Current drawing position for LineTo/MoveToEx.
    pos_x: i32,
    pos_y: i32,
    /// Selected pen color (COLORREF = 0x00BBGGRR).
    pen_color: u32,
    /// Selected brush color.
    brush_color: u32,
    /// Text color.
    text_color: u32,
    /// Background color.
    bk_color: u32,
    /// Background mode (OPAQUE=2, TRANSPARENT=1).
    bk_mode: i32,
    /// Selected font handle.
    selected_font: Handle,
    /// Selected bitmap handle.
    selected_bitmap: Handle,
}

/// Global DC state table.
static DC_TABLE: Mutex<Option<BTreeMap<u64, DcState>>> = Mutex::new(None);

/// Next DC handle counter.
static NEXT_DC: Mutex<u64> = Mutex::new(0x8000_0000);

/// Allocate a new DC handle.
fn alloc_dc(state: DcState) -> Hdc {
    let mut counter = NEXT_DC.lock();
    let hdc = *counter;
    *counter += 1;

    let mut table = DC_TABLE.lock();
    if table.is_none() {
        *table = Some(BTreeMap::new());
    }
    if let Some(ref mut map) = *table {
        map.insert(hdc, state);
    }
    hdc
}

/// Get mutable access to a DC.
fn with_dc_mut<F, R>(hdc: Hdc, f: F) -> Option<R>
where
    F: FnOnce(&mut DcState) -> R,
{
    let mut table = DC_TABLE.lock();
    table.as_mut().and_then(|map| map.get_mut(&hdc).map(f))
}

/// Get read access to a DC.
fn with_dc<F, R>(hdc: Hdc, f: F) -> Option<R>
where
    F: FnOnce(&DcState) -> R,
{
    let table = DC_TABLE.lock();
    table.as_ref().and_then(|map| map.get(&hdc).map(f))
}

// =============================================================================
// DC creation / deletion
// =============================================================================

/// Default framebuffer dimensions (used for screen DCs).
const DEFAULT_WIDTH: u32 = 1920;
const DEFAULT_HEIGHT: u32 = 1080;

/// CreateDCW — create a device context for a device.
pub fn create_dc_w(
    driver: LpcWStr,
    device: LpcWStr,
    output: LpcWStr,
    init_data: u64,
) -> Hdc {
    let driver_str = if !driver.is_null() { unsafe { utf16_to_utf8(driver) } } else { String::new() };
    log::debug!("[gdi32] CreateDCW: driver='{}'", driver_str);

    alloc_dc(DcState {
        window: 0,
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        pixels: vec![0u32; (DEFAULT_WIDTH * DEFAULT_HEIGHT) as usize],
        pos_x: 0,
        pos_y: 0,
        pen_color: 0x00000000,   // Black
        brush_color: 0x00FFFFFF, // White
        text_color: 0x00000000,
        bk_color: 0x00FFFFFF,
        bk_mode: 2, // OPAQUE
        selected_font: 0,
        selected_bitmap: 0,
    })
}

/// CreateCompatibleDC — create a memory DC compatible with the given DC.
pub fn create_compatible_dc(hdc: Hdc) -> Hdc {
    log::trace!("[gdi32] CreateCompatibleDC: hdc=0x{:X}", hdc);

    let (width, height) = with_dc(hdc, |dc| (dc.width, dc.height))
        .unwrap_or((DEFAULT_WIDTH, DEFAULT_HEIGHT));

    alloc_dc(DcState {
        window: 0,
        width,
        height,
        pixels: vec![0u32; (width * height) as usize],
        pos_x: 0,
        pos_y: 0,
        pen_color: 0x00000000,
        brush_color: 0x00FFFFFF,
        text_color: 0x00000000,
        bk_color: 0x00FFFFFF,
        bk_mode: 2,
        selected_font: 0,
        selected_bitmap: 0,
    })
}

/// DeleteDC — delete a device context.
pub fn delete_dc(hdc: Hdc) -> Bool {
    log::trace!("[gdi32] DeleteDC: hdc=0x{:X}", hdc);
    let mut table = DC_TABLE.lock();
    if let Some(ref mut map) = *table {
        map.remove(&hdc);
    }
    TRUE
}

/// GetDC — get the DC for a window.
pub fn get_dc(hwnd: HWnd) -> Hdc {
    log::trace!("[gdi32] GetDC: hwnd=0x{:X}", hwnd);

    // Get window dimensions if available
    let (w, h) = if let Some(HandleObject::Window { width, height, .. }) =
        handles::get_handle(hwnd)
    {
        (width as u32, height as u32)
    } else {
        (DEFAULT_WIDTH, DEFAULT_HEIGHT)
    };

    alloc_dc(DcState {
        window: hwnd,
        width: w,
        height: h,
        pixels: vec![0u32; (w * h) as usize],
        pos_x: 0,
        pos_y: 0,
        pen_color: 0x00000000,
        brush_color: 0x00FFFFFF,
        text_color: 0x00000000,
        bk_color: 0x00FFFFFF,
        bk_mode: 2,
        selected_font: 0,
        selected_bitmap: 0,
    })
}

/// ReleaseDC — release a window DC obtained by GetDC.
pub fn release_dc(hwnd: HWnd, hdc: Hdc) -> i32 {
    log::trace!("[gdi32] ReleaseDC: hwnd=0x{:X}, hdc=0x{:X}", hwnd, hdc);
    delete_dc(hdc);
    1
}

// =============================================================================
// Bitmap operations
// =============================================================================

/// Bitmap state.
struct BitmapState {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
}

/// Global bitmap table.
static BITMAP_TABLE: Mutex<Option<BTreeMap<u64, BitmapState>>> = Mutex::new(None);
static NEXT_BITMAP: Mutex<u64> = Mutex::new(0xA000_0000);

/// CreateCompatibleBitmap.
pub fn create_compatible_bitmap(hdc: Hdc, width: i32, height: i32) -> HBitmap {
    log::trace!("[gdi32] CreateCompatibleBitmap: {}x{}", width, height);

    let w = width.max(1) as u32;
    let h = height.max(1) as u32;

    let mut counter = NEXT_BITMAP.lock();
    let handle = *counter;
    *counter += 1;

    let mut table = BITMAP_TABLE.lock();
    if table.is_none() {
        *table = Some(BTreeMap::new());
    }
    if let Some(ref mut map) = *table {
        map.insert(handle, BitmapState {
            width: w,
            height: h,
            pixels: vec![0u32; (w * h) as usize],
        });
    }

    handle
}

/// Raster operation codes for BitBlt.
pub const SRCCOPY: DWord = 0x00CC0020;
pub const SRCPAINT: DWord = 0x00EE0086;
pub const SRCAND: DWord = 0x008800C6;
pub const SRCINVERT: DWord = 0x00660046;
pub const BLACKNESS: DWord = 0x00000042;
pub const WHITENESS: DWord = 0x00FF0062;

/// BitBlt — bit-block transfer between DCs.
pub fn bit_blt(
    hdc_dest: Hdc,
    x_dest: i32,
    y_dest: i32,
    width: i32,
    height: i32,
    hdc_src: Hdc,
    x_src: i32,
    y_src: i32,
    rop: DWord,
) -> Bool {
    log::trace!(
        "[gdi32] BitBlt: dest=0x{:X} ({},{}) {}x{} <- src=0x{:X} ({},{}) rop=0x{:X}",
        hdc_dest, x_dest, y_dest, width, height, hdc_src, x_src, y_src, rop
    );

    // For BLACKNESS/WHITENESS, just fill the dest region
    if rop == BLACKNESS || rop == WHITENESS {
        let fill_color = if rop == BLACKNESS { 0x00000000 } else { 0x00FFFFFF };
        with_dc_mut(hdc_dest, |dc| {
            for dy in 0..height {
                for dx in 0..width {
                    let px = (x_dest + dx) as u32;
                    let py = (y_dest + dy) as u32;
                    if px < dc.width && py < dc.height {
                        dc.pixels[(py * dc.width + px) as usize] = fill_color;
                    }
                }
            }
        });
        return TRUE;
    }

    // Read source pixels
    let src_pixels: Option<Vec<u32>> = with_dc(hdc_src, |src| {
        let mut buf = Vec::with_capacity((width * height) as usize);
        for dy in 0..height {
            for dx in 0..width {
                let sx = (x_src + dx) as u32;
                let sy = (y_src + dy) as u32;
                if sx < src.width && sy < src.height {
                    buf.push(src.pixels[(sy * src.width + sx) as usize]);
                } else {
                    buf.push(0);
                }
            }
        }
        buf
    });

    if let Some(src_px) = src_pixels {
        with_dc_mut(hdc_dest, |dc| {
            for dy in 0..height {
                for dx in 0..width {
                    let px = (x_dest + dx) as u32;
                    let py = (y_dest + dy) as u32;
                    if px < dc.width && py < dc.height {
                        let idx = (dy * width + dx) as usize;
                        let dest_idx = (py * dc.width + px) as usize;
                        match rop {
                            SRCCOPY => dc.pixels[dest_idx] = src_px[idx],
                            SRCPAINT => dc.pixels[dest_idx] |= src_px[idx],
                            SRCAND => dc.pixels[dest_idx] &= src_px[idx],
                            SRCINVERT => dc.pixels[dest_idx] ^= src_px[idx],
                            _ => dc.pixels[dest_idx] = src_px[idx],
                        }
                    }
                }
            }
        });
    }

    TRUE
}

/// StretchBlt — stretch bit-block transfer.
pub fn stretch_blt(
    hdc_dest: Hdc,
    x_dest: i32,
    y_dest: i32,
    w_dest: i32,
    h_dest: i32,
    hdc_src: Hdc,
    x_src: i32,
    y_src: i32,
    w_src: i32,
    h_src: i32,
    rop: DWord,
) -> Bool {
    log::trace!(
        "[gdi32] StretchBlt: dest=0x{:X} ({},{}) {}x{} <- src=0x{:X} ({},{}) {}x{}",
        hdc_dest, x_dest, y_dest, w_dest, h_dest, hdc_src, x_src, y_src, w_src, h_src
    );

    // Read source pixels
    let src_pixels: Option<(Vec<u32>, u32, u32)> = with_dc(hdc_src, |src| {
        let mut buf = Vec::with_capacity((w_src * h_src) as usize);
        for dy in 0..h_src {
            for dx in 0..w_src {
                let sx = (x_src + dx) as u32;
                let sy = (y_src + dy) as u32;
                if sx < src.width && sy < src.height {
                    buf.push(src.pixels[(sy * src.width + sx) as usize]);
                } else {
                    buf.push(0);
                }
            }
        }
        (buf, w_src as u32, h_src as u32)
    });

    if let Some((src_px, sw, sh)) = src_pixels {
        with_dc_mut(hdc_dest, |dc| {
            for dy in 0..h_dest {
                for dx in 0..w_dest {
                    let px = (x_dest + dx) as u32;
                    let py = (y_dest + dy) as u32;
                    if px < dc.width && py < dc.height {
                        // Nearest-neighbor sampling
                        let sx = ((dx as u32 * sw) / w_dest as u32).min(sw - 1);
                        let sy = ((dy as u32 * sh) / h_dest as u32).min(sh - 1);
                        let src_idx = (sy * sw + sx) as usize;
                        let dest_idx = (py * dc.width + px) as usize;
                        dc.pixels[dest_idx] = src_px[src_idx];
                    }
                }
            }
        });
    }

    TRUE
}

// =============================================================================
// GDI object selection
// =============================================================================

/// SelectObject — select a GDI object into a DC, return the previous one.
pub fn select_object(hdc: Hdc, obj: HGdiObj) -> HGdiObj {
    log::trace!("[gdi32] SelectObject: hdc=0x{:X}, obj=0x{:X}", hdc, obj);

    // Check if it's a bitmap
    let is_bitmap = {
        let table = BITMAP_TABLE.lock();
        table.as_ref().map(|m| m.contains_key(&obj)).unwrap_or(false)
    };

    if is_bitmap {
        with_dc_mut(hdc, |dc| {
            let prev = dc.selected_bitmap;
            dc.selected_bitmap = obj;
            prev
        }).unwrap_or(0)
    } else {
        // Assume font or other object
        with_dc_mut(hdc, |dc| {
            let prev = dc.selected_font;
            dc.selected_font = obj;
            prev
        }).unwrap_or(0)
    }
}

/// DeleteObject — delete a GDI object (bitmap, font, pen, brush).
pub fn delete_object(obj: HGdiObj) -> Bool {
    log::trace!("[gdi32] DeleteObject: obj=0x{:X}", obj);

    // Try to remove from bitmap table
    let mut table = BITMAP_TABLE.lock();
    if let Some(ref mut map) = *table {
        if map.remove(&obj).is_some() {
            return TRUE;
        }
    }

    TRUE // Always succeed
}

// =============================================================================
// Drawing primitives
// =============================================================================

/// Rectangle — draw a rectangle using the current pen and fill with current brush.
pub fn rectangle(hdc: Hdc, left: i32, top: i32, right: i32, bottom: i32) -> Bool {
    log::trace!("[gdi32] Rectangle: ({},{}) - ({},{})", left, top, right, bottom);

    with_dc_mut(hdc, |dc| {
        let color = dc.brush_color;
        for y in top..bottom {
            for x in left..right {
                let px = x as u32;
                let py = y as u32;
                if px < dc.width && py < dc.height {
                    let idx = (py * dc.width + px) as usize;
                    // Fill interior with brush, draw border with pen
                    if x == left || x == right - 1 || y == top || y == bottom - 1 {
                        dc.pixels[idx] = dc.pen_color;
                    } else {
                        dc.pixels[idx] = color;
                    }
                }
            }
        }
    });

    TRUE
}

/// Ellipse — draw an ellipse bounded by the given rectangle.
pub fn ellipse(hdc: Hdc, left: i32, top: i32, right: i32, bottom: i32) -> Bool {
    log::trace!("[gdi32] Ellipse: ({},{}) - ({},{})", left, top, right, bottom);

    with_dc_mut(hdc, |dc| {
        let cx = (left + right) as f64 / 2.0;
        let cy = (top + bottom) as f64 / 2.0;
        let rx = (right - left) as f64 / 2.0;
        let ry = (bottom - top) as f64 / 2.0;

        for y in top..bottom {
            for x in left..right {
                let dx = (x as f64 - cx) / rx;
                let dy = (y as f64 - cy) / ry;
                let dist = dx * dx + dy * dy;

                let px = x as u32;
                let py = y as u32;
                if px < dc.width && py < dc.height {
                    let idx = (py * dc.width + px) as usize;
                    if dist <= 1.0 {
                        dc.pixels[idx] = dc.brush_color;
                    }
                }
            }
        }
    });

    TRUE
}

/// MoveToEx — move the current drawing position.
pub fn move_to_ex(hdc: Hdc, x: i32, y: i32, prev_point: *mut crate::user32::Point) -> Bool {
    with_dc_mut(hdc, |dc| {
        if !prev_point.is_null() {
            unsafe {
                (*prev_point).x = dc.pos_x;
                (*prev_point).y = dc.pos_y;
            }
        }
        dc.pos_x = x;
        dc.pos_y = y;
    });
    TRUE
}

/// LineTo — draw a line from the current position to (x, y).
pub fn line_to(hdc: Hdc, x: i32, y: i32) -> Bool {
    with_dc_mut(hdc, |dc| {
        // Bresenham's line algorithm
        let x0 = dc.pos_x;
        let y0 = dc.pos_y;
        let x1 = x;
        let y1 = y;

        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx: i32 = if x0 < x1 { 1 } else { -1 };
        let sy: i32 = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let mut cx = x0;
        let mut cy = y0;

        let color = dc.pen_color;
        loop {
            let px = cx as u32;
            let py = cy as u32;
            if px < dc.width && py < dc.height {
                dc.pixels[(py * dc.width + px) as usize] = color;
            }

            if cx == x1 && cy == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                cx += sx;
            }
            if e2 <= dx {
                err += dx;
                cy += sy;
            }
        }

        dc.pos_x = x;
        dc.pos_y = y;
    });
    TRUE
}

/// TextOutW — draw text at the specified position.
pub fn text_out_w(hdc: Hdc, x: i32, y: i32, string: LpcWStr, count: i32) -> Bool {
    if string.is_null() || count <= 0 {
        return FALSE;
    }

    let text = unsafe {
        let slice = core::slice::from_raw_parts(string, count as usize);
        crate::unicode::utf16_slice_to_string(slice)
    };

    log::trace!("[gdi32] TextOutW: ({},{}) '{}'", x, y, text);

    // In a full implementation we'd rasterize glyphs from the selected font.
    // For now, we render a simple 8x16 placeholder per character.
    with_dc_mut(hdc, |dc| {
        let color = dc.text_color;
        let mut cx = x;
        for _ch in text.chars() {
            // Draw a filled rectangle as a placeholder glyph
            for dy in 0..16 {
                for dx in 0..8 {
                    let px = (cx + dx) as u32;
                    let py = (y + dy) as u32;
                    if px < dc.width && py < dc.height {
                        dc.pixels[(py * dc.width + px) as usize] = color;
                    }
                }
            }
            cx += 8;
        }
    });

    TRUE
}

/// SetPixel — set a single pixel's color.
pub fn set_pixel(hdc: Hdc, x: i32, y: i32, color: u32) -> u32 {
    with_dc_mut(hdc, |dc| {
        let px = x as u32;
        let py = y as u32;
        if px < dc.width && py < dc.height {
            dc.pixels[(py * dc.width + px) as usize] = color;
            color
        } else {
            u32::MAX // CLR_INVALID
        }
    }).unwrap_or(u32::MAX)
}

/// GetPixel — get a single pixel's color.
pub fn get_pixel(hdc: Hdc, x: i32, y: i32) -> u32 {
    with_dc(hdc, |dc| {
        let px = x as u32;
        let py = y as u32;
        if px < dc.width && py < dc.height {
            dc.pixels[(py * dc.width + px) as usize]
        } else {
            u32::MAX // CLR_INVALID
        }
    }).unwrap_or(u32::MAX)
}

/// CreateFontW — create a logical font (stub — returns handle, no rasterization).
pub fn create_font_w(
    height: i32,
    width: i32,
    escapement: i32,
    orientation: i32,
    weight: i32,
    italic: DWord,
    underline: DWord,
    strike_out: DWord,
    char_set: DWord,
    out_precision: DWord,
    clip_precision: DWord,
    quality: DWord,
    pitch_and_family: DWord,
    face_name: LpcWStr,
) -> HFont {
    let name = if !face_name.is_null() {
        unsafe { utf16_to_utf8(face_name) }
    } else {
        String::from("System")
    };
    log::debug!("[gdi32] CreateFontW: '{}' h={} w={}", name, height, weight);

    // Return a unique font handle
    static NEXT_FONT: Mutex<u64> = Mutex::new(0xB000_0000);
    let mut counter = NEXT_FONT.lock();
    let handle = *counter;
    *counter += 1;
    handle
}

/// SetTextColor — set the text foreground color for a DC.
pub fn set_text_color(hdc: Hdc, color: u32) -> u32 {
    with_dc_mut(hdc, |dc| {
        let prev = dc.text_color;
        dc.text_color = color;
        prev
    }).unwrap_or(u32::MAX)
}

/// SetBkColor — set the background color for a DC.
pub fn set_bk_color(hdc: Hdc, color: u32) -> u32 {
    with_dc_mut(hdc, |dc| {
        let prev = dc.bk_color;
        dc.bk_color = color;
        prev
    }).unwrap_or(u32::MAX)
}

/// SetBkMode — set background mix mode (TRANSPARENT=1, OPAQUE=2).
pub fn set_bk_mode(hdc: Hdc, mode: i32) -> i32 {
    with_dc_mut(hdc, |dc| {
        let prev = dc.bk_mode;
        dc.bk_mode = mode;
        prev
    }).unwrap_or(0)
}
