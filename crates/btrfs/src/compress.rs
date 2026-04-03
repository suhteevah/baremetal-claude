//! Decompression support for btrfs compressed extents.
//!
//! btrfs supports three compression types:
//! - **ZLIB** (type 1): DEFLATE compression wrapped in a zlib header (RFC 1950/1951).
//! - **LZO** (type 2): LZO1X compression with btrfs-specific framing.
//! - **ZSTD** (type 3): Zstandard compression (complex, stubbed for now).
//!
//! Compressed extents store `disk_num_bytes` of compressed data on disk, which
//! decompresses to `ram_bytes` of uncompressed data.

use alloc::vec;
use alloc::vec::Vec;

use crate::extent::{BTRFS_COMPRESS_NONE, BTRFS_COMPRESS_ZLIB, BTRFS_COMPRESS_LZO, BTRFS_COMPRESS_ZSTD};

/// Errors during decompression.
#[derive(Debug)]
pub enum DecompressError {
    /// Unsupported compression type.
    UnsupportedType(u8),
    /// Compressed data is malformed.
    InvalidData(&'static str),
    /// Output buffer too small.
    OutputTooSmall,
    /// ZSTD not yet implemented.
    ZstdNotImplemented,
}

/// Decompress data based on the btrfs compression type byte.
///
/// `compressed` is the raw bytes read from disk.
/// `output_size` is the expected decompressed size (`ram_bytes` from the extent item).
pub fn decompress(compression_type: u8, compressed: &[u8], output_size: usize) -> Result<Vec<u8>, DecompressError> {
    match compression_type {
        BTRFS_COMPRESS_NONE => {
            // No compression; just return the data.
            Ok(compressed.to_vec())
        }
        BTRFS_COMPRESS_ZLIB => {
            log::debug!("[btrfs::compress] decompressing zlib: {} compressed -> {} expected",
                compressed.len(), output_size);
            decompress_zlib(compressed, output_size)
        }
        BTRFS_COMPRESS_LZO => {
            log::debug!("[btrfs::compress] decompressing lzo: {} compressed -> {} expected",
                compressed.len(), output_size);
            decompress_lzo(compressed, output_size)
        }
        BTRFS_COMPRESS_ZSTD => {
            log::warn!("[btrfs::compress] ZSTD decompression not yet implemented");
            Err(DecompressError::ZstdNotImplemented)
        }
        other => {
            log::error!("[btrfs::compress] unknown compression type: {}", other);
            Err(DecompressError::UnsupportedType(other))
        }
    }
}

// ============================================================================
// ZLIB / DEFLATE decompressor
// ============================================================================

/// Decompress zlib-wrapped DEFLATE data (RFC 1950 header + RFC 1951 DEFLATE stream).
fn decompress_zlib(data: &[u8], output_size: usize) -> Result<Vec<u8>, DecompressError> {
    // Zlib header: 2 bytes (CMF + FLG), optionally 4 bytes DICTID, then DEFLATE stream, then 4 bytes Adler32.
    if data.len() < 2 {
        return Err(DecompressError::InvalidData("zlib header too short"));
    }

    let cmf = data[0];
    let flg = data[1];

    // Verify header checksum
    if (cmf as u16 * 256 + flg as u16) % 31 != 0 {
        return Err(DecompressError::InvalidData("zlib header checksum failed"));
    }

    // CM (compression method) is the lower 4 bits of CMF. Value 8 = DEFLATE,
    // which is the only compression method defined for zlib.
    let cm = cmf & 0x0F;
    if cm != 8 {
        return Err(DecompressError::InvalidData("zlib: compression method not DEFLATE"));
    }

    // FDICT (bit 5 of FLG): if set, a 4-byte preset dictionary ID follows
    // the header. The DEFLATE stream starts after the header + optional DICTID.
    let fdict = (flg >> 5) & 1;
    let deflate_start = if fdict != 0 { 6 } else { 2 };

    if data.len() < deflate_start {
        return Err(DecompressError::InvalidData("zlib data too short for header"));
    }

    deflate_decompress(&data[deflate_start..], output_size)
}

/// DEFLATE decompressor (RFC 1951).
///
/// Supports all three block types:
/// - Type 0: Stored (uncompressed)
/// - Type 1: Fixed Huffman codes
/// - Type 2: Dynamic Huffman codes
fn deflate_decompress(data: &[u8], output_size: usize) -> Result<Vec<u8>, DecompressError> {
    let mut output = Vec::with_capacity(output_size);
    let mut reader = BitReader::new(data);

    loop {
        let bfinal = reader.read_bits(1).ok_or(DecompressError::InvalidData("truncated DEFLATE: bfinal"))?;
        let btype = reader.read_bits(2).ok_or(DecompressError::InvalidData("truncated DEFLATE: btype"))?;

        match btype {
            0 => {
                // Stored block: skip to byte boundary, read LEN/NLEN, copy raw
                reader.align_to_byte();
                let len = reader.read_u16_le().ok_or(DecompressError::InvalidData("stored block: truncated LEN"))?;
                let nlen = reader.read_u16_le().ok_or(DecompressError::InvalidData("stored block: truncated NLEN"))?;
                if len != !nlen {
                    return Err(DecompressError::InvalidData("stored block: LEN/NLEN mismatch"));
                }
                for _ in 0..len {
                    let b = reader.read_byte().ok_or(DecompressError::InvalidData("stored block: truncated data"))?;
                    output.push(b);
                }
            }
            1 => {
                // Fixed Huffman codes
                decode_huffman_block(&mut reader, &mut output, true)?;
            }
            2 => {
                // Dynamic Huffman codes
                decode_huffman_block(&mut reader, &mut output, false)?;
            }
            _ => {
                return Err(DecompressError::InvalidData("DEFLATE: reserved block type 3"));
            }
        }

        if bfinal != 0 {
            break;
        }
    }

    // Truncate or pad to expected size
    output.truncate(output_size);
    Ok(output)
}

/// Decode a Huffman-coded DEFLATE block (type 1 = fixed, type 2 = dynamic).
fn decode_huffman_block(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    fixed: bool,
) -> Result<(), DecompressError> {
    // Build code tables
    let (lit_lengths, dist_lengths) = if fixed {
        build_fixed_tables()
    } else {
        build_dynamic_tables(reader)?
    };

    let lit_table = HuffmanTable::build(&lit_lengths)
        .ok_or(DecompressError::InvalidData("failed to build literal/length table"))?;
    let dist_table = HuffmanTable::build(&dist_lengths)
        .ok_or(DecompressError::InvalidData("failed to build distance table"))?;

    loop {
        let sym = lit_table.decode(reader)
            .ok_or(DecompressError::InvalidData("truncated Huffman literal/length"))?;

        if sym < 256 {
            // Literal byte
            output.push(sym as u8);
        } else if sym == 256 {
            // End of block
            break;
        } else {
            // Length/distance pair
            let length = decode_length(sym, reader)?;
            let dist_sym = dist_table.decode(reader)
                .ok_or(DecompressError::InvalidData("truncated Huffman distance"))?;
            let distance = decode_distance(dist_sym, reader)?;

            if distance as usize > output.len() {
                return Err(DecompressError::InvalidData("DEFLATE: distance exceeds output"));
            }

            // Copy from back-reference (byte by byte for overlapping copies)
            let start = output.len() - distance as usize;
            for i in 0..length as usize {
                let b = output[start + (i % distance as usize)];
                output.push(b);
            }
        }
    }

    Ok(())
}

/// Build fixed Huffman code lengths (RFC 1951 section 3.2.6).
fn build_fixed_tables() -> (Vec<u8>, Vec<u8>) {
    let mut lit_lengths = vec![0u8; 288];
    for i in 0..=143 { lit_lengths[i] = 8; }
    for i in 144..=255 { lit_lengths[i] = 9; }
    for i in 256..=279 { lit_lengths[i] = 7; }
    for i in 280..=287 { lit_lengths[i] = 8; }

    let dist_lengths = vec![5u8; 32];

    (lit_lengths, dist_lengths)
}

/// Build dynamic Huffman tables from the stream header.
fn build_dynamic_tables(reader: &mut BitReader<'_>) -> Result<(Vec<u8>, Vec<u8>), DecompressError> {
    let hlit = reader.read_bits(5).ok_or(DecompressError::InvalidData("dyn: truncated HLIT"))? as usize + 257;
    let hdist = reader.read_bits(5).ok_or(DecompressError::InvalidData("dyn: truncated HDIST"))? as usize + 1;
    let hclen = reader.read_bits(4).ok_or(DecompressError::InvalidData("dyn: truncated HCLEN"))? as usize + 4;

    // Code length code lengths arrive in a permuted order defined by RFC 1951.
    // This order puts the most likely-to-be-zero lengths last, allowing the
    // encoder to omit trailing zeros and use fewer HCLEN entries.
    const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
    let mut cl_lengths = [0u8; 19];
    for i in 0..hclen {
        cl_lengths[CL_ORDER[i]] = reader.read_bits(3)
            .ok_or(DecompressError::InvalidData("dyn: truncated code length"))? as u8;
    }

    let cl_table = HuffmanTable::build(&cl_lengths)
        .ok_or(DecompressError::InvalidData("dyn: failed to build CL table"))?;

    // Decode the literal/length + distance code lengths
    let total = hlit + hdist;
    let mut lengths = vec![0u8; total];
    let mut i = 0;
    while i < total {
        let sym = cl_table.decode(reader)
            .ok_or(DecompressError::InvalidData("dyn: truncated code length stream"))?;

        if sym < 16 {
            lengths[i] = sym as u8;
            i += 1;
        } else if sym == 16 {
            // Repeat previous length 3-6 times
            let repeat = reader.read_bits(2).ok_or(DecompressError::InvalidData("dyn: truncated repeat"))? + 3;
            if i == 0 {
                return Err(DecompressError::InvalidData("dyn: repeat with no previous"));
            }
            let prev = lengths[i - 1];
            for _ in 0..repeat {
                if i >= total { break; }
                lengths[i] = prev;
                i += 1;
            }
        } else if sym == 17 {
            // Repeat 0 for 3-10 times
            let repeat = reader.read_bits(3).ok_or(DecompressError::InvalidData("dyn: truncated zero repeat"))? + 3;
            for _ in 0..repeat {
                if i >= total { break; }
                lengths[i] = 0;
                i += 1;
            }
        } else if sym == 18 {
            // Repeat 0 for 11-138 times
            let repeat = reader.read_bits(7).ok_or(DecompressError::InvalidData("dyn: truncated long zero repeat"))? + 11;
            for _ in 0..repeat {
                if i >= total { break; }
                lengths[i] = 0;
                i += 1;
            }
        } else {
            return Err(DecompressError::InvalidData("dyn: invalid CL symbol"));
        }
    }

    let lit_lengths = lengths[..hlit].to_vec();
    let dist_lengths = lengths[hlit..].to_vec();

    Ok((lit_lengths, dist_lengths))
}

/// Decode a length value from a literal/length symbol (257-285).
fn decode_length(sym: u16, reader: &mut BitReader<'_>) -> Result<u16, DecompressError> {
    // RFC 1951 Table: length codes 257-285
    const BASE: [u16; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31,
        35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
    ];
    const EXTRA: [u8; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2,
        3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];

    let idx = (sym - 257) as usize;
    if idx >= 29 {
        return Err(DecompressError::InvalidData("invalid length code"));
    }
    let base = BASE[idx];
    let extra_bits = EXTRA[idx];
    let extra = if extra_bits > 0 {
        reader.read_bits(extra_bits as u32).ok_or(DecompressError::InvalidData("truncated length extra"))? as u16
    } else {
        0
    };
    Ok(base + extra)
}

/// Decode a distance value from a distance symbol (0-29).
fn decode_distance(sym: u16, reader: &mut BitReader<'_>) -> Result<u32, DecompressError> {
    const BASE: [u32; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193,
        257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    const EXTRA: [u8; 30] = [
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6,
        7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
    ];

    let idx = sym as usize;
    if idx >= 30 {
        return Err(DecompressError::InvalidData("invalid distance code"));
    }
    let base = BASE[idx];
    let extra_bits = EXTRA[idx];
    let extra = if extra_bits > 0 {
        reader.read_bits(extra_bits as u32).ok_or(DecompressError::InvalidData("truncated distance extra"))? as u32
    } else {
        0
    };
    Ok(base + extra)
}

/// Bit reader for the DEFLATE stream. Reads bits in LSB-first order.
///
/// DEFLATE packs bits starting from the least significant bit of each byte.
/// For example, reading 3 bits from byte 0b_abcdefgh yields bits h, g, f
/// (in that order, as the least significant bits of the result).
struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u32, // 0-7, bits consumed in current byte
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, byte_pos: 0, bit_pos: 0 }
    }

    /// Read `n` bits (1-16) LSB-first. Returns None on EOF.
    fn read_bits(&mut self, n: u32) -> Option<u32> {
        let mut result = 0u32;
        for i in 0..n {
            if self.byte_pos >= self.data.len() {
                return None;
            }
            let bit = ((self.data[self.byte_pos] >> self.bit_pos) & 1) as u32;
            result |= bit << i;
            self.bit_pos += 1;
            if self.bit_pos >= 8 {
                self.bit_pos = 0;
                self.byte_pos += 1;
            }
        }
        Some(result)
    }

    /// Align to the next byte boundary.
    fn align_to_byte(&mut self) {
        if self.bit_pos > 0 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
    }

    /// Read a raw byte (after aligning).
    fn read_byte(&mut self) -> Option<u8> {
        if self.bit_pos == 0 {
            if self.byte_pos >= self.data.len() {
                return None;
            }
            let b = self.data[self.byte_pos];
            self.byte_pos += 1;
            Some(b)
        } else {
            // Read 8 bits
            self.read_bits(8).map(|v| v as u8)
        }
    }

    /// Read a 16-bit little-endian value (after aligning).
    fn read_u16_le(&mut self) -> Option<u16> {
        let lo = self.read_byte()? as u16;
        let hi = self.read_byte()? as u16;
        Some(lo | (hi << 8))
    }
}

/// A simple canonical Huffman decoding table.
///
/// Supports code lengths up to 15 bits (DEFLATE max). Canonical Huffman codes
/// are uniquely determined by the set of code lengths: shorter codes are
/// numerically smaller, and codes of the same length are assigned in symbol order.
/// This table stores the minimum code value and symbol index for each code length,
/// enabling O(max_code_length) decoding per symbol.
struct HuffmanTable {
    /// For each code length (1..=15), store the starting code value and the
    /// first symbol index.
    /// Index 0 is unused; index `len` = (min_code, symbol_start_index).
    min_code: [u32; 16],
    max_code: [i32; 16], // -1 if no codes of this length
    /// Symbol lookup: symbols sorted by code length, then by code value.
    symbols: Vec<u16>,
}

impl HuffmanTable {
    /// Build a Huffman decoding table from code lengths.
    ///
    /// `lengths[i]` is the code length for symbol `i`. 0 means the symbol is not used.
    fn build(lengths: &[u8]) -> Option<Self> {
        let max_len = *lengths.iter().max()? as usize;
        if max_len == 0 || max_len > 15 {
            // All zeros or invalid length -- handle gracefully
            if max_len == 0 {
                // No symbols - create a dummy table
                return Some(HuffmanTable {
                    min_code: [0; 16],
                    max_code: [-1; 16],
                    symbols: Vec::new(),
                });
            }
            return None;
        }

        // Count codes of each length
        let mut bl_count = [0u32; 16];
        for &l in lengths {
            if l > 0 {
                bl_count[l as usize] += 1;
            }
        }

        // Compute the starting code for each bit length
        let mut next_code = [0u32; 16];
        let mut code = 0u32;
        for bits in 1..=max_len {
            code = (code + bl_count[bits - 1]) << 1;
            next_code[bits] = code;
        }

        // Assign symbols sorted by (length, then symbol index)
        let num_symbols: u32 = bl_count.iter().sum();
        let mut symbols = Vec::with_capacity(num_symbols as usize);
        let mut min_code_arr = [0u32; 16];
        let mut max_code_arr = [-1i32; 16];
        let mut symbol_offset = [0u32; 16];

        // First pass: compute offsets
        let mut offset = 0u32;
        for bits in 1..=15 {
            symbol_offset[bits] = offset;
            min_code_arr[bits] = next_code[bits];
            if bl_count[bits] > 0 {
                max_code_arr[bits] = (next_code[bits] + bl_count[bits] - 1) as i32;
            }
            offset += bl_count[bits];
        }

        symbols.resize(offset as usize, 0u16);

        // Second pass: fill symbols
        let mut code_count = [0u32; 16];
        for (sym, &l) in lengths.iter().enumerate() {
            if l > 0 {
                let idx = symbol_offset[l as usize] + code_count[l as usize];
                if (idx as usize) < symbols.len() {
                    symbols[idx as usize] = sym as u16;
                }
                code_count[l as usize] += 1;
            }
        }

        Some(HuffmanTable { min_code: min_code_arr, max_code: max_code_arr, symbols })
    }

    /// Decode one symbol from the bit stream.
    fn decode(&self, reader: &mut BitReader<'_>) -> Option<u16> {
        let mut code = 0u32;
        for bits in 1..=15u32 {
            let bit = reader.read_bits(1)?;
            code = (code << 1) | bit;
            if self.max_code[bits as usize] >= 0 && code <= self.max_code[bits as usize] as u32 {
                let index = code - self.min_code[bits as usize];
                let mut offset = 0u32;
                for b in 1..bits {
                    if self.max_code[b as usize] >= 0 {
                        offset += (self.max_code[b as usize] as u32) - self.min_code[b as usize] + 1;
                    }
                }
                let sym_idx = (offset + index) as usize;
                return self.symbols.get(sym_idx).copied();
            }
        }
        None
    }
}

// ============================================================================
// LZO decompressor (LZO1X for btrfs)
// ============================================================================

/// Decompress btrfs LZO-compressed data.
///
/// btrfs LZO format: 4-byte LE total compressed size, then one or more segments.
/// Each segment: 4-byte LE compressed size, then compressed data.
/// Each segment decompresses independently using LZO1X.
fn decompress_lzo(data: &[u8], output_size: usize) -> Result<Vec<u8>, DecompressError> {
    if data.len() < 4 {
        return Err(DecompressError::InvalidData("LZO: too short for header"));
    }

    // btrfs stores total compressed length as first 4 bytes (LE).
    let total_compressed = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let _ = total_compressed; // informational

    let mut output = Vec::with_capacity(output_size);
    let mut pos = 4; // skip the total length header

    // Process segments
    while pos + 4 <= data.len() && output.len() < output_size {
        let seg_len = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;

        if seg_len == 0 || pos + seg_len > data.len() {
            break;
        }

        let segment = &data[pos..pos + seg_len];
        lzo1x_decompress_segment(segment, &mut output, output_size)?;
        pos += seg_len;

        // btrfs aligns LZO segments to 4-byte boundaries within the compressed
        // data stream. The `& !3` mask rounds up to the next 4-byte boundary.
        // Some btrfs versions align to page boundaries instead, but 4-byte
        // alignment is the minimum required by the on-disk format.
        pos = (pos + 3) & !3;
    }

    output.truncate(output_size);
    Ok(output)
}

/// Decompress a single LZO1X segment.
///
/// LZO1X is a simple LZ77 variant with a specific command encoding.
fn lzo1x_decompress_segment(
    input: &[u8],
    output: &mut Vec<u8>,
    max_output: usize,
) -> Result<(), DecompressError> {
    let mut ip = 0usize; // input position

    if input.is_empty() {
        return Ok(());
    }

    // First byte determines initial state
    let mut t = input[0] as usize;
    ip += 1;

    if t > 17 {
        // Copy (t - 17) literal bytes
        let n = t - 17;
        for _ in 0..n {
            if ip >= input.len() || output.len() >= max_output {
                return Ok(());
            }
            output.push(input[ip]);
            ip += 1;
        }
        // Fall through to main loop
        t = if ip < input.len() { input[ip] as usize } else { return Ok(()) };
        ip += 1;
        if t < 16 {
            return Ok(());
        }
    }

    loop {
        if output.len() >= max_output || ip >= input.len() {
            return Ok(());
        }

        if t >= 64 {
            // Copy 2 bytes with short match
            let length = 1 + ((t >> 5) & 7);
            let high = (t & 31) as usize;
            if ip >= input.len() { return Ok(()); }
            let dist = (high << 3) | ((input[ip] as usize) >> 2);
            ip += 1;
            let m_off = dist + 1;

            if m_off > output.len() {
                return Err(DecompressError::InvalidData("LZO: match offset exceeds output"));
            }

            let start = output.len() - m_off;
            for i in 0..length {
                if output.len() >= max_output { return Ok(()); }
                let b = output[start + (i % m_off)];
                output.push(b);
            }

            t = if ip < input.len() { input[ip] as usize } else { return Ok(()) };
            ip += 1;
        } else if t >= 32 {
            // Long match
            let mut length = (t & 31) as usize;
            if length == 0 {
                // Read additional length bytes
                while ip < input.len() && input[ip] == 0 {
                    length += 255;
                    ip += 1;
                }
                if ip >= input.len() { return Ok(()); }
                length += 31 + input[ip] as usize;
                ip += 1;
            }
            length += 2;

            if ip + 1 >= input.len() { return Ok(()); }
            let dist = ((input[ip] as usize) | ((input[ip + 1] as usize) << 8)) >> 2;
            ip += 2;
            let m_off = dist + 1;

            if m_off > output.len() {
                return Err(DecompressError::InvalidData("LZO: match offset exceeds output"));
            }

            let start = output.len() - m_off;
            for i in 0..length {
                if output.len() >= max_output { return Ok(()); }
                let b = output[start + (i % m_off)];
                output.push(b);
            }

            t = if ip < input.len() { input[ip] as usize } else { return Ok(()) };
            ip += 1;
        } else if t >= 16 {
            // Medium match or end marker
            let mut length = (t & 7) as usize;
            if length == 0 {
                while ip < input.len() && input[ip] == 0 {
                    length += 255;
                    ip += 1;
                }
                if ip >= input.len() { return Ok(()); }
                length += 7 + input[ip] as usize;
                ip += 1;
            }
            length += 2;

            if ip + 1 >= input.len() { return Ok(()); }
            let dist_lo = input[ip] as usize;
            let dist_hi = input[ip + 1] as usize;
            ip += 2;

            let m_off = ((t & 8) << 11) | (dist_hi << 6) | (dist_lo >> 2);
            if m_off == 0 {
                // End of stream marker
                return Ok(());
            }
            let m_off = m_off + 0x4000;

            if m_off > output.len() {
                return Err(DecompressError::InvalidData("LZO: match offset exceeds output"));
            }

            let start = output.len() - m_off;
            for i in 0..length {
                if output.len() >= max_output { return Ok(()); }
                let b = output[start + (i % m_off)];
                output.push(b);
            }

            t = if ip < input.len() { input[ip] as usize } else { return Ok(()) };
            ip += 1;
        } else {
            // Literal run + short match
            // t < 16: copy (3 + t) literal bytes, then get next instruction
            // But this is also the initial literal handling
            let dist = (1 + (0x0800 | ((t & 3) << 8))) + if ip < input.len() {
                let v = (input[ip] as usize) >> 2;
                ip += 1;
                v
            } else {
                return Ok(());
            };

            if dist > output.len() {
                // Treat as literal copy: (t + 3) bytes
                let n = t + 3;
                // Rewind ip by 1 since we consumed a byte for dist
                if ip > 0 { ip -= 1; }
                for _ in 0..n {
                    if ip >= input.len() || output.len() >= max_output { return Ok(()); }
                    output.push(input[ip]);
                    ip += 1;
                }
                t = if ip < input.len() { input[ip] as usize } else { return Ok(()) };
                ip += 1;
            } else {
                let length = 2;
                let start = output.len() - dist;
                for i in 0..length {
                    if output.len() >= max_output { return Ok(()); }
                    let b = output[start + (i % dist)];
                    output.push(b);
                }

                t = if ip < input.len() { input[ip] as usize } else { return Ok(()) };
                ip += 1;
            }
        }

        // After a match, copy trailing literals based on low 2 bits of last consumed byte
        // The previous instruction's last byte low 2 bits
        if ip > 0 {
            let trail = (input[ip - 1] & 3) as usize;
            if trail > 0 {
                for _ in 0..trail {
                    if ip >= input.len() || output.len() >= max_output {
                        return Ok(());
                    }
                    output.push(input[ip]);
                    ip += 1;
                }
                t = if ip < input.len() { input[ip] as usize } else { return Ok(()) };
                ip += 1;
            } else if t >= 16 {
                continue;
            } else {
                // t < 16: literal run
                let n = t + 3;
                for _ in 0..n {
                    if ip >= input.len() || output.len() >= max_output {
                        return Ok(());
                    }
                    output.push(input[ip]);
                    ip += 1;
                }
                t = if ip < input.len() { input[ip] as usize } else { return Ok(()) };
                ip += 1;
            }
        }
    }
}
