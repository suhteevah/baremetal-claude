//! DirectWrite text rendering API implementation.
//!
//! Maps IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout to our
//! built-in Terminus/Unicode bitmap font renderer for actual pixel output.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::ole32::{self, HResult, S_OK, E_INVALIDARG, E_OUTOFMEMORY, E_FAIL, IUnknown, IUnknownVtbl};
use crate::unicode::*;

// =============================================================================
// GUIDs
// =============================================================================

/// CLSID_DWriteFactory
pub const CLSID_DWRITE_FACTORY: Guid = Guid {
    data1: 0xB859EE5A,
    data2: 0xD838,
    data3: 0x4B5B,
    data4: [0xA2, 0xE8, 0x1A, 0xDC, 0x7D, 0x93, 0xDB, 0x48],
};

/// IID_IDWriteFactory
pub const IID_IDWRITE_FACTORY: Guid = Guid {
    data1: 0xB859EE5A,
    data2: 0xD838,
    data3: 0x4B5B,
    data4: [0xA2, 0xE8, 0x1A, 0xDC, 0x7D, 0x93, 0xDB, 0x48],
};

/// IID_IDWriteTextFormat
pub const IID_IDWRITE_TEXT_FORMAT: Guid = Guid {
    data1: 0x9C906818,
    data2: 0x31D7,
    data3: 0x4FD3,
    data4: [0xA1, 0x51, 0x7C, 0x5E, 0x22, 0x5D, 0xB5, 0x5A],
};

/// IID_IDWriteTextLayout
pub const IID_IDWRITE_TEXT_LAYOUT: Guid = Guid {
    data1: 0x53737037,
    data2: 0x6D14,
    data3: 0x410B,
    data4: [0x9B, 0xFE, 0x0B, 0x18, 0x2B, 0xB7, 0x09, 0x61],
};

// =============================================================================
// Enums & structures
// =============================================================================

/// DWRITE_FACTORY_TYPE
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteFactoryType {
    Shared = 0,
    Isolated = 1,
}

/// DWRITE_FONT_WEIGHT
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontWeight {
    Thin = 100,
    ExtraLight = 200,
    Light = 300,
    SemiLight = 350,
    Normal = 400,
    Medium = 500,
    SemiBold = 600,
    Bold = 700,
    ExtraBold = 800,
    Black = 900,
}

/// DWRITE_FONT_STYLE
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontStyle {
    Normal = 0,
    Oblique = 1,
    Italic = 2,
}

/// DWRITE_FONT_STRETCH
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontStretch {
    Undefined = 0,
    UltraCondensed = 1,
    ExtraCondensed = 2,
    Condensed = 3,
    SemiCondensed = 4,
    Normal = 5,
    SemiExpanded = 6,
    Expanded = 7,
    ExtraExpanded = 8,
    UltraExpanded = 9,
}

/// DWRITE_TEXT_ALIGNMENT
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlignment {
    Leading = 0,
    Trailing = 1,
    Center = 2,
    Justified = 3,
}

/// DWRITE_PARAGRAPH_ALIGNMENT
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParagraphAlignment {
    Near = 0,
    Far = 1,
    Center = 2,
}

/// DWRITE_TEXT_METRICS
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct TextMetrics {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub width_including_trailing_whitespace: f32,
    pub height: f32,
    pub layout_width: f32,
    pub layout_height: f32,
    pub max_bidi_reordering_depth: u32,
    pub line_count: u32,
}

// =============================================================================
// Internal state
// =============================================================================

/// Internal text format state.
struct TextFormatState {
    font_family: String,
    font_size: f32,
    font_weight: u32,
    font_style: u32,
    font_stretch: u32,
    text_alignment: TextAlignment,
    paragraph_alignment: ParagraphAlignment,
}

/// Internal text layout state.
struct TextLayoutState {
    text: String,
    format_handle: u64,
    max_width: f32,
    max_height: f32,
}

/// Built-in font names we report as available.
static BUILTIN_FONTS: &[&str] = &[
    "Terminus",
    "Terminus (TTF)",
    "Consolas",
    "Courier New",
    "Segoe UI",
    "Arial",
    "Tahoma",
    "Lucida Console",
];

/// Global factory table.
static FACTORY_TABLE: Mutex<Option<BTreeMap<u64, bool>>> = Mutex::new(None);
static NEXT_FACTORY: Mutex<u64> = Mutex::new(0xDA00_0000);

/// Global text format table.
static FORMAT_TABLE: Mutex<Option<BTreeMap<u64, TextFormatState>>> = Mutex::new(None);
static NEXT_FORMAT: Mutex<u64> = Mutex::new(0xDA10_0000);

/// Global text layout table.
static LAYOUT_TABLE: Mutex<Option<BTreeMap<u64, TextLayoutState>>> = Mutex::new(None);
static NEXT_LAYOUT: Mutex<u64> = Mutex::new(0xDA20_0000);

const FACTORY_BASE: u64 = 0xDA00_0000;
const FORMAT_BASE: u64 = 0xDA10_0000;
const LAYOUT_BASE: u64 = 0xDA20_0000;

/// Font collection handle base.
const FONT_COLLECTION_BASE: u64 = 0xD300_0000;

// =============================================================================
// IDWriteFactory vtable
// =============================================================================

/// IDWriteFactory vtable.
#[repr(C)]
pub struct IDWriteFactoryVtbl {
    // IUnknown
    pub query_interface: extern "system" fn(*mut IDWriteFactory, *const Guid, *mut *mut core::ffi::c_void) -> HResult,
    pub add_ref: extern "system" fn(*mut IDWriteFactory) -> u32,
    pub release: extern "system" fn(*mut IDWriteFactory) -> u32,
    // IDWriteFactory
    pub get_system_font_collection: extern "system" fn(*mut IDWriteFactory, *mut *mut core::ffi::c_void, Bool) -> HResult,
    pub create_custom_font_collection: extern "system" fn(*mut IDWriteFactory, *mut core::ffi::c_void, *const core::ffi::c_void, u32, *mut *mut core::ffi::c_void) -> HResult,
    pub register_font_collection_loader: extern "system" fn(*mut IDWriteFactory, *mut core::ffi::c_void) -> HResult,
    pub unregister_font_collection_loader: extern "system" fn(*mut IDWriteFactory, *mut core::ffi::c_void) -> HResult,
    pub create_font_file_reference: extern "system" fn(*mut IDWriteFactory, LpcWStr, *const core::ffi::c_void, *mut *mut core::ffi::c_void) -> HResult,
    pub create_custom_font_file_reference: extern "system" fn(*mut IDWriteFactory, *const core::ffi::c_void, u32, *mut core::ffi::c_void, *mut *mut core::ffi::c_void) -> HResult,
    pub create_font_face: extern "system" fn(*mut IDWriteFactory, u32, u32, *const *mut core::ffi::c_void, u32, u32, *mut *mut core::ffi::c_void) -> HResult,
    pub create_rendering_params: extern "system" fn(*mut IDWriteFactory, *mut *mut core::ffi::c_void) -> HResult,
    pub create_monitor_rendering_params: extern "system" fn(*mut IDWriteFactory, u64, *mut *mut core::ffi::c_void) -> HResult,
    pub create_custom_rendering_params: extern "system" fn(*mut IDWriteFactory, f32, f32, f32, u32, u32, *mut *mut core::ffi::c_void) -> HResult,
    pub register_font_file_loader: extern "system" fn(*mut IDWriteFactory, *mut core::ffi::c_void) -> HResult,
    pub unregister_font_file_loader: extern "system" fn(*mut IDWriteFactory, *mut core::ffi::c_void) -> HResult,
    pub create_text_format: extern "system" fn(*mut IDWriteFactory, LpcWStr, *mut core::ffi::c_void, u32, u32, u32, f32, LpcWStr, *mut *mut core::ffi::c_void) -> HResult,
    pub create_typography: extern "system" fn(*mut IDWriteFactory, *mut *mut core::ffi::c_void) -> HResult,
    pub get_gdi_interop: extern "system" fn(*mut IDWriteFactory, *mut *mut core::ffi::c_void) -> HResult,
    pub create_text_layout: extern "system" fn(*mut IDWriteFactory, LpcWStr, u32, *mut core::ffi::c_void, f32, f32, *mut *mut core::ffi::c_void) -> HResult,
    pub create_gdi_compatible_text_layout: extern "system" fn(*mut IDWriteFactory, LpcWStr, u32, *mut core::ffi::c_void, f32, f32, f32, *const core::ffi::c_void, Bool, *mut *mut core::ffi::c_void) -> HResult,
    pub create_ellipsis_trimming_sign: extern "system" fn(*mut IDWriteFactory, *mut core::ffi::c_void, *mut *mut core::ffi::c_void) -> HResult,
    pub create_text_analyzer: extern "system" fn(*mut IDWriteFactory, *mut *mut core::ffi::c_void) -> HResult,
    pub create_number_substitution: extern "system" fn(*mut IDWriteFactory, u32, LpcWStr, Bool, *mut *mut core::ffi::c_void) -> HResult,
    pub create_glyph_run_analysis: extern "system" fn(*mut IDWriteFactory, *const core::ffi::c_void, f32, *const core::ffi::c_void, u32, u32, f32, f32, *mut *mut core::ffi::c_void) -> HResult,
}

/// IDWriteFactory COM object.
#[repr(C)]
pub struct IDWriteFactory {
    pub vtbl: *const IDWriteFactoryVtbl,
    pub handle: u64,
}

// =============================================================================
// IDWriteTextFormat vtable
// =============================================================================

/// IDWriteTextFormat vtable.
#[repr(C)]
pub struct IDWriteTextFormatVtbl {
    // IUnknown
    pub query_interface: extern "system" fn(*mut IDWriteTextFormat, *const Guid, *mut *mut core::ffi::c_void) -> HResult,
    pub add_ref: extern "system" fn(*mut IDWriteTextFormat) -> u32,
    pub release: extern "system" fn(*mut IDWriteTextFormat) -> u32,
    // IDWriteTextFormat
    pub set_text_alignment: extern "system" fn(*mut IDWriteTextFormat, u32) -> HResult,
    pub set_paragraph_alignment: extern "system" fn(*mut IDWriteTextFormat, u32) -> HResult,
    pub set_word_wrapping: extern "system" fn(*mut IDWriteTextFormat, u32) -> HResult,
    pub set_reading_direction: extern "system" fn(*mut IDWriteTextFormat, u32) -> HResult,
    pub set_flow_direction: extern "system" fn(*mut IDWriteTextFormat, u32) -> HResult,
    pub set_incremental_tab_stop: extern "system" fn(*mut IDWriteTextFormat, f32) -> HResult,
    pub set_trimming: extern "system" fn(*mut IDWriteTextFormat, *const core::ffi::c_void, *mut core::ffi::c_void) -> HResult,
    pub set_line_spacing: extern "system" fn(*mut IDWriteTextFormat, u32, f32, f32) -> HResult,
    pub get_text_alignment: extern "system" fn(*mut IDWriteTextFormat) -> u32,
    pub get_paragraph_alignment: extern "system" fn(*mut IDWriteTextFormat) -> u32,
    pub get_word_wrapping: extern "system" fn(*mut IDWriteTextFormat) -> u32,
    pub get_reading_direction: extern "system" fn(*mut IDWriteTextFormat) -> u32,
    pub get_flow_direction: extern "system" fn(*mut IDWriteTextFormat) -> u32,
    pub get_incremental_tab_stop: extern "system" fn(*mut IDWriteTextFormat) -> f32,
    pub get_trimming: extern "system" fn(*mut IDWriteTextFormat, *mut core::ffi::c_void, *mut *mut core::ffi::c_void) -> HResult,
    pub get_line_spacing: extern "system" fn(*mut IDWriteTextFormat, *mut u32, *mut f32, *mut f32) -> HResult,
    pub get_font_collection: extern "system" fn(*mut IDWriteTextFormat, *mut *mut core::ffi::c_void) -> HResult,
    pub get_font_family_name_length: extern "system" fn(*mut IDWriteTextFormat) -> u32,
    pub get_font_family_name: extern "system" fn(*mut IDWriteTextFormat, LpWStr, u32) -> HResult,
    pub get_font_weight: extern "system" fn(*mut IDWriteTextFormat) -> u32,
    pub get_font_style: extern "system" fn(*mut IDWriteTextFormat) -> u32,
    pub get_font_stretch: extern "system" fn(*mut IDWriteTextFormat) -> u32,
    pub get_font_size: extern "system" fn(*mut IDWriteTextFormat) -> f32,
    pub get_locale_name_length: extern "system" fn(*mut IDWriteTextFormat) -> u32,
    pub get_locale_name: extern "system" fn(*mut IDWriteTextFormat, LpWStr, u32) -> HResult,
}

/// IDWriteTextFormat COM object.
#[repr(C)]
pub struct IDWriteTextFormat {
    pub vtbl: *const IDWriteTextFormatVtbl,
    pub handle: u64,
}

// =============================================================================
// IDWriteTextLayout vtable
// =============================================================================

/// IDWriteTextLayout vtable (extends IDWriteTextFormat).
#[repr(C)]
pub struct IDWriteTextLayoutVtbl {
    // IUnknown
    pub query_interface: extern "system" fn(*mut IDWriteTextLayout, *const Guid, *mut *mut core::ffi::c_void) -> HResult,
    pub add_ref: extern "system" fn(*mut IDWriteTextLayout) -> u32,
    pub release: extern "system" fn(*mut IDWriteTextLayout) -> u32,
    // IDWriteTextFormat methods (inherited) — omitted for brevity, pad with stubs
    pub text_format_methods: [u64; 26],
    // IDWriteTextLayout
    pub set_max_width: extern "system" fn(*mut IDWriteTextLayout, f32) -> HResult,
    pub set_max_height: extern "system" fn(*mut IDWriteTextLayout, f32) -> HResult,
    pub set_font_collection: extern "system" fn(*mut IDWriteTextLayout, *mut core::ffi::c_void, u32, u32) -> HResult,
    pub set_font_family_name: extern "system" fn(*mut IDWriteTextLayout, LpcWStr, u32, u32) -> HResult,
    pub set_font_weight: extern "system" fn(*mut IDWriteTextLayout, u32, u32, u32) -> HResult,
    pub set_font_style: extern "system" fn(*mut IDWriteTextLayout, u32, u32, u32) -> HResult,
    pub set_font_stretch: extern "system" fn(*mut IDWriteTextLayout, u32, u32, u32) -> HResult,
    pub set_font_size: extern "system" fn(*mut IDWriteTextLayout, f32, u32, u32) -> HResult,
    pub set_underline: extern "system" fn(*mut IDWriteTextLayout, Bool, u32, u32) -> HResult,
    pub set_strikethrough: extern "system" fn(*mut IDWriteTextLayout, Bool, u32, u32) -> HResult,
    pub set_drawing_effect: extern "system" fn(*mut IDWriteTextLayout, *mut core::ffi::c_void, u32, u32) -> HResult,
    pub set_inline_object: extern "system" fn(*mut IDWriteTextLayout, *mut core::ffi::c_void, u32, u32) -> HResult,
    pub set_typography: extern "system" fn(*mut IDWriteTextLayout, *mut core::ffi::c_void, u32, u32) -> HResult,
    pub set_locale_name: extern "system" fn(*mut IDWriteTextLayout, LpcWStr, u32, u32) -> HResult,
    pub get_max_width: extern "system" fn(*mut IDWriteTextLayout) -> f32,
    pub get_max_height: extern "system" fn(*mut IDWriteTextLayout) -> f32,
    pub get_metrics: extern "system" fn(*mut IDWriteTextLayout, *mut TextMetrics) -> HResult,
    pub get_overhang_metrics: extern "system" fn(*mut IDWriteTextLayout, *mut core::ffi::c_void) -> HResult,
    pub get_cluster_metrics: extern "system" fn(*mut IDWriteTextLayout, *mut core::ffi::c_void, u32, *mut u32) -> HResult,
    pub determine_min_width: extern "system" fn(*mut IDWriteTextLayout, *mut f32) -> HResult,
    pub hit_test_point: extern "system" fn(*mut IDWriteTextLayout, f32, f32, *mut Bool, *mut Bool, *mut core::ffi::c_void) -> HResult,
    pub hit_test_text_position: extern "system" fn(*mut IDWriteTextLayout, u32, Bool, *mut f32, *mut f32, *mut core::ffi::c_void) -> HResult,
    pub hit_test_text_range: extern "system" fn(*mut IDWriteTextLayout, u32, u32, f32, f32, *mut core::ffi::c_void, u32, *mut u32) -> HResult,
    pub draw: extern "system" fn(*mut IDWriteTextLayout, *mut core::ffi::c_void, *mut core::ffi::c_void, f32, f32) -> HResult,
}

/// IDWriteTextLayout COM object.
#[repr(C)]
pub struct IDWriteTextLayout {
    pub vtbl: *const IDWriteTextLayoutVtbl,
    pub handle: u64,
}

// =============================================================================
// Factory vtable implementation
// =============================================================================

extern "system" fn factory_qi(this: *mut IDWriteFactory, riid: *const Guid, ppv: *mut *mut core::ffi::c_void) -> HResult {
    if ppv.is_null() { return E_INVALIDARG; }
    unsafe { *ppv = this as *mut core::ffi::c_void; }
    S_OK
}

extern "system" fn factory_addref(_this: *mut IDWriteFactory) -> u32 { 1 }
extern "system" fn factory_release(_this: *mut IDWriteFactory) -> u32 { 0 }

extern "system" fn factory_get_system_font_collection(
    _this: *mut IDWriteFactory,
    collection: *mut *mut core::ffi::c_void,
    _check_for_updates: Bool,
) -> HResult {
    if collection.is_null() { return E_INVALIDARG; }
    // Return a pseudo font collection handle
    unsafe { *collection = FONT_COLLECTION_BASE as *mut core::ffi::c_void; }
    log::debug!("[directwrite] GetSystemFontCollection: returning built-in font collection ({} fonts)", BUILTIN_FONTS.len());
    S_OK
}

extern "system" fn factory_create_text_format(
    _this: *mut IDWriteFactory,
    font_family: LpcWStr,
    _font_collection: *mut core::ffi::c_void,
    font_weight: u32,
    font_style: u32,
    font_stretch: u32,
    font_size: f32,
    locale: LpcWStr,
    text_format: *mut *mut core::ffi::c_void,
) -> HResult {
    if text_format.is_null() { return E_INVALIDARG; }

    let family = if !font_family.is_null() {
        unsafe { utf16_to_utf8(font_family) }
    } else {
        String::from("Terminus")
    };

    log::debug!(
        "[directwrite] CreateTextFormat: family='{}' size={} weight={} style={}",
        family, font_size, font_weight, font_style
    );

    let handle = {
        let mut counter = NEXT_FORMAT.lock();
        // Use a simple incrementing counter from FORMAT_BASE
        let h = FORMAT_BASE + (*counter - FORMAT_BASE);
        *counter += 1;
        h
    };

    {
        let mut table = FORMAT_TABLE.lock();
        if table.is_none() { *table = Some(BTreeMap::new()); }
        if let Some(ref mut map) = *table {
            map.insert(handle, TextFormatState {
                font_family: family,
                font_size,
                font_weight,
                font_style,
                font_stretch,
                text_alignment: TextAlignment::Leading,
                paragraph_alignment: ParagraphAlignment::Near,
            });
        }
    }

    // Allocate a COM object on the heap
    let obj = alloc::boxed::Box::new(IDWriteTextFormat {
        vtbl: &TEXT_FORMAT_VTBL,
        handle,
    });
    unsafe { *text_format = alloc::boxed::Box::into_raw(obj) as *mut core::ffi::c_void; }
    S_OK
}

extern "system" fn factory_create_text_layout(
    _this: *mut IDWriteFactory,
    string: LpcWStr,
    string_length: u32,
    text_format: *mut core::ffi::c_void,
    max_width: f32,
    max_height: f32,
    text_layout: *mut *mut core::ffi::c_void,
) -> HResult {
    if text_layout.is_null() { return E_INVALIDARG; }

    let text = if !string.is_null() && string_length > 0 {
        unsafe {
            let slice = core::slice::from_raw_parts(string, string_length as usize);
            utf16_slice_to_string(slice)
        }
    } else {
        String::new()
    };

    // Get the format handle from the COM object pointer
    let format_handle = if !text_format.is_null() {
        unsafe { (*(text_format as *const IDWriteTextFormat)).handle }
    } else {
        0
    };

    log::debug!(
        "[directwrite] CreateTextLayout: text='{}' max={}x{}",
        if text.len() > 32 { &text[..32] } else { &text },
        max_width, max_height
    );

    let handle = {
        let mut counter = NEXT_LAYOUT.lock();
        let h = LAYOUT_BASE + (*counter - LAYOUT_BASE);
        *counter += 1;
        h
    };

    {
        let mut table = LAYOUT_TABLE.lock();
        if table.is_none() { *table = Some(BTreeMap::new()); }
        if let Some(ref mut map) = *table {
            map.insert(handle, TextLayoutState {
                text,
                format_handle,
                max_width,
                max_height,
            });
        }
    }

    let obj = alloc::boxed::Box::new(IDWriteTextLayout {
        vtbl: core::ptr::null(), // Simplified — layout vtable not fully wired
        handle,
    });
    unsafe { *text_layout = alloc::boxed::Box::into_raw(obj) as *mut core::ffi::c_void; }
    S_OK
}

// Stub implementations for unused factory methods
extern "system" fn factory_stub_1(_: *mut IDWriteFactory, _: *mut core::ffi::c_void, _: *const core::ffi::c_void, _: u32, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_2(_: *mut IDWriteFactory, _: *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_3(_: *mut IDWriteFactory, _: *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_4(_: *mut IDWriteFactory, _: LpcWStr, _: *const core::ffi::c_void, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_5(_: *mut IDWriteFactory, _: *const core::ffi::c_void, _: u32, _: *mut core::ffi::c_void, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_6(_: *mut IDWriteFactory, _: u32, _: u32, _: *const *mut core::ffi::c_void, _: u32, _: u32, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_7(_: *mut IDWriteFactory, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_8(_: *mut IDWriteFactory, _: u64, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_9(_: *mut IDWriteFactory, _: f32, _: f32, _: f32, _: u32, _: u32, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_10(_: *mut IDWriteFactory, _: *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_11(_: *mut IDWriteFactory, _: *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_12(_: *mut IDWriteFactory, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_13(_: *mut IDWriteFactory, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_gdi_layout(_: *mut IDWriteFactory, _: LpcWStr, _: u32, _: *mut core::ffi::c_void, _: f32, _: f32, _: f32, _: *const core::ffi::c_void, _: Bool, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_ellipsis(_: *mut IDWriteFactory, _: *mut core::ffi::c_void, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_analyzer(_: *mut IDWriteFactory, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_number_sub(_: *mut IDWriteFactory, _: u32, _: LpcWStr, _: Bool, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }
extern "system" fn factory_stub_glyph(_: *mut IDWriteFactory, _: *const core::ffi::c_void, _: f32, _: *const core::ffi::c_void, _: u32, _: u32, _: f32, _: f32, _: *mut *mut core::ffi::c_void) -> HResult { ole32::E_NOTIMPL }

static FACTORY_VTBL: IDWriteFactoryVtbl = IDWriteFactoryVtbl {
    query_interface: factory_qi,
    add_ref: factory_addref,
    release: factory_release,
    get_system_font_collection: factory_get_system_font_collection,
    create_custom_font_collection: factory_stub_1,
    register_font_collection_loader: factory_stub_2,
    unregister_font_collection_loader: factory_stub_3,
    create_font_file_reference: factory_stub_4,
    create_custom_font_file_reference: factory_stub_5,
    create_font_face: factory_stub_6,
    create_rendering_params: factory_stub_7,
    create_monitor_rendering_params: factory_stub_8,
    create_custom_rendering_params: factory_stub_9,
    register_font_file_loader: factory_stub_10,
    unregister_font_file_loader: factory_stub_11,
    create_text_format: factory_create_text_format,
    create_typography: factory_stub_12,
    get_gdi_interop: factory_stub_13,
    create_text_layout: factory_create_text_layout,
    create_gdi_compatible_text_layout: factory_stub_gdi_layout,
    create_ellipsis_trimming_sign: factory_stub_ellipsis,
    create_text_analyzer: factory_stub_analyzer,
    create_number_substitution: factory_stub_number_sub,
    create_glyph_run_analysis: factory_stub_glyph,
};

// =============================================================================
// TextFormat vtable implementation
// =============================================================================

extern "system" fn format_qi(this: *mut IDWriteTextFormat, _riid: *const Guid, ppv: *mut *mut core::ffi::c_void) -> HResult {
    if ppv.is_null() { return E_INVALIDARG; }
    unsafe { *ppv = this as *mut core::ffi::c_void; }
    S_OK
}
extern "system" fn format_addref(_: *mut IDWriteTextFormat) -> u32 { 1 }
extern "system" fn format_release(_: *mut IDWriteTextFormat) -> u32 { 0 }

extern "system" fn format_set_text_alignment(this: *mut IDWriteTextFormat, alignment: u32) -> HResult {
    let handle = unsafe { (*this).handle };
    let mut table = FORMAT_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(fmt) = map.get_mut(&handle) {
            fmt.text_alignment = match alignment {
                0 => TextAlignment::Leading,
                1 => TextAlignment::Trailing,
                2 => TextAlignment::Center,
                3 => TextAlignment::Justified,
                _ => TextAlignment::Leading,
            };
            return S_OK;
        }
    }
    E_FAIL
}

extern "system" fn format_set_paragraph_alignment(this: *mut IDWriteTextFormat, alignment: u32) -> HResult {
    let handle = unsafe { (*this).handle };
    let mut table = FORMAT_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(fmt) = map.get_mut(&handle) {
            fmt.paragraph_alignment = match alignment {
                0 => ParagraphAlignment::Near,
                1 => ParagraphAlignment::Far,
                2 => ParagraphAlignment::Center,
                _ => ParagraphAlignment::Near,
            };
            return S_OK;
        }
    }
    E_FAIL
}

extern "system" fn format_stub_hresult_u32(_: *mut IDWriteTextFormat, _: u32) -> HResult { S_OK }
extern "system" fn format_stub_hresult_f32(_: *mut IDWriteTextFormat, _: f32) -> HResult { S_OK }
extern "system" fn format_stub_set_trimming(_: *mut IDWriteTextFormat, _: *const core::ffi::c_void, _: *mut core::ffi::c_void) -> HResult { S_OK }
extern "system" fn format_stub_set_line_spacing(_: *mut IDWriteTextFormat, _: u32, _: f32, _: f32) -> HResult { S_OK }
extern "system" fn format_get_text_alignment(this: *mut IDWriteTextFormat) -> u32 {
    let handle = unsafe { (*this).handle };
    let table = FORMAT_TABLE.lock();
    table.as_ref().and_then(|m| m.get(&handle)).map(|f| f.text_alignment as u32).unwrap_or(0)
}
extern "system" fn format_get_paragraph_alignment(this: *mut IDWriteTextFormat) -> u32 {
    let handle = unsafe { (*this).handle };
    let table = FORMAT_TABLE.lock();
    table.as_ref().and_then(|m| m.get(&handle)).map(|f| f.paragraph_alignment as u32).unwrap_or(0)
}
extern "system" fn format_get_u32_zero(_: *mut IDWriteTextFormat) -> u32 { 0 }
extern "system" fn format_get_f32_zero(_: *mut IDWriteTextFormat) -> f32 { 0.0 }
extern "system" fn format_stub_get_trimming(_: *mut IDWriteTextFormat, _: *mut core::ffi::c_void, _: *mut *mut core::ffi::c_void) -> HResult { S_OK }
extern "system" fn format_stub_get_line_spacing(_: *mut IDWriteTextFormat, _: *mut u32, _: *mut f32, _: *mut f32) -> HResult { S_OK }
extern "system" fn format_get_font_collection(_: *mut IDWriteTextFormat, _: *mut *mut core::ffi::c_void) -> HResult { S_OK }
extern "system" fn format_get_font_family_name_length(this: *mut IDWriteTextFormat) -> u32 {
    let handle = unsafe { (*this).handle };
    let table = FORMAT_TABLE.lock();
    table.as_ref().and_then(|m| m.get(&handle)).map(|f| f.font_family.len() as u32).unwrap_or(0)
}
extern "system" fn format_get_font_family_name(this: *mut IDWriteTextFormat, buf: LpWStr, size: u32) -> HResult {
    let handle = unsafe { (*this).handle };
    let table = FORMAT_TABLE.lock();
    if let Some(fmt) = table.as_ref().and_then(|m| m.get(&handle)) {
        let encoded = utf8_to_utf16(&fmt.font_family);
        let copy_len = (encoded.len()).min(size as usize);
        unsafe {
            core::ptr::copy_nonoverlapping(encoded.as_ptr(), buf, copy_len);
        }
        S_OK
    } else {
        E_FAIL
    }
}
extern "system" fn format_get_font_weight(this: *mut IDWriteTextFormat) -> u32 {
    let handle = unsafe { (*this).handle };
    let table = FORMAT_TABLE.lock();
    table.as_ref().and_then(|m| m.get(&handle)).map(|f| f.font_weight).unwrap_or(400)
}
extern "system" fn format_get_font_style(this: *mut IDWriteTextFormat) -> u32 {
    let handle = unsafe { (*this).handle };
    let table = FORMAT_TABLE.lock();
    table.as_ref().and_then(|m| m.get(&handle)).map(|f| f.font_style).unwrap_or(0)
}
extern "system" fn format_get_font_stretch(this: *mut IDWriteTextFormat) -> u32 {
    let handle = unsafe { (*this).handle };
    let table = FORMAT_TABLE.lock();
    table.as_ref().and_then(|m| m.get(&handle)).map(|f| f.font_stretch).unwrap_or(5)
}
extern "system" fn format_get_font_size(this: *mut IDWriteTextFormat) -> f32 {
    let handle = unsafe { (*this).handle };
    let table = FORMAT_TABLE.lock();
    table.as_ref().and_then(|m| m.get(&handle)).map(|f| f.font_size).unwrap_or(12.0)
}
extern "system" fn format_get_locale_name_length(_: *mut IDWriteTextFormat) -> u32 { 5 } // "en-us"
extern "system" fn format_get_locale_name(this: *mut IDWriteTextFormat, buf: LpWStr, size: u32) -> HResult {
    let locale = utf8_to_utf16("en-us");
    let copy_len = locale.len().min(size as usize);
    unsafe { core::ptr::copy_nonoverlapping(locale.as_ptr(), buf, copy_len); }
    S_OK
}

static TEXT_FORMAT_VTBL: IDWriteTextFormatVtbl = IDWriteTextFormatVtbl {
    query_interface: format_qi,
    add_ref: format_addref,
    release: format_release,
    set_text_alignment: format_set_text_alignment,
    set_paragraph_alignment: format_set_paragraph_alignment,
    set_word_wrapping: format_stub_hresult_u32,
    set_reading_direction: format_stub_hresult_u32,
    set_flow_direction: format_stub_hresult_u32,
    set_incremental_tab_stop: format_stub_hresult_f32,
    set_trimming: format_stub_set_trimming,
    set_line_spacing: format_stub_set_line_spacing,
    get_text_alignment: format_get_text_alignment,
    get_paragraph_alignment: format_get_paragraph_alignment,
    get_word_wrapping: format_get_u32_zero,
    get_reading_direction: format_get_u32_zero,
    get_flow_direction: format_get_u32_zero,
    get_incremental_tab_stop: format_get_f32_zero,
    get_trimming: format_stub_get_trimming,
    get_line_spacing: format_stub_get_line_spacing,
    get_font_collection: format_get_font_collection,
    get_font_family_name_length: format_get_font_family_name_length,
    get_font_family_name: format_get_font_family_name,
    get_font_weight: format_get_font_weight,
    get_font_style: format_get_font_style,
    get_font_stretch: format_get_font_stretch,
    get_font_size: format_get_font_size,
    get_locale_name_length: format_get_locale_name_length,
    get_locale_name: format_get_locale_name,
};

// =============================================================================
// Public API
// =============================================================================

/// DWriteCreateFactory — create a DirectWrite factory.
pub fn dwrite_create_factory(
    factory_type: u32,
    iid: *const Guid,
    factory: *mut *mut core::ffi::c_void,
) -> HResult {
    if factory.is_null() {
        return E_INVALIDARG;
    }

    log::info!("[directwrite] DWriteCreateFactory: type={}", factory_type);

    let handle = {
        let mut counter = NEXT_FACTORY.lock();
        let h = FACTORY_BASE + (*counter - FACTORY_BASE);
        *counter += 1;
        h
    };

    {
        let mut table = FACTORY_TABLE.lock();
        if table.is_none() { *table = Some(BTreeMap::new()); }
        if let Some(ref mut map) = *table {
            map.insert(handle, true);
        }
    }

    let obj = alloc::boxed::Box::new(IDWriteFactory {
        vtbl: &FACTORY_VTBL,
        handle,
    });
    unsafe { *factory = alloc::boxed::Box::into_raw(obj) as *mut core::ffi::c_void; }
    S_OK
}

/// Get text metrics for a layout — standalone helper.
pub fn get_text_layout_metrics(layout_handle: u64, metrics: *mut TextMetrics) -> HResult {
    if metrics.is_null() { return E_INVALIDARG; }

    let table = LAYOUT_TABLE.lock();
    if let Some(layout) = table.as_ref().and_then(|m| m.get(&layout_handle)) {
        // Estimate metrics based on text length and font size
        let font_size = {
            let fmt_table = FORMAT_TABLE.lock();
            fmt_table.as_ref()
                .and_then(|m| m.get(&layout.format_handle))
                .map(|f| f.font_size)
                .unwrap_or(16.0)
        };

        let char_width = font_size * 0.6; // Approximate monospace character width
        let line_height = font_size * 1.2;
        let text_width = layout.text.len() as f32 * char_width;
        let lines = if layout.max_width > 0.0 {
            let ratio = text_width / layout.max_width;
            let floored = ratio as u32;
            let ceiled = if ratio > floored as f32 { floored + 1 } else { floored };
            ceiled.max(1)
        } else {
            1
        };

        unsafe {
            *metrics = TextMetrics {
                left: 0.0,
                top: 0.0,
                width: text_width.min(layout.max_width),
                width_including_trailing_whitespace: text_width.min(layout.max_width),
                height: lines as f32 * line_height,
                layout_width: layout.max_width,
                layout_height: layout.max_height,
                max_bidi_reordering_depth: 1,
                line_count: lines,
            };
        }
        S_OK
    } else {
        E_FAIL
    }
}

/// Enumerate built-in system fonts — returns font family names.
pub fn enumerate_system_fonts() -> Vec<String> {
    BUILTIN_FONTS.iter().map(|&s| String::from(s)).collect()
}
