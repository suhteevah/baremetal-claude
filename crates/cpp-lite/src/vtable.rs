//! Virtual function table (vtable) generation and dynamic dispatch for C++.
//!
//! In C++, virtual methods are dispatched at runtime through vtables. Each class
//! with virtual methods has a vtable: a flat array of function pointers, one per
//! virtual method. Objects of that class contain a hidden pointer (the "vptr")
//! to their class's vtable.
//!
//! ## Vtable Layout
//!
//! ```text
//! VTable for class Derived (inherits from Base):
//! +---------+-----------------------------------+
//! | Index 0 | &Derived::foo  (overrides Base)   |
//! | Index 1 | &Base::bar     (inherited as-is)  |
//! | Index 2 | &Derived::baz  (new virtual)      |
//! +---------+-----------------------------------+
//! ```
//!
//! ## Inheritance
//!
//! When a derived class is created, its vtable starts as a clone of the base
//! class's vtable. Overridden methods replace entries at the same index
//! (preserving ABI compatibility). New virtual methods are appended.
//!
//! ## Pure Virtual (Abstract Classes)
//!
//! A pure virtual slot has `is_pure = true` and no implementation. A class
//! with any pure virtual slots is abstract and cannot be instantiated.
//!
//! ## Dynamic Dispatch
//!
//! To call a virtual method: `obj->vptr[vtable_index](obj, args...)`.
//! The vtable index is resolved at compile time from the method name.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// A single entry in a virtual function table.
///
/// Each entry corresponds to one virtual method and records the mangled name
/// of the implementation that should be called for this class.
#[derive(Debug, Clone)]
pub struct VTableEntry {
    /// Mangled name of the function implementation (e.g., `_ZN7Derived3fooEv`).
    /// Used as the symbol to link against when generating dispatch code.
    pub mangled_name: String,
    /// Position of this method in the vtable array.
    pub index: usize,
    /// True if this is a pure virtual slot (`= 0` in C++).
    /// Pure virtual entries have no valid implementation; calling them is UB.
    pub is_pure: bool,
}

/// Complete virtual function table for a single class.
///
/// The `entries` vector is the vtable itself: an ordered list of function
/// pointers. The `method_index` map provides O(log n) lookup from method
/// name to vtable slot index.
#[derive(Debug, Clone)]
pub struct VTable {
    /// The class this vtable belongs to.
    pub class_name: String,
    /// Ordered vtable entries (index 0 is the first virtual method).
    pub entries: Vec<VTableEntry>,
    /// Fast lookup from unmangled method name to vtable index.
    pub method_index: BTreeMap<String, usize>,
}

impl VTable {
    pub fn new(class_name: String) -> Self {
        Self {
            class_name,
            entries: Vec::new(),
            method_index: BTreeMap::new(),
        }
    }

    /// Add a virtual method. Returns its vtable index.
    pub fn add_method(&mut self, name: String, mangled_name: String, is_pure: bool) -> usize {
        let index = self.entries.len();
        self.method_index.insert(name, index);
        self.entries.push(VTableEntry { mangled_name, index, is_pure });
        index
    }

    /// Override a method from a base class.
    pub fn override_method(&mut self, name: &str, new_mangled: String) -> bool {
        if let Some(&idx) = self.method_index.get(name) {
            self.entries[idx].mangled_name = new_mangled;
            self.entries[idx].is_pure = false;
            true
        } else {
            false
        }
    }

    /// Check if this class has any pure virtual methods (is abstract).
    pub fn is_abstract(&self) -> bool {
        self.entries.iter().any(|e| e.is_pure)
    }

    /// Get the vtable index for a method.
    pub fn lookup(&self, method_name: &str) -> Option<usize> {
        self.method_index.get(method_name).copied()
    }

    /// Size of the vtable in pointers.
    pub fn size(&self) -> usize {
        self.entries.len()
    }
}

/// VTable builder: constructs vtables respecting inheritance.
pub struct VTableBuilder {
    /// All vtables by class name.
    vtables: BTreeMap<String, VTable>,
}

impl VTableBuilder {
    pub fn new() -> Self {
        Self { vtables: BTreeMap::new() }
    }

    /// Create a vtable for a class, inheriting from an optional base.
    pub fn create_vtable(&mut self, class_name: &str, base_name: Option<&str>) -> &mut VTable {
        let vtable = if let Some(base) = base_name {
            // Clone the base vtable
            if let Some(base_vt) = self.vtables.get(base) {
                let mut vt = base_vt.clone();
                vt.class_name = String::from(class_name);
                vt
            } else {
                VTable::new(String::from(class_name))
            }
        } else {
            VTable::new(String::from(class_name))
        };

        self.vtables.insert(String::from(class_name), vtable);
        self.vtables.get_mut(class_name).unwrap()
    }

    /// Get a vtable.
    pub fn get_vtable(&self, class_name: &str) -> Option<&VTable> {
        self.vtables.get(class_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vtable_basic() {
        let mut vt = VTable::new(String::from("Base"));
        let idx = vt.add_method(
            String::from("foo"),
            String::from("_ZN4Base3fooEv"),
            false,
        );
        assert_eq!(idx, 0);
        assert_eq!(vt.lookup("foo"), Some(0));
    }

    #[test]
    fn test_vtable_override() {
        let mut vt = VTable::new(String::from("Base"));
        vt.add_method(String::from("foo"), String::from("_ZN4Base3fooEv"), false);

        let overridden = vt.override_method("foo", String::from("_ZN7Derived3fooEv"));
        assert!(overridden);
        assert_eq!(vt.entries[0].mangled_name, "_ZN7Derived3fooEv");
    }

    #[test]
    fn test_abstract_class() {
        let mut vt = VTable::new(String::from("Abstract"));
        vt.add_method(String::from("pure_fn"), String::from(""), true);
        assert!(vt.is_abstract());
    }

    #[test]
    fn test_vtable_inheritance() {
        let mut builder = VTableBuilder::new();
        let base = builder.create_vtable("Base", None);
        base.add_method(String::from("foo"), String::from("_ZN4Base3fooEv"), false);
        base.add_method(String::from("bar"), String::from("_ZN4Base3barEv"), false);

        let derived = builder.create_vtable("Derived", Some("Base"));
        derived.override_method("foo", String::from("_ZN7Derived3fooEv"));

        let vt = builder.get_vtable("Derived").unwrap();
        assert_eq!(vt.size(), 2);
        assert_eq!(vt.entries[0].mangled_name, "_ZN7Derived3fooEv");
        assert_eq!(vt.entries[1].mangled_name, "_ZN4Base3barEv");
    }
}
