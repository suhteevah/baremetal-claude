//! Direct2D hardware-accelerated 2D rendering API implementation.
//!
//! Maps ID2D1Factory, ID2D1RenderTarget, brushes, and drawing primitives
//! to framebuffer back-buffer rendering (software rasterization).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::ole32::{self, HResult, S_OK, E_INVALIDARG, E_FAIL, E_OUTOFMEMORY};
use crate::unicode::*;

// =============================================================================
// GUIDs
// =============================================================================

/// CLSID_D2D1Factory
pub const CLSID_D2D1_FACTORY: Guid = Guid {
    data1: 0x06152247,
    data2: 0x6F50,
    data3: 0x465A,
    data4: [0x92, 0x45, 0x11, 0x8B, 0xFD, 0x3B, 0x60, 0x07],
};

/// IID_ID2D1Factory
pub const IID_ID2D1_FACTORY: Guid = Guid {
    data1: 0x06152247,
    data2: 0x6F50,
    data3: 0x465A,
    data4: [0x92, 0x45, 0x11, 0x8B, 0xFD, 0x3B, 0x60, 0x07],
};

// =============================================================================
// Structures
// =============================================================================

/// D2D1_FACTORY_TYPE
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D2D1FactoryType {
    SingleThreaded = 0,
    MultiThreaded = 1,
}

/// D2D1_RENDER_TARGET_TYPE
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderTargetType {
    Default = 0,
    Software = 1,
    Hardware = 2,
}

/// D2D1_POINT_2F
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Point2F {
    pub x: f32,
    pub y: f32,
}

/// D2D1_SIZE_F
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SizeF {
    pub width: f32,
    pub height: f32,
}

/// D2D1_SIZE_U
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SizeU {
    pub width: u32,
    pub height: u32,
}

/// D2D1_RECT_F
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct RectF {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// D2D1_ROUNDED_RECT
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct RoundedRect {
    pub rect: RectF,
    pub radius_x: f32,
    pub radius_y: f32,
}

/// D2D1_ELLIPSE
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Ellipse {
    pub point: Point2F,
    pub radius_x: f32,
    pub radius_y: f32,
}

/// D2D1_COLOR_F (RGBA float color)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ColorF {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl ColorF {
    pub fn to_bgra32(&self) -> u32 {
        let r = (self.r.clamp(0.0, 1.0) * 255.0) as u32;
        let g = (self.g.clamp(0.0, 1.0) * 255.0) as u32;
        let b = (self.b.clamp(0.0, 1.0) * 255.0) as u32;
        let a = (self.a.clamp(0.0, 1.0) * 255.0) as u32;
        (a << 24) | (r << 16) | (g << 8) | b
    }
}

/// D2D1_PIXEL_FORMAT
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PixelFormat {
    pub format: u32,       // DXGI_FORMAT
    pub alpha_mode: u32,   // D2D1_ALPHA_MODE
}

/// D2D1_RENDER_TARGET_PROPERTIES
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RenderTargetProperties {
    pub render_target_type: u32,
    pub pixel_format: PixelFormat,
    pub dpi_x: f32,
    pub dpi_y: f32,
    pub usage: u32,
    pub min_level: u32,
}

/// D2D1_HWND_RENDER_TARGET_PROPERTIES
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HwndRenderTargetProperties {
    pub hwnd: HWnd,
    pub pixel_size: SizeU,
    pub present_options: u32,
}

/// D2D1_GRADIENT_STOP
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GradientStop {
    pub position: f32,
    pub color: ColorF,
}

/// D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct LinearGradientBrushProperties {
    pub start_point: Point2F,
    pub end_point: Point2F,
}

/// D2D1_BITMAP_PROPERTIES
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct BitmapProperties {
    pub pixel_format: PixelFormat,
    pub dpi_x: f32,
    pub dpi_y: f32,
}

// =============================================================================
// Internal state
// =============================================================================

/// Render target back buffer.
struct RenderTargetState {
    hwnd: HWnd,
    width: u32,
    height: u32,
    pixels: Vec<u32>,
    drawing: bool,
}

/// Brush state.
#[derive(Clone)]
enum BrushState {
    Solid { color: ColorF },
    LinearGradient {
        start: Point2F,
        end: Point2F,
        stops: Vec<GradientStop>,
    },
}

/// Bitmap state.
struct D2DBitmapState {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
}

static RT_TABLE: Mutex<Option<BTreeMap<u64, RenderTargetState>>> = Mutex::new(None);
static NEXT_RT: Mutex<u64> = Mutex::new(0xD2D0_0000);

static BRUSH_TABLE: Mutex<Option<BTreeMap<u64, BrushState>>> = Mutex::new(None);
static NEXT_BRUSH: Mutex<u64> = Mutex::new(0xD2D1_0000);

static D2D_BITMAP_TABLE: Mutex<Option<BTreeMap<u64, D2DBitmapState>>> = Mutex::new(None);
static NEXT_D2D_BITMAP: Mutex<u64> = Mutex::new(0xD2D2_0000);

static FACTORY_TABLE: Mutex<Option<BTreeMap<u64, bool>>> = Mutex::new(None);
static NEXT_FACTORY: Mutex<u64> = Mutex::new(0xD2DF_0000);

fn alloc_rt(state: RenderTargetState) -> u64 {
    let mut counter = NEXT_RT.lock();
    let handle = *counter;
    *counter += 1;
    let mut table = RT_TABLE.lock();
    if table.is_none() { *table = Some(BTreeMap::new()); }
    if let Some(ref mut map) = *table { map.insert(handle, state); }
    handle
}

fn with_rt_mut<F, R>(handle: u64, f: F) -> Option<R>
where F: FnOnce(&mut RenderTargetState) -> R {
    let mut table = RT_TABLE.lock();
    table.as_mut().and_then(|map| map.get_mut(&handle).map(f))
}

fn with_rt<F, R>(handle: u64, f: F) -> Option<R>
where F: FnOnce(&RenderTargetState) -> R {
    let table = RT_TABLE.lock();
    table.as_ref().and_then(|map| map.get(&handle).map(f))
}

fn alloc_brush(state: BrushState) -> u64 {
    let mut counter = NEXT_BRUSH.lock();
    let handle = *counter;
    *counter += 1;
    let mut table = BRUSH_TABLE.lock();
    if table.is_none() { *table = Some(BTreeMap::new()); }
    if let Some(ref mut map) = *table { map.insert(handle, state); }
    handle
}

fn get_brush_color(handle: u64) -> u32 {
    let table = BRUSH_TABLE.lock();
    if let Some(brush) = table.as_ref().and_then(|m| m.get(&handle)) {
        match brush {
            BrushState::Solid { color } => color.to_bgra32(),
            BrushState::LinearGradient { stops, .. } => {
                // Use first stop color as fallback
                stops.first().map(|s| s.color.to_bgra32()).unwrap_or(0xFF000000)
            }
        }
    } else {
        0xFF000000 // Default black
    }
}

/// Sample a linear gradient at position t (0.0 to 1.0).
fn sample_gradient(stops: &[GradientStop], t: f32) -> u32 {
    if stops.is_empty() { return 0xFF000000; }
    if stops.len() == 1 { return stops[0].color.to_bgra32(); }

    let t = t.clamp(0.0, 1.0);

    // Find the two surrounding stops
    let mut prev = &stops[0];
    for stop in &stops[1..] {
        if t <= stop.position {
            let range = stop.position - prev.position;
            let local_t = if range > 0.0 { (t - prev.position) / range } else { 0.0 };
            let color = ColorF {
                r: prev.color.r + (stop.color.r - prev.color.r) * local_t,
                g: prev.color.g + (stop.color.g - prev.color.g) * local_t,
                b: prev.color.b + (stop.color.b - prev.color.b) * local_t,
                a: prev.color.a + (stop.color.a - prev.color.a) * local_t,
            };
            return color.to_bgra32();
        }
        prev = stop;
    }
    stops.last().unwrap().color.to_bgra32()
}

// =============================================================================
// Public API — Factory
// =============================================================================

/// D2D1CreateFactory — create a Direct2D factory.
pub fn d2d1_create_factory(
    factory_type: u32,
    riid: *const Guid,
    factory_options: *const core::ffi::c_void,
    factory: *mut *mut core::ffi::c_void,
) -> HResult {
    if factory.is_null() { return E_INVALIDARG; }

    log::info!("[direct2d] D2D1CreateFactory: type={}", factory_type);

    let handle = {
        let mut counter = NEXT_FACTORY.lock();
        let h = *counter;
        *counter += 1;
        h
    };

    {
        let mut table = FACTORY_TABLE.lock();
        if table.is_none() { *table = Some(BTreeMap::new()); }
        if let Some(ref mut map) = *table { map.insert(handle, true); }
    }

    // Return the handle as a pseudo COM pointer
    unsafe { *factory = handle as *mut core::ffi::c_void; }
    S_OK
}

// =============================================================================
// Public API — Render Target
// =============================================================================

/// CreateHwndRenderTarget — create a render target for a window.
pub fn create_hwnd_render_target(
    factory: u64,
    rt_props: *const RenderTargetProperties,
    hwnd_props: *const HwndRenderTargetProperties,
    render_target: *mut u64,
) -> HResult {
    if render_target.is_null() || hwnd_props.is_null() { return E_INVALIDARG; }

    let (hwnd, w, h) = unsafe {
        let props = &*hwnd_props;
        (props.hwnd, props.pixel_size.width.max(1), props.pixel_size.height.max(1))
    };

    log::debug!("[direct2d] CreateHwndRenderTarget: hwnd=0x{:X} size={}x{}", hwnd, w, h);

    let handle = alloc_rt(RenderTargetState {
        hwnd,
        width: w,
        height: h,
        pixels: vec![0u32; (w * h) as usize],
        drawing: false,
    });

    unsafe { *render_target = handle; }
    S_OK
}

/// BeginDraw — start a drawing batch on a render target.
pub fn begin_draw(render_target: u64) {
    with_rt_mut(render_target, |rt| {
        rt.drawing = true;
        log::trace!("[direct2d] BeginDraw: rt=0x{:X}", render_target);
    });
}

/// EndDraw — finish a drawing batch. Returns S_OK on success.
pub fn end_draw(render_target: u64, tag1: *mut u64, tag2: *mut u64) -> HResult {
    with_rt_mut(render_target, |rt| {
        rt.drawing = false;
        log::trace!("[direct2d] EndDraw: rt=0x{:X}", render_target);
    });
    if !tag1.is_null() { unsafe { *tag1 = 0; } }
    if !tag2.is_null() { unsafe { *tag2 = 0; } }
    S_OK
}

/// Clear — clear the render target to a color.
pub fn clear(render_target: u64, color: *const ColorF) {
    let fill = if !color.is_null() {
        unsafe { (*color).to_bgra32() }
    } else {
        0x00000000
    };

    with_rt_mut(render_target, |rt| {
        for pixel in rt.pixels.iter_mut() {
            *pixel = fill;
        }
    });
}

// =============================================================================
// Public API — Brushes
// =============================================================================

/// CreateSolidColorBrush — create a solid color brush.
pub fn create_solid_color_brush(
    render_target: u64,
    color: *const ColorF,
    _properties: *const core::ffi::c_void,
    brush: *mut u64,
) -> HResult {
    if brush.is_null() || color.is_null() { return E_INVALIDARG; }

    let c = unsafe { *color };
    log::trace!("[direct2d] CreateSolidColorBrush: rgba=({},{},{},{})", c.r, c.g, c.b, c.a);

    let handle = alloc_brush(BrushState::Solid { color: c });
    unsafe { *brush = handle; }
    S_OK
}

/// CreateLinearGradientBrush — create a linear gradient brush.
pub fn create_linear_gradient_brush(
    render_target: u64,
    properties: *const LinearGradientBrushProperties,
    _brush_properties: *const core::ffi::c_void,
    gradient_stops: *const GradientStop,
    stop_count: u32,
    brush: *mut u64,
) -> HResult {
    if brush.is_null() || properties.is_null() { return E_INVALIDARG; }

    let props = unsafe { &*properties };
    let stops = if !gradient_stops.is_null() && stop_count > 0 {
        unsafe { core::slice::from_raw_parts(gradient_stops, stop_count as usize).to_vec() }
    } else {
        Vec::new()
    };

    log::trace!(
        "[direct2d] CreateLinearGradientBrush: ({},{})->({},{}) {} stops",
        props.start_point.x, props.start_point.y,
        props.end_point.x, props.end_point.y,
        stops.len()
    );

    let handle = alloc_brush(BrushState::LinearGradient {
        start: props.start_point,
        end: props.end_point,
        stops,
    });
    unsafe { *brush = handle; }
    S_OK
}

// =============================================================================
// Public API — Drawing primitives
// =============================================================================

/// DrawRectangle — draw a rectangle outline.
pub fn draw_rectangle(render_target: u64, rect: *const RectF, brush: u64, stroke_width: f32) {
    if rect.is_null() { return; }
    let r = unsafe { *rect };
    let color = get_brush_color(brush);
    let sw = stroke_width.max(1.0) as i32;

    with_rt_mut(render_target, |rt| {
        let left = r.left as i32;
        let top = r.top as i32;
        let right = r.right as i32;
        let bottom = r.bottom as i32;

        // Top and bottom edges
        for x in left..right {
            for s in 0..sw {
                set_pixel_safe(rt, x as u32, (top + s) as u32, color);
                set_pixel_safe(rt, x as u32, (bottom - 1 - s) as u32, color);
            }
        }
        // Left and right edges
        for y in top..bottom {
            for s in 0..sw {
                set_pixel_safe(rt, (left + s) as u32, y as u32, color);
                set_pixel_safe(rt, (right - 1 - s) as u32, y as u32, color);
            }
        }
    });
}

/// FillRectangle — fill a rectangle.
pub fn fill_rectangle(render_target: u64, rect: *const RectF, brush: u64) {
    if rect.is_null() { return; }
    let r = unsafe { *rect };
    let color = get_brush_color(brush);

    with_rt_mut(render_target, |rt| {
        for y in (r.top as i32)..(r.bottom as i32) {
            for x in (r.left as i32)..(r.right as i32) {
                set_pixel_safe(rt, x as u32, y as u32, color);
            }
        }
    });
}

/// DrawEllipse — draw an ellipse outline.
pub fn draw_ellipse(render_target: u64, ellipse: *const Ellipse, brush: u64, stroke_width: f32) {
    if ellipse.is_null() { return; }
    let e = unsafe { *ellipse };
    let color = get_brush_color(brush);

    with_rt_mut(render_target, |rt| {
        // Midpoint ellipse algorithm (approximate with filled boundary)
        let cx = e.point.x;
        let cy = e.point.y;
        let rx = e.radius_x;
        let ry = e.radius_y;
        let steps = ((rx + ry) * 4.0) as i32;

        for i in 0..steps {
            let angle = (i as f32 / steps as f32) * 2.0 * core::f32::consts::PI;
            let x = cx + rx * cos_approx(angle);
            let y = cy + ry * sin_approx(angle);
            set_pixel_safe(rt, x as u32, y as u32, color);
        }
    });
}

/// FillEllipse — fill an ellipse.
pub fn fill_ellipse(render_target: u64, ellipse: *const Ellipse, brush: u64) {
    if ellipse.is_null() { return; }
    let e = unsafe { *ellipse };
    let color = get_brush_color(brush);

    with_rt_mut(render_target, |rt| {
        let cx = e.point.x;
        let cy = e.point.y;
        let rx = e.radius_x;
        let ry = e.radius_y;

        let top = (cy - ry) as i32;
        let bottom = (cy + ry) as i32;

        for y in top..=bottom {
            let dy = (y as f32 - cy) / ry;
            if dy * dy <= 1.0 {
                let half_width = rx * sqrt_approx(1.0 - dy * dy);
                let left = (cx - half_width) as i32;
                let right = (cx + half_width) as i32;
                for x in left..=right {
                    set_pixel_safe(rt, x as u32, y as u32, color);
                }
            }
        }
    });
}

/// DrawLine — draw a line between two points.
pub fn draw_line(render_target: u64, p0: Point2F, p1: Point2F, brush: u64, stroke_width: f32) {
    let color = get_brush_color(brush);

    with_rt_mut(render_target, |rt| {
        // Bresenham's line
        let x0 = p0.x as i32;
        let y0 = p0.y as i32;
        let x1 = p1.x as i32;
        let y1 = p1.y as i32;

        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx: i32 = if x0 < x1 { 1 } else { -1 };
        let sy: i32 = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let mut cx = x0;
        let mut cy = y0;

        loop {
            set_pixel_safe(rt, cx as u32, cy as u32, color);
            if cx == x1 && cy == y1 { break; }
            let e2 = 2 * err;
            if e2 >= dy { err += dy; cx += sx; }
            if e2 <= dx { err += dx; cy += sy; }
        }
    });
}

/// DrawText — draw text using a DirectWrite text format.
pub fn draw_text(
    render_target: u64,
    string: LpcWStr,
    string_length: u32,
    text_format: u64,
    layout_rect: *const RectF,
    brush: u64,
) {
    if string.is_null() || layout_rect.is_null() { return; }

    let text = unsafe {
        let slice = core::slice::from_raw_parts(string, string_length as usize);
        crate::unicode::utf16_slice_to_string(slice)
    };
    let rect = unsafe { *layout_rect };
    let color = get_brush_color(brush);

    log::trace!("[direct2d] DrawText: '{}' at ({},{})", text, rect.left, rect.top);

    // Render as 8x16 placeholder glyphs (same as gdi32 TextOutW)
    with_rt_mut(render_target, |rt| {
        let mut cx = rect.left as i32;
        let cy = rect.top as i32;
        for _ch in text.chars() {
            for dy in 0..16 {
                for dx in 0..8 {
                    set_pixel_safe(rt, (cx + dx) as u32, (cy + dy) as u32, color);
                }
            }
            cx += 8;
            if cx as f32 >= rect.right { break; }
        }
    });
}

// =============================================================================
// Public API — Bitmap
// =============================================================================

/// CreateBitmapFromWicBitmap — create a D2D bitmap from a WIC bitmap source.
pub fn create_bitmap_from_wic_bitmap(
    render_target: u64,
    wic_source: u64,
    properties: *const BitmapProperties,
    bitmap: *mut u64,
) -> HResult {
    if bitmap.is_null() { return E_INVALIDARG; }

    log::debug!("[direct2d] CreateBitmapFromWicBitmap: wic_source=0x{:X}", wic_source);

    // In a full implementation, we'd read pixel data from the WIC bitmap.
    // For now, create a 1x1 placeholder.
    let mut counter = NEXT_D2D_BITMAP.lock();
    let handle = *counter;
    *counter += 1;

    let mut table = D2D_BITMAP_TABLE.lock();
    if table.is_none() { *table = Some(BTreeMap::new()); }
    if let Some(ref mut map) = *table {
        map.insert(handle, D2DBitmapState {
            width: 1,
            height: 1,
            pixels: vec![0xFFFFFFFF],
        });
    }

    unsafe { *bitmap = handle; }
    S_OK
}

/// DrawBitmap — draw a bitmap to the render target.
pub fn draw_bitmap(
    render_target: u64,
    d2d_bitmap: u64,
    dest_rect: *const RectF,
    opacity: f32,
    _interpolation: u32,
    source_rect: *const RectF,
) {
    log::trace!("[direct2d] DrawBitmap: bitmap=0x{:X}", d2d_bitmap);

    // Read bitmap pixels
    let bmp_data: Option<(Vec<u32>, u32, u32)> = {
        let table = D2D_BITMAP_TABLE.lock();
        table.as_ref().and_then(|m| m.get(&d2d_bitmap)).map(|b| {
            (b.pixels.clone(), b.width, b.height)
        })
    };

    if let Some((pixels, bw, bh)) = bmp_data {
        if let Some(dr) = if !dest_rect.is_null() { Some(unsafe { *dest_rect }) } else { None } {
            with_rt_mut(render_target, |rt| {
                let dw = (dr.right - dr.left) as u32;
                let dh = (dr.bottom - dr.top) as u32;
                if dw == 0 || dh == 0 { return; }

                for dy in 0..dh {
                    for dx in 0..dw {
                        let sx = (dx * bw / dw).min(bw - 1);
                        let sy = (dy * bh / dh).min(bh - 1);
                        let src_idx = (sy * bw + sx) as usize;
                        if src_idx < pixels.len() {
                            let px = (dr.left as u32 + dx) as u32;
                            let py = (dr.top as u32 + dy) as u32;
                            set_pixel_safe(rt, px, py, pixels[src_idx]);
                        }
                    }
                }
            });
        }
    }
}

/// Resize — resize the render target.
pub fn resize(render_target: u64, size: *const SizeU) -> HResult {
    if size.is_null() { return E_INVALIDARG; }
    let s = unsafe { *size };

    with_rt_mut(render_target, |rt| {
        rt.width = s.width.max(1);
        rt.height = s.height.max(1);
        rt.pixels = vec![0u32; (rt.width * rt.height) as usize];
        log::debug!("[direct2d] Resize: {}x{}", rt.width, rt.height);
    });
    S_OK
}

/// GetSize — get the render target dimensions.
pub fn get_size(render_target: u64) -> SizeF {
    with_rt(render_target, |rt| SizeF {
        width: rt.width as f32,
        height: rt.height as f32,
    }).unwrap_or(SizeF { width: 0.0, height: 0.0 })
}

// =============================================================================
// Helpers
// =============================================================================

#[inline]
fn set_pixel_safe(rt: &mut RenderTargetState, x: u32, y: u32, color: u32) {
    if x < rt.width && y < rt.height {
        rt.pixels[(y * rt.width + x) as usize] = color;
    }
}

/// Approximate sine for drawing (no libm dependency needed inline).
fn sin_approx(x: f32) -> f32 {
    // Normalize to -PI..PI
    let mut x = x;
    while x > core::f32::consts::PI { x -= 2.0 * core::f32::consts::PI; }
    while x < -core::f32::consts::PI { x += 2.0 * core::f32::consts::PI; }
    // Bhaskara I approximation
    let abs_x = if x < 0.0 { -x } else { x };
    let y = (16.0 * abs_x * (core::f32::consts::PI - abs_x))
        / (5.0 * core::f32::consts::PI * core::f32::consts::PI - 4.0 * abs_x * (core::f32::consts::PI - abs_x));
    if x < 0.0 { -y } else { y }
}

fn cos_approx(x: f32) -> f32 {
    sin_approx(x + core::f32::consts::FRAC_PI_2)
}

/// Approximate square root using Newton's method (no libm needed).
fn sqrt_approx(x: f32) -> f32 {
    if x <= 0.0 { return 0.0; }
    // Initial guess using bit manipulation
    let mut guess = x * 0.5;
    // 4 Newton iterations
    for _ in 0..4 {
        guess = 0.5 * (guess + x / guess);
    }
    guess
}
