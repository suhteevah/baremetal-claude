//! GGUF (GGML Universal Format) parser for quantized LLM model files.
//!
//! Parses the binary GGUF format used by llama.cpp and compatible tools.
//! Fully `no_std` — uses only `alloc` collections.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// GGUF magic number: "GGUF" in little-endian = 0x46475547
const GGUF_MAGIC: u32 = 0x4647_5547; // "GGUF" in little-endian

/// Default alignment for the data section (bytes).
const DEFAULT_ALIGNMENT: usize = 32;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct GgufError(String);

impl fmt::Display for GgufError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn err(msg: &str) -> GgufError {
    GgufError(String::from(msg))
}

// ---------------------------------------------------------------------------
// GGMLType — quantization formats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum GgmlType {
    F32  = 0,
    F16  = 1,
    Q4_0 = 2,
    Q4_1 = 3,
    Q5_0 = 6,
    Q5_1 = 7,
    Q8_0 = 8,
}

impl GgmlType {
    /// Parse a raw u32 into a `GgmlType`.
    pub fn from_u32(v: u32) -> Result<Self, GgufError> {
        match v {
            0 => Ok(GgmlType::F32),
            1 => Ok(GgmlType::F16),
            2 => Ok(GgmlType::Q4_0),
            3 => Ok(GgmlType::Q4_1),
            6 => Ok(GgmlType::Q5_0),
            7 => Ok(GgmlType::Q5_1),
            8 => Ok(GgmlType::Q8_0),
            _ => Err(err("unsupported GgmlType")),
        }
    }

    /// Number of elements per quantization block.
    pub fn block_size(&self) -> usize {
        match self {
            GgmlType::F32  => 1,
            GgmlType::F16  => 1,
            GgmlType::Q4_0 => 32,
            GgmlType::Q4_1 => 32,
            GgmlType::Q5_0 => 32,
            GgmlType::Q5_1 => 32,
            GgmlType::Q8_0 => 32,
        }
    }

    /// Bytes per quantization block.
    pub fn type_size(&self) -> usize {
        match self {
            GgmlType::F32  => 4,
            GgmlType::F16  => 2,
            GgmlType::Q4_0 => 18,  // 1×f16 scale + 16 bytes of 4-bit values
            GgmlType::Q4_1 => 20,  // 1×f16 scale + 1×f16 min + 16 bytes
            GgmlType::Q5_0 => 22,
            GgmlType::Q5_1 => 24,
            GgmlType::Q8_0 => 34,  // 1×f16 scale + 32×i8
        }
    }
}

// ---------------------------------------------------------------------------
// GgufValueType
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum GgufValueType {
    Uint8   = 0,
    Int8    = 1,
    Uint16  = 2,
    Int16   = 3,
    Uint32  = 4,
    Int32   = 5,
    Float32 = 6,
    Bool    = 7,
    String  = 8,
    Array   = 9,
    Uint64  = 10,
    Int64   = 11,
    Float64 = 12,
}

impl GgufValueType {
    fn from_u32(v: u32) -> Result<Self, GgufError> {
        match v {
            0  => Ok(GgufValueType::Uint8),
            1  => Ok(GgufValueType::Int8),
            2  => Ok(GgufValueType::Uint16),
            3  => Ok(GgufValueType::Int16),
            4  => Ok(GgufValueType::Uint32),
            5  => Ok(GgufValueType::Int32),
            6  => Ok(GgufValueType::Float32),
            7  => Ok(GgufValueType::Bool),
            8  => Ok(GgufValueType::String),
            9  => Ok(GgufValueType::Array),
            10 => Ok(GgufValueType::Uint64),
            11 => Ok(GgufValueType::Int64),
            12 => Ok(GgufValueType::Float64),
            _  => Err(err("unsupported GgufValueType")),
        }
    }
}

// ---------------------------------------------------------------------------
// GgufValue
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum GgufValue {
    Uint8(u8),
    Int8(i8),
    Uint16(u16),
    Int16(i16),
    Uint32(u32),
    Int32(i32),
    Float32(f32),
    Bool(bool),
    String(String),
    Array(Vec<GgufValue>),
    Uint64(u64),
    Int64(i64),
    Float64(f64),
}

// ---------------------------------------------------------------------------
// TensorInfo
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TensorInfo {
    pub name: String,
    pub shape: Vec<u64>,
    pub dtype: GgmlType,
    pub offset: u64,
}

impl TensorInfo {
    /// Total number of elements in this tensor.
    pub fn n_elements(&self) -> u64 {
        self.shape.iter().copied().product()
    }

    /// Total byte size of the tensor data.
    pub fn byte_size(&self) -> usize {
        let elems = self.n_elements() as usize;
        let bs = self.dtype.block_size();
        let ts = self.dtype.type_size();
        // Quantized types pack multiple elements per block; round up for partial blocks
        let n_blocks = (elems + bs - 1) / bs;
        n_blocks * ts
    }
}

// ---------------------------------------------------------------------------
// Cursor — little-endian binary reader
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], GgufError> {
        if self.pos + n > self.data.len() {
            return Err(err("unexpected end of data"));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, GgufError> {
        let b = self.read_bytes(1)?;
        Ok(b[0])
    }

    fn read_i8(&mut self) -> Result<i8, GgufError> {
        Ok(self.read_u8()? as i8)
    }

    fn read_u16(&mut self) -> Result<u16, GgufError> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_i16(&mut self) -> Result<i16, GgufError> {
        let b = self.read_bytes(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, GgufError> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i32(&mut self) -> Result<i32, GgufError> {
        let b = self.read_bytes(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_f32(&mut self) -> Result<f32, GgufError> {
        let b = self.read_bytes(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u64(&mut self) -> Result<u64, GgufError> {
        let b = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_i64(&mut self) -> Result<i64, GgufError> {
        let b = self.read_bytes(8)?;
        Ok(i64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_f64(&mut self) -> Result<f64, GgufError> {
        let b = self.read_bytes(8)?;
        Ok(f64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_string(&mut self) -> Result<String, GgufError> {
        let len = self.read_u64()? as usize;
        let bytes = self.read_bytes(len)?;
        let s = core::str::from_utf8(bytes).map_err(|_| err("invalid UTF-8 in string"))?;
        Ok(String::from(s))
    }

    /// Align position forward to the next multiple of `alignment`.
    fn align_to(&mut self, alignment: usize) {
        let rem = self.pos % alignment;
        if rem != 0 {
            self.pos += alignment - rem;
        }
    }
}

// ---------------------------------------------------------------------------
// Value parsing
// ---------------------------------------------------------------------------

fn read_value(cursor: &mut Cursor<'_>, vtype: GgufValueType) -> Result<GgufValue, GgufError> {
    match vtype {
        GgufValueType::Uint8   => Ok(GgufValue::Uint8(cursor.read_u8()?)),
        GgufValueType::Int8    => Ok(GgufValue::Int8(cursor.read_i8()?)),
        GgufValueType::Uint16  => Ok(GgufValue::Uint16(cursor.read_u16()?)),
        GgufValueType::Int16   => Ok(GgufValue::Int16(cursor.read_i16()?)),
        GgufValueType::Uint32  => Ok(GgufValue::Uint32(cursor.read_u32()?)),
        GgufValueType::Int32   => Ok(GgufValue::Int32(cursor.read_i32()?)),
        GgufValueType::Float32 => Ok(GgufValue::Float32(cursor.read_f32()?)),
        GgufValueType::Bool    => Ok(GgufValue::Bool(cursor.read_u8()? != 0)),
        GgufValueType::String  => Ok(GgufValue::String(cursor.read_string()?)),
        GgufValueType::Uint64  => Ok(GgufValue::Uint64(cursor.read_u64()?)),
        GgufValueType::Int64   => Ok(GgufValue::Int64(cursor.read_i64()?)),
        GgufValueType::Float64 => Ok(GgufValue::Float64(cursor.read_f64()?)),
        GgufValueType::Array   => {
            let elem_type = GgufValueType::from_u32(cursor.read_u32()?)?;
            let count = cursor.read_u64()? as usize;
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                items.push(read_value(cursor, elem_type)?);
            }
            Ok(GgufValue::Array(items))
        }
    }
}

// ---------------------------------------------------------------------------
// GgufFile
// ---------------------------------------------------------------------------

/// Parsed GGUF file. Holds metadata, tensor descriptors, and a reference to
/// the raw data section for zero-copy tensor access.
#[derive(Debug)]
pub struct GgufFile<'a> {
    pub version: u32,
    pub metadata: BTreeMap<String, GgufValue>,
    pub tensors: Vec<TensorInfo>,
    /// Raw tensor data section (slice into the original buffer).
    pub data: &'a [u8],
    /// Alignment used for the data section (from metadata or default 32).
    pub alignment: usize,
}

impl<'a> GgufFile<'a> {
    /// Parse a complete GGUF file from a byte slice.
    ///
    /// The returned `GgufFile` borrows into `data` for the tensor data section,
    /// avoiding a copy of potentially multi-gigabyte weight blobs.
    pub fn parse(data: &'a [u8]) -> Result<Self, GgufError> {
        let mut cursor = Cursor::new(data);

        // --- Header ---
        let magic = cursor.read_u32()?;
        if magic != GGUF_MAGIC {
            return Err(err("invalid GGUF magic number"));
        }

        let version = cursor.read_u32()?;
        if version < 2 || version > 3 {
            return Err(err("unsupported GGUF version (expected 2 or 3)"));
        }

        let tensor_count = cursor.read_u64()? as usize;
        let metadata_kv_count = cursor.read_u64()? as usize;

        log::debug!(
            "GGUF v{}: {} tensors, {} metadata entries",
            version,
            tensor_count,
            metadata_kv_count
        );

        // --- Metadata ---
        let mut metadata = BTreeMap::new();
        for _ in 0..metadata_kv_count {
            let key = cursor.read_string()?;
            let vtype = GgufValueType::from_u32(cursor.read_u32()?)?;
            let value = read_value(&mut cursor, vtype)?;
            log::trace!("metadata: {} = {:?}", key, value);
            metadata.insert(key, value);
        }

        // Determine alignment from metadata (default 32).
        let alignment = match metadata.get("general.alignment") {
            Some(GgufValue::Uint32(a)) => *a as usize,
            Some(GgufValue::Uint64(a)) => *a as usize,
            Some(GgufValue::Int32(a)) if *a > 0 => *a as usize,
            _ => DEFAULT_ALIGNMENT,
        };

        // --- Tensor infos ---
        let mut tensors = Vec::with_capacity(tensor_count);
        for _ in 0..tensor_count {
            let name = cursor.read_string()?;
            let n_dims = cursor.read_u32()? as usize;
            let mut shape = Vec::with_capacity(n_dims);
            for _ in 0..n_dims {
                shape.push(cursor.read_u64()?);
            }
            let dtype = GgmlType::from_u32(cursor.read_u32()?)?;
            let offset = cursor.read_u64()?;

            log::trace!(
                "tensor: {} shape={:?} dtype={:?} offset={}",
                name,
                shape,
                dtype,
                offset
            );

            tensors.push(TensorInfo {
                name,
                shape,
                dtype,
                offset,
            });
        }

        // --- Data section ---
        // The GGUF spec requires tensor data to begin at the next alignment
        // boundary after the header+metadata+tensor_info region. Tensor offsets
        // are relative to this aligned start position, not the file start.
        cursor.align_to(alignment);
        let data_start = cursor.pos;

        // Gracefully handle truncated files (e.g. header-only parsing)
        let data_section = if data_start <= data.len() {
            &data[data_start..]
        } else {
            &[]
        };

        log::info!(
            "GGUF parsed: {} tensors, {} metadata keys, data section at offset {} ({} bytes)",
            tensors.len(),
            metadata.len(),
            data_start,
            data_section.len()
        );

        Ok(GgufFile {
            version,
            metadata,
            tensors,
            data: data_section,
            alignment,
        })
    }

    // -----------------------------------------------------------------------
    // Convenience metadata accessors
    // -----------------------------------------------------------------------

    /// Get the model architecture string (e.g. "llama", "gpt2").
    pub fn architecture(&self) -> &str {
        match self.metadata.get("general.architecture") {
            Some(GgufValue::String(s)) => s.as_str(),
            _ => "unknown",
        }
    }

    /// Get a metadata value as `u32`.
    pub fn get_u32(&self, key: &str) -> Option<u32> {
        match self.metadata.get(key)? {
            GgufValue::Uint32(v) => Some(*v),
            GgufValue::Int32(v) if *v >= 0 => Some(*v as u32),
            GgufValue::Uint8(v) => Some(*v as u32),
            GgufValue::Uint16(v) => Some(*v as u32),
            _ => None,
        }
    }

    /// Get a metadata value as `f32`.
    pub fn get_f32(&self, key: &str) -> Option<f32> {
        match self.metadata.get(key)? {
            GgufValue::Float32(v) => Some(*v),
            GgufValue::Float64(v) => Some(*v as f32),
            _ => None,
        }
    }

    /// Get a metadata value as `&str`.
    pub fn get_string(&self, key: &str) -> Option<&str> {
        match self.metadata.get(key)? {
            GgufValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Get a metadata value as `u64`.
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        match self.metadata.get(key)? {
            GgufValue::Uint64(v) => Some(*v),
            GgufValue::Uint32(v) => Some(*v as u64),
            GgufValue::Int32(v) if *v >= 0 => Some(*v as u64),
            GgufValue::Int64(v) if *v >= 0 => Some(*v as u64),
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Tensor accessors
    // -----------------------------------------------------------------------

    /// Look up a tensor by name.
    pub fn get_tensor(&self, name: &str) -> Option<&TensorInfo> {
        self.tensors.iter().find(|t| t.name == name)
    }

    /// Get the raw bytes for a tensor from the data section.
    ///
    /// # Panics
    /// Panics if the tensor offset + size exceeds the data section length.
    pub fn tensor_data(&self, tensor: &TensorInfo) -> &'a [u8] {
        let start = tensor.offset as usize;
        let size = tensor.byte_size();
        &self.data[start..start + size]
    }

    /// Get the raw bytes for a tensor, returning `None` if out of bounds.
    pub fn try_tensor_data(&self, tensor: &TensorInfo) -> Option<&'a [u8]> {
        let start = tensor.offset as usize;
        let size = tensor.byte_size();
        let end = start.checked_add(size)?;
        if end <= self.data.len() {
            Some(&self.data[start..end])
        } else {
            None
        }
    }

    /// Summary of model parameters (convenience for logging).
    pub fn model_summary(&self) -> String {
        use alloc::format;
        let arch = self.architecture();
        let total_params: u64 = self.tensors.iter().map(|t| t.n_elements()).sum();
        let total_bytes: usize = self.tensors.iter().map(|t| t.byte_size()).sum();
        format!(
            "arch={}, tensors={}, params={}M, size={}MB",
            arch,
            self.tensors.len(),
            total_params / 1_000_000,
            total_bytes / (1024 * 1024),
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Build a minimal valid GGUF v3 file in memory.
    fn build_test_gguf() -> Vec<u8> {
        let mut buf = Vec::new();

        // Magic
        buf.extend_from_slice(&GGUF_MAGIC.to_le_bytes());
        // Version 3
        buf.extend_from_slice(&3u32.to_le_bytes());
        // 1 tensor
        buf.extend_from_slice(&1u64.to_le_bytes());
        // 2 metadata entries
        buf.extend_from_slice(&2u64.to_le_bytes());

        // Metadata entry 1: "general.architecture" = "llama" (string)
        let key1 = b"general.architecture";
        buf.extend_from_slice(&(key1.len() as u64).to_le_bytes());
        buf.extend_from_slice(key1);
        buf.extend_from_slice(&8u32.to_le_bytes()); // STRING type
        let val1 = b"llama";
        buf.extend_from_slice(&(val1.len() as u64).to_le_bytes());
        buf.extend_from_slice(val1);

        // Metadata entry 2: "llama.context_length" = 4096 (UINT32)
        let key2 = b"llama.context_length";
        buf.extend_from_slice(&(key2.len() as u64).to_le_bytes());
        buf.extend_from_slice(key2);
        buf.extend_from_slice(&4u32.to_le_bytes()); // UINT32 type
        buf.extend_from_slice(&4096u32.to_le_bytes());

        // Tensor info: "output.weight", 2D [128, 64], F32, offset 0
        let tname = b"output.weight";
        buf.extend_from_slice(&(tname.len() as u64).to_le_bytes());
        buf.extend_from_slice(tname);
        buf.extend_from_slice(&2u32.to_le_bytes()); // 2 dimensions
        buf.extend_from_slice(&128u64.to_le_bytes());
        buf.extend_from_slice(&64u64.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // F32
        buf.extend_from_slice(&0u64.to_le_bytes()); // offset 0

        // Align to 32 bytes
        let rem = buf.len() % 32;
        if rem != 0 {
            buf.resize(buf.len() + (32 - rem), 0);
        }

        // Data section: 128 * 64 * 4 = 32768 bytes of zeros
        buf.resize(buf.len() + 128 * 64 * 4, 0);

        buf
    }

    #[test]
    fn test_parse_header() {
        let data = build_test_gguf();
        let file = GgufFile::parse(&data).unwrap();
        assert_eq!(file.version, 3);
        assert_eq!(file.alignment, DEFAULT_ALIGNMENT);
    }

    #[test]
    fn test_architecture() {
        let data = build_test_gguf();
        let file = GgufFile::parse(&data).unwrap();
        assert_eq!(file.architecture(), "llama");
    }

    #[test]
    fn test_metadata_u32() {
        let data = build_test_gguf();
        let file = GgufFile::parse(&data).unwrap();
        assert_eq!(file.get_u32("llama.context_length"), Some(4096));
    }

    #[test]
    fn test_metadata_string() {
        let data = build_test_gguf();
        let file = GgufFile::parse(&data).unwrap();
        assert_eq!(file.get_string("general.architecture"), Some("llama"));
    }

    #[test]
    fn test_tensor_info() {
        let data = build_test_gguf();
        let file = GgufFile::parse(&data).unwrap();
        assert_eq!(file.tensors.len(), 1);
        let t = file.get_tensor("output.weight").unwrap();
        assert_eq!(t.shape, vec![128, 64]);
        assert_eq!(t.dtype, GgmlType::F32);
        assert_eq!(t.n_elements(), 128 * 64);
        assert_eq!(t.byte_size(), 128 * 64 * 4);
    }

    #[test]
    fn test_tensor_data() {
        let data = build_test_gguf();
        let file = GgufFile::parse(&data).unwrap();
        let t = file.get_tensor("output.weight").unwrap();
        let td = file.tensor_data(t);
        assert_eq!(td.len(), 128 * 64 * 4);
    }

    #[test]
    fn test_invalid_magic() {
        let data = vec![0u8; 64];
        let result = GgufFile::parse(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_truncated_file() {
        let data = vec![0x47, 0x47, 0x55, 0x46]; // just magic, nothing else
        let result = GgufFile::parse(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_ggml_type_sizes() {
        assert_eq!(GgmlType::F32.block_size(), 1);
        assert_eq!(GgmlType::F32.type_size(), 4);
        assert_eq!(GgmlType::F16.block_size(), 1);
        assert_eq!(GgmlType::F16.type_size(), 2);
        assert_eq!(GgmlType::Q4_0.block_size(), 32);
        assert_eq!(GgmlType::Q4_0.type_size(), 18);
        assert_eq!(GgmlType::Q4_1.block_size(), 32);
        assert_eq!(GgmlType::Q4_1.type_size(), 20);
        assert_eq!(GgmlType::Q8_0.block_size(), 32);
        assert_eq!(GgmlType::Q8_0.type_size(), 34);
    }

    #[test]
    fn test_model_summary() {
        let data = build_test_gguf();
        let file = GgufFile::parse(&data).unwrap();
        let summary = file.model_summary();
        assert!(summary.contains("llama"));
        assert!(summary.contains("tensors=1"));
    }

    #[test]
    fn test_missing_metadata_returns_none() {
        let data = build_test_gguf();
        let file = GgufFile::parse(&data).unwrap();
        assert_eq!(file.get_u32("nonexistent.key"), None);
        assert_eq!(file.get_f32("nonexistent.key"), None);
        assert_eq!(file.get_string("nonexistent.key"), None);
    }

    #[test]
    fn test_get_tensor_missing() {
        let data = build_test_gguf();
        let file = GgufFile::parse(&data).unwrap();
        assert!(file.get_tensor("nonexistent.weight").is_none());
    }
}
