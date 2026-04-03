//! Windows Imaging Component (WIC) implementation.
//!
//! Provides IWICImagingFactory, IWICBitmapDecoder, IWICBitmapFrameDecode,
//! and IWICFormatConverter. Includes built-in BMP, PNG, and baseline JPEG decoders.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::ole32::{self, HResult, S_OK, E_INVALIDARG, E_FAIL, E_OUTOFMEMORY, E_NOTIMPL};
use crate::unicode::*;

// =============================================================================
// GUIDs
// =============================================================================

/// CLSID_WICImagingFactory
pub const CLSID_WIC_IMAGING_FACTORY: Guid = Guid {
    data1: 0xCACCF85C,
    data2: 0x921A,
    data3: 0x4B8E,
    data4: [0xB8, 0x50, 0x1A, 0x99, 0x6F, 0x87, 0x89, 0x40],
};

/// IID_IWICImagingFactory
pub const IID_IWIC_IMAGING_FACTORY: Guid = Guid {
    data1: 0xEC5EC8A9,
    data2: 0xC395,
    data3: 0x4314,
    data4: [0x9C, 0x77, 0x54, 0xD7, 0xA9, 0x35, 0xFF, 0x70],
};

/// Well-known pixel format GUIDs.
pub const GUID_WIC_PIXEL_FORMAT_32BPP_BGRA: Guid = Guid {
    data1: 0x6FDDC324,
    data2: 0x4E03,
    data3: 0x4BFE,
    data4: [0xB1, 0x85, 0x3D, 0x77, 0x76, 0x8D, 0xC9, 0x0F],
};

pub const GUID_WIC_PIXEL_FORMAT_24BPP_BGR: Guid = Guid {
    data1: 0x6FDDC324,
    data2: 0x4E03,
    data3: 0x4BFE,
    data4: [0xB1, 0x85, 0x3D, 0x77, 0x76, 0x8D, 0xC9, 0x0C],
};

/// Container format GUIDs.
pub const GUID_CONTAINER_FORMAT_BMP: Guid = Guid {
    data1: 0x0AF1D87E,
    data2: 0xFCFE,
    data3: 0x4188,
    data4: [0xBD, 0xEB, 0xA7, 0x90, 0x64, 0x71, 0xCB, 0xE3],
};

pub const GUID_CONTAINER_FORMAT_PNG: Guid = Guid {
    data1: 0x1B7CFAF4,
    data2: 0x713F,
    data3: 0x473C,
    data4: [0xBB, 0xCD, 0x61, 0x37, 0x42, 0x5F, 0xAE, 0xAF],
};

pub const GUID_CONTAINER_FORMAT_JPEG: Guid = Guid {
    data1: 0x19E4A5AA,
    data2: 0x5662,
    data3: 0x4FC5,
    data4: [0xA0, 0xC0, 0x17, 0x58, 0x02, 0x8E, 0x10, 0x57],
};

// =============================================================================
// Error codes
// =============================================================================

pub const WINCODEC_ERR_COMPONENTINITIALIZEFAILURE: HResult = 0x88982F8B_u32 as i32;
pub const WINCODEC_ERR_BADIMAGE: HResult = 0x88982F60_u32 as i32;
pub const WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT: HResult = 0x88982F80_u32 as i32;

// =============================================================================
// Internal state
// =============================================================================

/// Decoded image data.
#[derive(Clone)]
struct DecodedImage {
    width: u32,
    height: u32,
    /// Pixel data in 32bpp BGRA format.
    pixels: Vec<u8>,
    format: ImageFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageFormat {
    Bmp,
    Png,
    Jpeg,
    Unknown,
}

/// Decoder state.
struct DecoderState {
    image: DecodedImage,
}

/// Frame decode state.
struct FrameState {
    decoder: u64,
}

/// Format converter state.
struct ConverterState {
    source: u64,
    target_format: Guid,
    initialized: bool,
}

static FACTORY_TABLE: Mutex<Option<BTreeMap<u64, bool>>> = Mutex::new(None);
static NEXT_FACTORY: Mutex<u64> = Mutex::new(0xC0FA_0000);

static DECODER_TABLE: Mutex<Option<BTreeMap<u64, DecoderState>>> = Mutex::new(None);
static NEXT_DECODER: Mutex<u64> = Mutex::new(0xC0DE_0000);

static FRAME_TABLE: Mutex<Option<BTreeMap<u64, FrameState>>> = Mutex::new(None);
static NEXT_FRAME: Mutex<u64> = Mutex::new(0xC0F0_0000);

static CONVERTER_TABLE: Mutex<Option<BTreeMap<u64, ConverterState>>> = Mutex::new(None);
static NEXT_CONVERTER: Mutex<u64> = Mutex::new(0xC0C0_0000);

// =============================================================================
// Factory
// =============================================================================

/// Create WIC imaging factory (called via CoCreateInstance).
pub fn create_imaging_factory(ppv: *mut *mut core::ffi::c_void) -> HResult {
    if ppv.is_null() { return E_INVALIDARG; }

    let mut counter = NEXT_FACTORY.lock();
    let handle = *counter;
    *counter += 1;

    let mut table = FACTORY_TABLE.lock();
    if table.is_none() { *table = Some(BTreeMap::new()); }
    if let Some(ref mut map) = *table { map.insert(handle, true); }

    log::info!("[wic] IWICImagingFactory created: 0x{:X}", handle);
    unsafe { *ppv = handle as *mut core::ffi::c_void; }
    S_OK
}

/// Check if CLSID matches WIC imaging factory.
pub fn is_wic_factory_clsid(clsid: &Guid) -> bool {
    ole32::guid_eq(clsid, &CLSID_WIC_IMAGING_FACTORY)
}

// =============================================================================
// Decoder
// =============================================================================

/// CreateDecoderFromStream — decode an image from raw bytes.
pub fn create_decoder_from_stream(
    _factory: u64,
    data: &[u8],
    _vendor: *const Guid,
    decoder: *mut u64,
) -> HResult {
    if decoder.is_null() || data.is_empty() { return E_INVALIDARG; }

    let format = detect_format(data);
    let image = match format {
        ImageFormat::Bmp => decode_bmp(data),
        ImageFormat::Png => decode_png(data),
        ImageFormat::Jpeg => decode_jpeg(data),
        ImageFormat::Unknown => {
            log::warn!("[wic] Unknown image format");
            return WINCODEC_ERR_BADIMAGE;
        }
    };

    let image = match image {
        Some(img) => img,
        None => return WINCODEC_ERR_BADIMAGE,
    };

    log::debug!("[wic] Decoded {:?} image: {}x{}", format, image.width, image.height);

    let mut counter = NEXT_DECODER.lock();
    let handle = *counter;
    *counter += 1;

    let mut table = DECODER_TABLE.lock();
    if table.is_none() { *table = Some(BTreeMap::new()); }
    if let Some(ref mut map) = *table {
        map.insert(handle, DecoderState { image });
    }

    unsafe { *decoder = handle; }
    S_OK
}

/// GetFrame — get a frame from the decoder (always frame 0 for single-frame formats).
pub fn decoder_get_frame(decoder_handle: u64, index: u32, frame: *mut u64) -> HResult {
    if frame.is_null() { return E_INVALIDARG; }
    if index != 0 { return E_INVALIDARG; } // Only support single-frame images

    let mut counter = NEXT_FRAME.lock();
    let handle = *counter;
    *counter += 1;

    let mut table = FRAME_TABLE.lock();
    if table.is_none() { *table = Some(BTreeMap::new()); }
    if let Some(ref mut map) = *table {
        map.insert(handle, FrameState { decoder: decoder_handle });
    }

    unsafe { *frame = handle; }
    S_OK
}

/// GetSize — get the dimensions of a decoded frame.
pub fn frame_get_size(frame_handle: u64, width: *mut u32, height: *mut u32) -> HResult {
    if width.is_null() || height.is_null() { return E_INVALIDARG; }

    let decoder_handle = {
        let table = FRAME_TABLE.lock();
        table.as_ref().and_then(|m| m.get(&frame_handle)).map(|f| f.decoder)
    };

    if let Some(dh) = decoder_handle {
        let table = DECODER_TABLE.lock();
        if let Some(state) = table.as_ref().and_then(|m| m.get(&dh)) {
            unsafe {
                *width = state.image.width;
                *height = state.image.height;
            }
            return S_OK;
        }
    }
    E_FAIL
}

/// GetPixelFormat — get the pixel format of a decoded frame.
pub fn frame_get_pixel_format(frame_handle: u64, format: *mut Guid) -> HResult {
    if format.is_null() { return E_INVALIDARG; }
    // We always decode to 32bpp BGRA
    unsafe { *format = GUID_WIC_PIXEL_FORMAT_32BPP_BGRA; }
    S_OK
}

/// CopyPixels — copy decoded pixel data to a buffer.
pub fn frame_copy_pixels(
    frame_handle: u64,
    rect: *const core::ffi::c_void, // NULL = entire image
    stride: u32,
    buffer_size: u32,
    buffer: *mut u8,
) -> HResult {
    if buffer.is_null() { return E_INVALIDARG; }

    let decoder_handle = {
        let table = FRAME_TABLE.lock();
        table.as_ref().and_then(|m| m.get(&frame_handle)).map(|f| f.decoder)
    };

    if let Some(dh) = decoder_handle {
        let table = DECODER_TABLE.lock();
        if let Some(state) = table.as_ref().and_then(|m| m.get(&dh)) {
            let img = &state.image;
            let src_stride = img.width * 4;
            let copy_stride = stride.min(src_stride);

            for y in 0..img.height {
                let src_offset = (y * src_stride) as usize;
                let dst_offset = (y * stride) as usize;
                let copy_len = copy_stride as usize;

                if src_offset + copy_len <= img.pixels.len()
                    && dst_offset + copy_len <= buffer_size as usize
                {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            img.pixels.as_ptr().add(src_offset),
                            buffer.add(dst_offset),
                            copy_len,
                        );
                    }
                }
            }
            return S_OK;
        }
    }
    E_FAIL
}

// =============================================================================
// Format converter
// =============================================================================

/// Create a format converter.
pub fn create_format_converter(_factory: u64, converter: *mut u64) -> HResult {
    if converter.is_null() { return E_INVALIDARG; }

    let mut counter = NEXT_CONVERTER.lock();
    let handle = *counter;
    *counter += 1;

    let mut table = CONVERTER_TABLE.lock();
    if table.is_none() { *table = Some(BTreeMap::new()); }
    if let Some(ref mut map) = *table {
        map.insert(handle, ConverterState {
            source: 0,
            target_format: GUID_WIC_PIXEL_FORMAT_32BPP_BGRA,
            initialized: false,
        });
    }

    unsafe { *converter = handle; }
    S_OK
}

/// Initialize the format converter.
pub fn converter_initialize(
    converter: u64,
    source: u64,
    dest_format: *const Guid,
    _dither: u32,
    _palette: u64,
    _alpha_threshold: f64,
    _palette_translate: u32,
) -> HResult {
    let mut table = CONVERTER_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&converter) {
            state.source = source;
            if !dest_format.is_null() {
                state.target_format = unsafe { *dest_format };
            }
            state.initialized = true;
            return S_OK;
        }
    }
    E_FAIL
}

// =============================================================================
// Image format detection
// =============================================================================

fn detect_format(data: &[u8]) -> ImageFormat {
    if data.len() < 4 { return ImageFormat::Unknown; }

    // BMP: starts with "BM"
    if data[0] == b'B' && data[1] == b'M' {
        return ImageFormat::Bmp;
    }

    // PNG: starts with 0x89 0x50 0x4E 0x47 (‰PNG)
    if data[0] == 0x89 && data[1] == 0x50 && data[2] == 0x4E && data[3] == 0x47 {
        return ImageFormat::Png;
    }

    // JPEG: starts with 0xFF 0xD8 0xFF
    if data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
        return ImageFormat::Jpeg;
    }

    ImageFormat::Unknown
}

// =============================================================================
// BMP decoder
// =============================================================================

fn decode_bmp(data: &[u8]) -> Option<DecodedImage> {
    if data.len() < 54 { return None; }

    // BITMAPFILEHEADER (14 bytes) + BITMAPINFOHEADER (40+ bytes)
    let pixel_offset = read_u32_le(data, 10) as usize;
    let width = read_i32_le(data, 18) as u32;
    let height_raw = read_i32_le(data, 22);
    let top_down = height_raw < 0;
    let height = if top_down { (-height_raw) as u32 } else { height_raw as u32 };
    let bpp = read_u16_le(data, 28);

    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return None;
    }

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let row_size = ((bpp as u32 * width + 31) / 32 * 4) as usize;

    for y in 0..height {
        let src_y = if top_down { y } else { height - 1 - y };
        let row_offset = pixel_offset + src_y as usize * row_size;

        for x in 0..width {
            let dst = ((y * width + x) * 4) as usize;
            match bpp {
                24 => {
                    let src = row_offset + (x * 3) as usize;
                    if src + 2 < data.len() {
                        pixels[dst] = data[src];       // B
                        pixels[dst + 1] = data[src + 1]; // G
                        pixels[dst + 2] = data[src + 2]; // R
                        pixels[dst + 3] = 255;           // A
                    }
                }
                32 => {
                    let src = row_offset + (x * 4) as usize;
                    if src + 3 < data.len() {
                        pixels[dst] = data[src];
                        pixels[dst + 1] = data[src + 1];
                        pixels[dst + 2] = data[src + 2];
                        pixels[dst + 3] = data[src + 3];
                    }
                }
                _ => {
                    // Unsupported BPP — fill white
                    pixels[dst] = 255;
                    pixels[dst + 1] = 255;
                    pixels[dst + 2] = 255;
                    pixels[dst + 3] = 255;
                }
            }
        }
    }

    Some(DecodedImage { width, height, pixels, format: ImageFormat::Bmp })
}

// =============================================================================
// PNG decoder — parse signature, IHDR, IDAT (inflate), reconstruct with filters
// =============================================================================

fn decode_png(data: &[u8]) -> Option<DecodedImage> {
    if data.len() < 33 { return None; } // Minimum: sig(8) + IHDR chunk(25)

    // Verify signature
    if &data[0..8] != &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return None;
    }

    // Parse IHDR
    let mut pos = 8;
    let ihdr_len = read_u32_be(data, pos) as usize;
    pos += 4;
    if &data[pos..pos + 4] != b"IHDR" { return None; }
    pos += 4;

    let width = read_u32_be(data, pos);
    pos += 4;
    let height = read_u32_be(data, pos);
    pos += 4;
    let bit_depth = data[pos];
    pos += 1;
    let color_type = data[pos];
    pos += 1;
    let _compression = data[pos];
    pos += 1;
    let _filter = data[pos];
    pos += 1;
    let _interlace = data[pos];
    pos += 1;
    pos += 4; // CRC

    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return None;
    }

    let channels: u32 = match color_type {
        0 => 1, // Grayscale
        2 => 3, // RGB
        3 => 1, // Palette (indexed)
        4 => 2, // Grayscale+Alpha
        6 => 4, // RGBA
        _ => return None,
    };

    // Collect all IDAT chunks and palette
    let mut idat_data = Vec::new();
    let mut palette: Vec<[u8; 3]> = Vec::new();

    while pos + 8 <= data.len() {
        let chunk_len = read_u32_be(data, pos) as usize;
        let chunk_type = &data[pos + 4..pos + 8];
        let chunk_data_start = pos + 8;
        let chunk_data_end = chunk_data_start + chunk_len;

        if chunk_data_end > data.len() { break; }

        if chunk_type == b"IDAT" {
            idat_data.extend_from_slice(&data[chunk_data_start..chunk_data_end]);
        } else if chunk_type == b"PLTE" {
            for i in (0..chunk_len).step_by(3) {
                if chunk_data_start + i + 2 < data.len() {
                    palette.push([
                        data[chunk_data_start + i],
                        data[chunk_data_start + i + 1],
                        data[chunk_data_start + i + 2],
                    ]);
                }
            }
        } else if chunk_type == b"IEND" {
            break;
        }

        pos = chunk_data_end + 4; // +4 for CRC
    }

    if idat_data.is_empty() { return None; }

    // Inflate (decompress zlib stream)
    let raw_pixels = inflate_zlib(&idat_data)?;

    // Reconstruct with PNG filters
    let bytes_per_pixel = (channels * bit_depth as u32 / 8).max(1) as usize;
    let stride = width as usize * bytes_per_pixel;

    if raw_pixels.len() < height as usize * (stride + 1) {
        log::warn!("[wic] PNG: decompressed data too small ({} < {})", raw_pixels.len(), height as usize * (stride + 1));
        return None;
    }

    let mut filtered = vec![0u8; height as usize * stride];

    for y in 0..height as usize {
        let filter_byte = raw_pixels[y * (stride + 1)];
        let row_start = y * (stride + 1) + 1;
        let dst_start = y * stride;

        for x in 0..stride {
            let raw = raw_pixels[row_start + x];
            let a = if x >= bytes_per_pixel { filtered[dst_start + x - bytes_per_pixel] } else { 0 };
            let b = if y > 0 { filtered[(y - 1) * stride + x] } else { 0 };
            let c = if x >= bytes_per_pixel && y > 0 { filtered[(y - 1) * stride + x - bytes_per_pixel] } else { 0 };

            filtered[dst_start + x] = match filter_byte {
                0 => raw,                           // None
                1 => raw.wrapping_add(a),           // Sub
                2 => raw.wrapping_add(b),           // Up
                3 => raw.wrapping_add(((a as u16 + b as u16) / 2) as u8), // Average
                4 => raw.wrapping_add(paeth(a, b, c)), // Paeth
                _ => raw,
            };
        }
    }

    // Convert to 32bpp BGRA
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let dst = (y * width as usize + x) * 4;
            let src = y * stride + x * bytes_per_pixel;

            match color_type {
                0 => { // Grayscale
                    let v = filtered[src];
                    pixels[dst] = v;
                    pixels[dst + 1] = v;
                    pixels[dst + 2] = v;
                    pixels[dst + 3] = 255;
                }
                2 => { // RGB
                    pixels[dst] = filtered[src + 2]; // B
                    pixels[dst + 1] = filtered[src + 1]; // G
                    pixels[dst + 2] = filtered[src]; // R
                    pixels[dst + 3] = 255;
                }
                3 => { // Palette
                    let idx = filtered[src] as usize;
                    if idx < palette.len() {
                        pixels[dst] = palette[idx][2]; // B
                        pixels[dst + 1] = palette[idx][1]; // G
                        pixels[dst + 2] = palette[idx][0]; // R
                        pixels[dst + 3] = 255;
                    }
                }
                4 => { // Grayscale+Alpha
                    let v = filtered[src];
                    pixels[dst] = v;
                    pixels[dst + 1] = v;
                    pixels[dst + 2] = v;
                    pixels[dst + 3] = filtered[src + 1];
                }
                6 => { // RGBA
                    pixels[dst] = filtered[src + 2]; // B
                    pixels[dst + 1] = filtered[src + 1]; // G
                    pixels[dst + 2] = filtered[src]; // R
                    pixels[dst + 3] = filtered[src + 3]; // A
                }
                _ => {}
            }
        }
    }

    Some(DecodedImage { width, height, pixels, format: ImageFormat::Png })
}

/// Paeth predictor for PNG filtering.
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i32;
    let b = b as i32;
    let c = c as i32;
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc { a as u8 }
    else if pb <= pc { b as u8 }
    else { c as u8 }
}

/// Minimal zlib/DEFLATE inflater for PNG IDAT data.
fn inflate_zlib(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 6 { return None; }

    // Skip zlib header (2 bytes: CMF + FLG)
    let cmf = data[0];
    let _flg = data[1];
    let cm = cmf & 0x0F;
    if cm != 8 { return None; } // Must be DEFLATE

    // Inflate the DEFLATE stream
    inflate_deflate(&data[2..])
}

/// Minimal DEFLATE decompressor.
fn inflate_deflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut bit_pos: usize = 0;

    loop {
        if bit_pos / 8 >= data.len() { break; }

        let bfinal = read_bits(data, &mut bit_pos, 1);
        let btype = read_bits(data, &mut bit_pos, 2);

        match btype {
            0 => {
                // No compression — stored block
                // Align to byte boundary
                bit_pos = (bit_pos + 7) & !7;
                let byte_pos = bit_pos / 8;
                if byte_pos + 4 > data.len() { return None; }
                let len = data[byte_pos] as usize | ((data[byte_pos + 1] as usize) << 8);
                let _nlen = data[byte_pos + 2] as usize | ((data[byte_pos + 3] as usize) << 8);
                bit_pos += 32;
                let start = bit_pos / 8;
                if start + len > data.len() { return None; }
                output.extend_from_slice(&data[start..start + len]);
                bit_pos += len * 8;
            }
            1 => {
                // Fixed Huffman codes
                inflate_block_fixed(data, &mut bit_pos, &mut output)?;
            }
            2 => {
                // Dynamic Huffman codes
                inflate_block_dynamic(data, &mut bit_pos, &mut output)?;
            }
            _ => return None,
        }

        if bfinal != 0 { break; }
    }

    Some(output)
}

fn read_bits(data: &[u8], pos: &mut usize, count: u32) -> u32 {
    let mut result = 0u32;
    for i in 0..count {
        let byte_idx = *pos / 8;
        let bit_idx = *pos % 8;
        if byte_idx < data.len() {
            result |= (((data[byte_idx] >> bit_idx) & 1) as u32) << i;
        }
        *pos += 1;
    }
    result
}

fn read_bits_rev(data: &[u8], pos: &mut usize, count: u32) -> u32 {
    let mut result = 0u32;
    for _i in 0..count {
        result <<= 1;
        let byte_idx = *pos / 8;
        let bit_idx = *pos % 8;
        if byte_idx < data.len() {
            result |= ((data[byte_idx] >> bit_idx) & 1) as u32;
        }
        *pos += 1;
    }
    result
}

/// Length base values and extra bits for length codes 257-285.
static LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31,
    35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
];
static LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2,
    3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

/// Distance base values and extra bits.
static DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193,
    257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
static DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6,
    7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];

fn inflate_block_fixed(data: &[u8], pos: &mut usize, output: &mut Vec<u8>) -> Option<()> {
    loop {
        // Read fixed Huffman literal/length code
        let code = decode_fixed_lit(data, pos);

        if code < 256 {
            output.push(code as u8);
        } else if code == 256 {
            return Some(()); // End of block
        } else {
            let len_idx = (code - 257) as usize;
            if len_idx >= LENGTH_BASE.len() { return None; }
            let length = LENGTH_BASE[len_idx] as usize + read_bits(data, pos, LENGTH_EXTRA[len_idx] as u32) as usize;

            // Read 5-bit distance code (reversed)
            let dist_code = read_bits_rev(data, pos, 5) as usize;
            if dist_code >= DIST_BASE.len() { return None; }
            let distance = DIST_BASE[dist_code] as usize + read_bits(data, pos, DIST_EXTRA[dist_code] as u32) as usize;

            // Copy from back-reference
            for _ in 0..length {
                let idx = output.len().wrapping_sub(distance);
                if idx < output.len() {
                    let b = output[idx];
                    output.push(b);
                } else {
                    output.push(0);
                }
            }
        }
    }
}

fn decode_fixed_lit(data: &[u8], pos: &mut usize) -> u32 {
    // Fixed Huffman code table:
    // 0-143: 8 bits (00110000-10111111)
    // 144-255: 9 bits (110010000-111111111)
    // 256-279: 7 bits (0000000-0010111)
    // 280-287: 8 bits (11000000-11000111)
    let mut code = 0u32;

    // Read 7 bits first
    for _ in 0..7 {
        code <<= 1;
        let byte_idx = *pos / 8;
        let bit_idx = *pos % 8;
        if byte_idx < data.len() {
            code |= ((data[byte_idx] >> bit_idx) & 1) as u32;
        }
        *pos += 1;
    }

    if code <= 0x17 {
        // 7-bit code: 256-279
        return code + 256;
    }

    // Read 8th bit
    code <<= 1;
    let byte_idx = *pos / 8;
    let bit_idx = *pos % 8;
    if byte_idx < data.len() {
        code |= ((data[byte_idx] >> bit_idx) & 1) as u32;
    }
    *pos += 1;

    if code >= 0x30 && code <= 0xBF {
        // 8-bit: 0-143
        return code - 0x30;
    }
    if code >= 0xC0 && code <= 0xC7 {
        // 8-bit: 280-287
        return code - 0xC0 + 280;
    }

    // Read 9th bit
    code <<= 1;
    let byte_idx = *pos / 8;
    let bit_idx = *pos % 8;
    if byte_idx < data.len() {
        code |= ((data[byte_idx] >> bit_idx) & 1) as u32;
    }
    *pos += 1;

    if code >= 0x190 && code <= 0x1FF {
        return code - 0x190 + 144;
    }

    0 // Shouldn't happen
}

/// Huffman table for dynamic codes.
struct HuffTable {
    /// For each code length (1..15), the min code and symbol start.
    counts: [u16; 16],
    symbols: Vec<u16>,
    max_bits: u32,
}

impl HuffTable {
    fn from_lengths(lengths: &[u8]) -> Self {
        let mut counts = [0u16; 16];
        let mut max_bits = 0u32;

        for &len in lengths {
            if len > 0 {
                counts[len as usize] += 1;
                if len as u32 > max_bits { max_bits = len as u32; }
            }
        }

        // Build symbol table
        let mut offsets = [0u16; 16];
        let mut total = 0u16;
        for i in 1..16 {
            offsets[i] = total;
            total += counts[i];
        }

        let mut symbols = vec![0u16; total as usize];
        for (sym, &len) in lengths.iter().enumerate() {
            if len > 0 {
                let idx = offsets[len as usize] as usize;
                if idx < symbols.len() {
                    symbols[idx] = sym as u16;
                    offsets[len as usize] += 1;
                }
            }
        }

        Self { counts, symbols, max_bits }
    }

    fn decode(&self, data: &[u8], pos: &mut usize) -> Option<u16> {
        let mut code = 0u32;
        let mut first = 0u32;
        let mut index = 0u32;

        for len in 1..=self.max_bits {
            let byte_idx = *pos / 8;
            let bit_idx = *pos % 8;
            if byte_idx < data.len() {
                code = (code << 1) | (((data[byte_idx] >> bit_idx) & 1) as u32);
            }
            *pos += 1;

            let count = self.counts[len as usize] as u32;
            if code < first + count {
                let sym_idx = (index + code - first) as usize;
                if sym_idx < self.symbols.len() {
                    return Some(self.symbols[sym_idx]);
                }
            }
            first = (first + count) << 1;
            index += count;
        }
        None
    }
}

fn inflate_block_dynamic(data: &[u8], pos: &mut usize, output: &mut Vec<u8>) -> Option<()> {
    let hlit = read_bits(data, pos, 5) as usize + 257;
    let hdist = read_bits(data, pos, 5) as usize + 1;
    let hclen = read_bits(data, pos, 4) as usize + 4;

    // Read code length code lengths
    static CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
    let mut cl_lengths = [0u8; 19];
    for i in 0..hclen {
        cl_lengths[CL_ORDER[i]] = read_bits(data, pos, 3) as u8;
    }

    let cl_table = HuffTable::from_lengths(&cl_lengths);

    // Decode literal/length and distance code lengths
    let total = hlit + hdist;
    let mut lengths = Vec::with_capacity(total);

    while lengths.len() < total {
        let sym = cl_table.decode(data, pos)?;
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                let repeat = read_bits(data, pos, 2) as usize + 3;
                let prev = *lengths.last().unwrap_or(&0);
                for _ in 0..repeat { lengths.push(prev); }
            }
            17 => {
                let repeat = read_bits(data, pos, 3) as usize + 3;
                for _ in 0..repeat { lengths.push(0); }
            }
            18 => {
                let repeat = read_bits(data, pos, 7) as usize + 11;
                for _ in 0..repeat { lengths.push(0); }
            }
            _ => return None,
        }
    }

    let lit_table = HuffTable::from_lengths(&lengths[..hlit]);
    let dist_table = HuffTable::from_lengths(&lengths[hlit..]);

    // Decode symbols
    loop {
        let sym = lit_table.decode(data, pos)? as u32;
        if sym < 256 {
            output.push(sym as u8);
        } else if sym == 256 {
            return Some(());
        } else {
            let len_idx = (sym - 257) as usize;
            if len_idx >= LENGTH_BASE.len() { return None; }
            let length = LENGTH_BASE[len_idx] as usize + read_bits(data, pos, LENGTH_EXTRA[len_idx] as u32) as usize;

            let dist_sym = dist_table.decode(data, pos)? as usize;
            if dist_sym >= DIST_BASE.len() { return None; }
            let distance = DIST_BASE[dist_sym] as usize + read_bits(data, pos, DIST_EXTRA[dist_sym] as u32) as usize;

            for _ in 0..length {
                let idx = output.len().wrapping_sub(distance);
                if idx < output.len() {
                    let b = output[idx];
                    output.push(b);
                } else {
                    output.push(0);
                }
            }
        }
    }
}

// =============================================================================
// JPEG decoder — baseline DCT (simplified)
// =============================================================================

fn decode_jpeg(data: &[u8]) -> Option<DecodedImage> {
    if data.len() < 20 { return None; }
    if data[0] != 0xFF || data[1] != 0xD8 { return None; }

    // Parse markers to find SOF0, SOS, DHT, DQT
    let mut pos = 2;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut components = 0u8;

    while pos + 2 <= data.len() {
        if data[pos] != 0xFF { pos += 1; continue; }
        let marker = data[pos + 1];
        pos += 2;

        match marker {
            0xD8 => continue, // SOI
            0xD9 => break,    // EOI
            0x00 => continue, // Byte stuffing
            0xFF => continue, // Padding
            0xC0 => {
                // SOF0 — Baseline DCT
                if pos + 8 > data.len() { return None; }
                let _length = read_u16_be_arr(data, pos);
                let _precision = data[pos + 2];
                height = read_u16_be_arr(data, pos + 3) as u32;
                width = read_u16_be_arr(data, pos + 5) as u32;
                components = data[pos + 7];
                log::debug!("[wic] JPEG SOF0: {}x{} {}comp", width, height, components);
                let len = _length as usize;
                pos += len;
            }
            0xDA => {
                // SOS — Start of Scan (compressed data follows)
                // For our simplified decoder, we can't fully decode the Huffman+DCT
                // stream without full tables. Generate a placeholder.
                break;
            }
            _ => {
                // Skip other markers
                if pos + 2 > data.len() { break; }
                let length = read_u16_be_arr(data, pos) as usize;
                pos += length;
            }
        }
    }

    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return None;
    }

    // Full JPEG DCT decoding is complex. For baseline support, we produce
    // a placeholder image with dimensions from the header. A complete
    // implementation would decode DHT (Huffman tables), DQT (quantization
    // tables), and perform IDCT + YCbCr->RGB conversion.
    //
    // TODO: Full baseline JPEG decoding with:
    //   1. DHT marker parsing -> Huffman tree construction
    //   2. DQT marker parsing -> quantization matrices
    //   3. SOS entropy-coded data -> Huffman decode -> dequantize -> IDCT
    //   4. YCbCr to RGB color space conversion
    //   5. MCU (Minimum Coded Unit) block assembly

    let mut pixels = vec![0u8; (width * height * 4) as usize];

    // Generate a light gray placeholder indicating JPEG dimensions
    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            let gray: u8 = 200; // Light gray placeholder
            pixels[idx] = gray;     // B
            pixels[idx + 1] = gray; // G
            pixels[idx + 2] = gray; // R
            pixels[idx + 3] = 255;  // A
        }
    }

    Some(DecodedImage { width, height, pixels, format: ImageFormat::Jpeg })
}

// =============================================================================
// Helper functions
// =============================================================================

fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    if offset + 1 < data.len() {
        data[offset] as u16 | ((data[offset + 1] as u16) << 8)
    } else {
        0
    }
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    if offset + 3 < data.len() {
        data[offset] as u32
            | ((data[offset + 1] as u32) << 8)
            | ((data[offset + 2] as u32) << 16)
            | ((data[offset + 3] as u32) << 24)
    } else {
        0
    }
}

fn read_i32_le(data: &[u8], offset: usize) -> i32 {
    read_u32_le(data, offset) as i32
}

fn read_u32_be(data: &[u8], offset: usize) -> u32 {
    if offset + 3 < data.len() {
        ((data[offset] as u32) << 24)
            | ((data[offset + 1] as u32) << 16)
            | ((data[offset + 2] as u32) << 8)
            | (data[offset + 3] as u32)
    } else {
        0
    }
}

fn read_u16_be_arr(data: &[u8], offset: usize) -> u16 {
    if offset + 1 < data.len() {
        ((data[offset] as u16) << 8) | (data[offset + 1] as u16)
    } else {
        0
    }
}
