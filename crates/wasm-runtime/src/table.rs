//! Function reference tables for call_indirect.

use alloc::vec;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

use crate::types::Limits;

/// A table of function references (or null).
pub struct Table {
    /// Each entry is either Some(func_idx) or None (null ref).
    elements: Vec<Option<u32>>,
    max: Option<u32>,
}

impl Table {
    pub fn new(limits: &Limits) -> Self {
        let size = limits.min as usize;
        Self {
            elements: vec![None; size],
            max: limits.max,
        }
    }

    pub fn size(&self) -> u32 {
        self.elements.len() as u32
    }

    pub fn get(&self, idx: u32) -> Result<Option<u32>, String> {
        let i = idx as usize;
        if i >= self.elements.len() {
            return Err(format!("table index out of bounds: {}", idx));
        }
        Ok(self.elements[i])
    }

    pub fn set(&mut self, idx: u32, val: Option<u32>) -> Result<(), String> {
        let i = idx as usize;
        if i >= self.elements.len() {
            return Err(format!("table index out of bounds: {}", idx));
        }
        self.elements[i] = val;
        Ok(())
    }

    pub fn grow(&mut self, delta: u32, init: Option<u32>) -> i32 {
        let old = self.elements.len() as u32;
        let new_size = old.checked_add(delta);
        match new_size {
            Some(ns) if self.max.is_none_or(|m| ns <= m) => {
                self.elements.resize(ns as usize, init);
                old as i32
            }
            _ => -1,
        }
    }
}
