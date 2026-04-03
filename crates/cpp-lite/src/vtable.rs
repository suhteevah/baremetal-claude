//! Virtual function table generation and dynamic dispatch.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// A virtual function table entry.
#[derive(Debug, Clone)]
pub struct VTableEntry {
    /// Mangled function name.
    pub mangled_name: String,
    /// Index in the vtable.
    pub index: usize,
    /// Whether this is a pure virtual slot.
    pub is_pure: bool,
}

/// Virtual function table for a class.
#[derive(Debug, Clone)]
pub struct VTable {
    /// Class this vtable belongs to.
    pub class_name: String,
    /// Ordered entries: index -> entry.
    pub entries: Vec<VTableEntry>,
    /// Lookup: method_name -> vtable index.
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
        let mut vtable = if let Some(base) = base_name {
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
