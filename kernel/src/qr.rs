//! QR code rendering for on-screen debug dumps.
//!
//! Our only out-of-band channel on the Victus is "point a phone at the
//! framebuffer." A QR code is a high-bandwidth one-shot: kernel renders
//! one or more, phone scans them, user pastes the decoded text into chat.
//!
//! Features:
//! - **Alphanumeric mode**: `encode_text` auto-picks densest mode. For
//!   uppercase-hex-with-separators payloads it uses alphanumeric mode
//!   (11 bits per 2 chars, ~1.6× denser than byte mode).
//! - **Compression**: payloads > `COMPRESS_THRESHOLD` get deflated with
//!   miniz_oxide and base64-encoded with a `Z:` prefix. Phone-side: paste
//!   the whole thing, the `Z:` header tells the decoder to base64-decode
//!   and inflate.
//! - **Multi-QR**: if the chosen payload exceeds a single QR's capacity at
//!   the target module size, split into N chunks and render side-by-side.
//!   Each chunk is prefixed `N/K|` so the receiver can reassemble.
//! - **Small modules**: 3px per module by default — phones decode easily;
//!   leaves room for multiple QRs on screen.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use qrcodegen_no_heap::{QrCode, QrCodeEcc, Version};

use crate::framebuffer;

const QR_BUF_LEN: usize = Version::MAX.buffer_len();
// 3px was too dense for iPhone autofocus at arm's length. 4px ≈ 33% bigger
// modules, ~77% more area per module — scannable on phone cameras without
// eating much screen real estate.
const DEFAULT_PX_PER_MODULE: usize = 4;

/// Compress payloads larger than this many bytes (deflate + base64).
const COMPRESS_THRESHOLD: usize = 400;

/// Maximum bytes per QR chunk in split/multi mode. Smaller chunks = lower
/// QR version = fewer modules = more forgiving to scan. Two medium QRs beat
/// one version-40 giant every time on a phone camera.
const CHUNK_BYTE_LIMIT: usize = 1200;

/// Left-right spacing between QRs when multiple are rendered.
const QR_GAP_PX: usize = 16;

/// Alphabet for the alphanumeric QR mode: 0-9, A-Z, space, $, %, *, +, -, ., /, :.
/// Payloads containing only these chars get automatic density boost from
/// `encode_text`.
fn is_alphanumeric_safe(c: char) -> bool {
    matches!(c,
        '0'..='9' | 'A'..='Z' | ' ' | '$' | '%' | '*' | '+' | '-' | '.' | '/' | ':'
    )
}

/// Return true if every byte is inside the QR alphanumeric character set.
fn payload_is_alphanumeric(s: &str) -> bool {
    s.chars().all(is_alphanumeric_safe)
}

/// Compress with deflate + base64-encode. Return `Z:<b64>` so the receiver
/// can tell it's compressed. Falls back to the uncompressed string if the
/// compressed form is larger.
pub fn maybe_compress(data: &[u8]) -> String {
    if data.len() < COMPRESS_THRESHOLD {
        // Small payloads: don't bother.
        return match core::str::from_utf8(data) {
            Ok(s) => s.to_string(),
            Err(_) => base64_encode(data),
        };
    }
    let compressed = miniz_oxide::deflate::compress_to_vec(data, 8);
    if compressed.len() + 4 < data.len() {
        let mut s = String::from("Z:");
        s.push_str(&base64_encode(&compressed));
        s
    } else {
        match core::str::from_utf8(data) {
            Ok(u) => u.to_string(),
            Err(_) => base64_encode(data),
        }
    }
}

/// Tiny standard base64 encoder. Output only uses `A-Za-z0-9+/=` so receivers
/// with a standard base64 decoder (every language) can round-trip.
fn base64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    let chunks = input.chunks_exact(3);
    let rem = chunks.remainder();
    for c in chunks {
        let n = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32);
        out.push(T[((n >> 18) & 0x3F) as usize] as char);
        out.push(T[((n >> 12) & 0x3F) as usize] as char);
        out.push(T[((n >> 6) & 0x3F) as usize] as char);
        out.push(T[(n & 0x3F) as usize] as char);
    }
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(T[((n >> 18) & 0x3F) as usize] as char);
            out.push(T[((n >> 12) & 0x3F) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(T[((n >> 18) & 0x3F) as usize] as char);
            out.push(T[((n >> 12) & 0x3F) as usize] as char);
            out.push(T[((n >> 6) & 0x3F) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

/// Split `payload` into `CHUNK_BYTE_LIMIT`-sized slices, each prefixed
/// `idx/total|`. Receiver: sort by idx, concatenate, strip prefixes.
fn split_into_chunks(payload: &str) -> Vec<String> {
    let bytes = payload.as_bytes();
    if bytes.len() <= CHUNK_BYTE_LIMIT {
        return alloc::vec![payload.to_string()];
    }
    let total = (bytes.len() + CHUNK_BYTE_LIMIT - 1) / CHUNK_BYTE_LIMIT;
    let mut out = Vec::with_capacity(total);
    for (i, chunk) in bytes.chunks(CHUNK_BYTE_LIMIT).enumerate() {
        let mut s = alloc::format!("{}/{}|", i + 1, total);
        // str chunk may split a UTF-8 char — acceptable because the input is
        // ASCII for our debug payloads.
        s.push_str(core::str::from_utf8(chunk).unwrap_or(""));
        out.push(s);
    }
    out
}

/// Render `payload` as one or more QR codes on the framebuffer back buffer,
/// starting at top-right. If the payload is large it will be compressed
/// and/or split; the receiver pastes the concatenated decoded text.
pub fn render_payload(payload: &str) {
    let encoded = maybe_compress(payload.as_bytes());
    let chunks = split_into_chunks(&encoded);

    let (fb_w, _fb_h) = framebuffer::dimensions();
    let px_per_module = DEFAULT_PX_PER_MODULE;

    // Encode + draw each chunk in a single pass. We can't cheaply re-parse a
    // QrCode back out of its codeword buffer (the `qrcodegen-no-heap` API
    // doesn't expose that), so keep encode+draw in the same scope where the
    // QR borrows `out`.
    let y = 300usize;
    // Start left-of-center, lay out chunks left-to-right with a gap.
    // For a single chunk this is centered-ish; for N chunks they trail to
    // the right but within the framebuffer.
    let mut x = fb_w / 2;
    let mut rendered = 0usize;
    for chunk in &chunks {
        let mut temp = [0u8; QR_BUF_LEN];
        let mut out = [0u8; QR_BUF_LEN];
        match QrCode::encode_text(
            chunk,
            &mut temp,
            &mut out,
            QrCodeEcc::Low,
            Version::MIN,
            Version::MAX,
            None,
            true,
        ) {
            Ok(qr) => {
                let size = qr.size() as usize;
                let border = 4usize;
                let total_px = (size + 2 * border) * px_per_module;
                // Clamp x so QR stays on screen.
                if x + total_px + 20 > fb_w {
                    x = fb_w.saturating_sub(total_px + 20);
                }
                draw_qr_at(&qr, x, y, px_per_module);
                x += total_px + QR_GAP_PX;
                rendered += 1;
            }
            Err(_) => {
                log::warn!("[qr] chunk {}B too large to encode, skipping", chunk.len());
            }
        }
    }

    log::info!(
        "[qr] rendered {}/{} QR(s) at y={} (payload {}B -> {}B after compress/b64)",
        rendered,
        chunks.len(),
        y,
        payload.len(),
        encoded.len(),
    );
}

fn draw_qr_at(qr: &QrCode, origin_x: usize, origin_y: usize, px_per_module: usize) {
    let size = qr.size() as usize;
    let border = 4usize;
    let total_modules = size + 2 * border;
    let total_px = total_modules * px_per_module;
    framebuffer::fill_rect(origin_x, origin_y, total_px, total_px, (255, 255, 255));
    for y in 0..size {
        for x in 0..size {
            if qr.get_module(x as i32, y as i32) {
                let px_x = origin_x + (border + x) * px_per_module;
                let px_y = origin_y + (border + y) * px_per_module;
                framebuffer::fill_rect(px_x, px_y, px_per_module, px_per_module, (0, 0, 0));
            }
        }
    }
}

/// Legacy entry point — bytes get stringified (lossy for non-UTF-8 but our
/// debug payloads are always ASCII) then passed to `render_payload`.
pub fn render_top_right(data: &[u8]) {
    let s = match core::str::from_utf8(data) {
        Ok(s) => s.to_string(),
        Err(_) => base64_encode(data),
    };
    render_payload(&s);
}
