//! Linear memory: 64KiB pages, bounds-checked access, grow.

use alloc::vec;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

/// WASM page size: 64 KiB.
pub const PAGE_SIZE: usize = 65536;

/// Maximum pages (by spec: 65536 = 4 GiB, but we limit to 256 = 16 MiB for bare metal).
pub const MAX_PAGES: u32 = 256;

/// Linear memory instance.
pub struct LinearMemory {
    data: Vec<u8>,
    current_pages: u32,
    max_pages: Option<u32>,
}

impl LinearMemory {
    pub fn new(initial_pages: u32, max_pages: Option<u32>) -> Result<Self, String> {
        let max = max_pages.unwrap_or(MAX_PAGES).min(MAX_PAGES);
        if initial_pages > max {
            return Err(format!(
                "initial pages {} exceeds maximum {}",
                initial_pages, max
            ));
        }
        let size = (initial_pages as usize) * PAGE_SIZE;
        Ok(Self {
            data: vec![0u8; size],
            current_pages: initial_pages,
            max_pages: Some(max),
        })
    }

    pub fn size_pages(&self) -> u32 {
        self.current_pages
    }

    pub fn size_bytes(&self) -> usize {
        self.data.len()
    }

    /// Grow memory by `delta` pages. Returns previous size in pages, or -1 on failure.
    pub fn grow(&mut self, delta: u32) -> i32 {
        let old = self.current_pages;
        let new_pages = old.checked_add(delta);
        match new_pages {
            Some(np) if np <= self.max_pages.unwrap_or(MAX_PAGES) => {
                let additional = (delta as usize) * PAGE_SIZE;
                self.data.resize(self.data.len() + additional, 0);
                self.current_pages = np;
                old as i32
            }
            _ => -1,
        }
    }

    /// Read a single byte.
    pub fn read_u8(&self, addr: u32) -> Result<u8, String> {
        let a = addr as usize;
        if a >= self.data.len() {
            return Err(format!("memory access out of bounds: 0x{:x}", addr));
        }
        Ok(self.data[a])
    }

    /// Write a single byte.
    pub fn write_u8(&mut self, addr: u32, val: u8) -> Result<(), String> {
        let a = addr as usize;
        if a >= self.data.len() {
            return Err(format!("memory access out of bounds: 0x{:x}", addr));
        }
        self.data[a] = val;
        Ok(())
    }

    /// Read N bytes from memory.
    pub fn read_bytes(&self, addr: u32, len: usize) -> Result<&[u8], String> {
        let a = addr as usize;
        let end = a.checked_add(len).ok_or_else(|| String::from("address overflow"))?;
        if end > self.data.len() {
            return Err(format!("memory access out of bounds: 0x{:x}+{}", addr, len));
        }
        Ok(&self.data[a..end])
    }

    /// Write bytes to memory.
    pub fn write_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), String> {
        let a = addr as usize;
        let end = a.checked_add(data.len()).ok_or_else(|| String::from("address overflow"))?;
        if end > self.data.len() {
            return Err(format!("memory access out of bounds: 0x{:x}+{}", addr, data.len()));
        }
        self.data[a..end].copy_from_slice(data);
        Ok(())
    }

    /// Read a little-endian u16.
    pub fn read_u16_le(&self, addr: u32) -> Result<u16, String> {
        let bytes = self.read_bytes(addr, 2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// Read a little-endian u32.
    pub fn read_u32_le(&self, addr: u32) -> Result<u32, String> {
        let bytes = self.read_bytes(addr, 4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Read a little-endian u64.
    pub fn read_u64_le(&self, addr: u32) -> Result<u64, String> {
        let bytes = self.read_bytes(addr, 8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    /// Write a little-endian u16.
    pub fn write_u16_le(&mut self, addr: u32, val: u16) -> Result<(), String> {
        self.write_bytes(addr, &val.to_le_bytes())
    }

    /// Write a little-endian u32.
    pub fn write_u32_le(&mut self, addr: u32, val: u32) -> Result<(), String> {
        self.write_bytes(addr, &val.to_le_bytes())
    }

    /// Write a little-endian u64.
    pub fn write_u64_le(&mut self, addr: u32, val: u64) -> Result<(), String> {
        self.write_bytes(addr, &val.to_le_bytes())
    }

    /// Read a little-endian f32.
    pub fn read_f32_le(&self, addr: u32) -> Result<f32, String> {
        let bits = self.read_u32_le(addr)?;
        Ok(f32::from_bits(bits))
    }

    /// Read a little-endian f64.
    pub fn read_f64_le(&self, addr: u32) -> Result<f64, String> {
        let bits = self.read_u64_le(addr)?;
        Ok(f64::from_bits(bits))
    }

    /// Write a little-endian f32.
    pub fn write_f32_le(&mut self, addr: u32, val: f32) -> Result<(), String> {
        self.write_u32_le(addr, val.to_bits())
    }

    /// Write a little-endian f64.
    pub fn write_f64_le(&mut self, addr: u32, val: f64) -> Result<(), String> {
        self.write_u64_le(addr, val.to_bits())
    }

    /// Direct access to underlying data (for data segment init).
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }
}
