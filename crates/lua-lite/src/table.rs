//! Lua table: hybrid array + hash map, metatable support.

use alloc::string::String;
use alloc::vec::Vec;

use crate::vm::LuaValue;

/// A Lua table with hybrid array + hash storage.
#[derive(Debug, Clone)]
pub struct LuaTable {
    /// Array part (1-indexed in Lua, 0-indexed internally).
    pub array: Vec<LuaValue>,
    /// Hash part: key-value pairs.
    pub hash: Vec<(LuaValue, LuaValue)>,
    /// Metatable reference (index into VM's table list).
    pub metatable: Option<usize>,
}

impl LuaTable {
    pub fn new() -> Self {
        Self {
            array: Vec::new(),
            hash: Vec::new(),
            metatable: None,
        }
    }

    /// Get a value by key.
    pub fn get(&self, key: &LuaValue) -> LuaValue {
        // Try array part first for integer keys
        if let LuaValue::Integer(i) = key {
            let idx = *i;
            if idx >= 1 && (idx as usize) <= self.array.len() {
                return self.array[(idx - 1) as usize].clone();
            }
        }
        // Also try converting float keys that are integers
        if let LuaValue::Number(f) = key {
            let i = *f as i64;
            if (i as f64) == *f && i >= 1 && (i as usize) <= self.array.len() {
                return self.array[(i - 1) as usize].clone();
            }
        }

        // Hash part
        for (k, v) in &self.hash {
            if lua_key_eq(k, key) {
                return v.clone();
            }
        }

        LuaValue::Nil
    }

    /// Set a value by key.
    pub fn set(&mut self, key: LuaValue, value: LuaValue) {
        // Try array part for integer keys
        if let LuaValue::Integer(i) = &key {
            let idx = *i;
            if idx >= 1 {
                let uidx = idx as usize;
                if uidx <= self.array.len() {
                    if matches!(&value, LuaValue::Nil) && uidx == self.array.len() {
                        self.array.pop();
                    } else {
                        self.array[uidx - 1] = value;
                    }
                    return;
                } else if uidx == self.array.len() + 1 {
                    if !matches!(&value, LuaValue::Nil) {
                        self.array.push(value);
                    }
                    return;
                }
            }
        }

        // Handle float keys that are integers
        if let LuaValue::Number(f) = &key {
            let i = *f as i64;
            if (i as f64) == *f {
                self.set(LuaValue::Integer(i), value);
                return;
            }
        }

        if matches!(&value, LuaValue::Nil) {
            // Remove from hash
            self.hash.retain(|(k, _)| !lua_key_eq(k, &key));
            return;
        }

        // Update existing or insert
        for (k, v) in &mut self.hash {
            if lua_key_eq(k, &key) {
                *v = value;
                return;
            }
        }
        self.hash.push((key, value));
    }

    /// Length operator (#table) - returns length of array part.
    pub fn len(&self) -> usize {
        self.array.len()
    }

    /// Get next key-value pair after the given key (for pairs()).
    pub fn next(&self, key: &LuaValue) -> Option<(LuaValue, LuaValue)> {
        if matches!(key, LuaValue::Nil) {
            // Return first element
            if !self.array.is_empty() {
                return Some((LuaValue::Integer(1), self.array[0].clone()));
            }
            if !self.hash.is_empty() {
                return Some((self.hash[0].0.clone(), self.hash[0].1.clone()));
            }
            return None;
        }

        // Check array part
        if let LuaValue::Integer(i) = key {
            let idx = *i as usize;
            if idx >= 1 && idx <= self.array.len() {
                // Next array element
                if idx < self.array.len() {
                    return Some((LuaValue::Integer(idx as i64 + 1), self.array[idx].clone()));
                }
                // Transition to hash part
                if !self.hash.is_empty() {
                    return Some((self.hash[0].0.clone(), self.hash[0].1.clone()));
                }
                return None;
            }
        }

        // Check hash part
        for (i, (k, _)) in self.hash.iter().enumerate() {
            if lua_key_eq(k, key) {
                if i + 1 < self.hash.len() {
                    return Some((self.hash[i + 1].0.clone(), self.hash[i + 1].1.clone()));
                }
                return None;
            }
        }

        None
    }

    /// Insert a value at position in the array part.
    pub fn insert(&mut self, pos: usize, value: LuaValue) {
        if pos == 0 || pos > self.array.len() + 1 {
            self.array.push(value);
        } else {
            self.array.insert(pos - 1, value);
        }
    }

    /// Remove and return a value at position in the array part.
    pub fn remove(&mut self, pos: usize) -> LuaValue {
        if pos >= 1 && pos <= self.array.len() {
            self.array.remove(pos - 1)
        } else {
            LuaValue::Nil
        }
    }

    /// Sort the array part using a comparison function.
    /// Returns true if sorting succeeded.
    pub fn sort_array(&mut self) {
        self.array.sort_by(|a, b| lua_compare(a, b));
    }

    /// Concatenate array elements as strings.
    pub fn concat(&self, sep: &str, i: usize, j: usize) -> String {
        let mut parts = Vec::new();
        for idx in i..=j {
            if idx >= 1 && idx <= self.array.len() {
                parts.push(self.array[idx - 1].to_display_string());
            }
        }
        let mut result = String::new();
        for (idx, part) in parts.iter().enumerate() {
            if idx > 0 {
                result.push_str(sep);
            }
            result.push_str(part);
        }
        result
    }
}

/// Compare two Lua values as keys (for table lookup).
fn lua_key_eq(a: &LuaValue, b: &LuaValue) -> bool {
    match (a, b) {
        (LuaValue::Nil, LuaValue::Nil) => true,
        (LuaValue::Boolean(a), LuaValue::Boolean(b)) => a == b,
        (LuaValue::Integer(a), LuaValue::Integer(b)) => a == b,
        (LuaValue::Number(a), LuaValue::Number(b)) => a.to_bits() == b.to_bits(),
        (LuaValue::Integer(a), LuaValue::Number(b)) => (*a as f64).to_bits() == b.to_bits(),
        (LuaValue::Number(a), LuaValue::Integer(b)) => a.to_bits() == (*b as f64).to_bits(),
        (LuaValue::String(a), LuaValue::String(b)) => a == b,
        _ => false,
    }
}

/// Compare two Lua values for sorting.
fn lua_compare(a: &LuaValue, b: &LuaValue) -> core::cmp::Ordering {
    match (a, b) {
        (LuaValue::Integer(a), LuaValue::Integer(b)) => a.cmp(b),
        (LuaValue::Number(a), LuaValue::Number(b)) => a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal),
        (LuaValue::Integer(a), LuaValue::Number(b)) => (*a as f64).partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal),
        (LuaValue::Number(a), LuaValue::Integer(b)) => a.partial_cmp(&(*b as f64)).unwrap_or(core::cmp::Ordering::Equal),
        (LuaValue::String(a), LuaValue::String(b)) => a.cmp(b),
        _ => core::cmp::Ordering::Equal,
    }
}
