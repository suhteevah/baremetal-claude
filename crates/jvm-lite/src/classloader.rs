//! Class loading: bootstrap loader, class resolution, field/method lookup.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::classfile::{ClassFile, FieldInfo, MethodInfo, ACC_STATIC};

/// Class loader that resolves classes, fields, and methods.
pub struct ClassLoader {
    /// Loaded classes by fully-qualified name.
    classes: BTreeMap<String, ClassFile>,
    /// Class initialization status.
    initialized: BTreeMap<String, bool>,
}

impl ClassLoader {
    pub fn new() -> Self {
        Self {
            classes: BTreeMap::new(),
            initialized: BTreeMap::new(),
        }
    }

    /// Load a class from parsed bytes.
    pub fn load(&mut self, class: ClassFile) -> Result<String, String> {
        let name = class.class_name()
            .ok_or_else(|| String::from("class has no name"))?
            .to_string();
        log::debug!("[classloader] loading: {}", name);
        self.classes.insert(name.clone(), class);
        Ok(name)
    }

    /// Get a loaded class.
    pub fn get_class(&self, name: &str) -> Option<&ClassFile> {
        self.classes.get(name)
    }

    /// Resolve a method: search the class and its superclasses.
    pub fn resolve_method(&self, class_name: &str, method_name: &str, descriptor: &str) -> Option<(&ClassFile, &MethodInfo)> {
        let mut current = class_name.to_string();
        loop {
            if let Some(class) = self.classes.get(&current) {
                for method in &class.methods {
                    let name = class.get_utf8(method.name_index).unwrap_or("");
                    let desc = class.get_utf8(method.descriptor_index).unwrap_or("");
                    if name == method_name && desc == descriptor {
                        return Some((class, method));
                    }
                }
                // Walk up to super class
                if let Some(super_name) = class.super_class_name() {
                    current = super_name.to_string();
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        None
    }

    /// Resolve a static method.
    pub fn resolve_static_method(&self, class_name: &str, method_name: &str) -> Option<(&ClassFile, &MethodInfo)> {
        if let Some(class) = self.classes.get(class_name) {
            for method in &class.methods {
                if method.access_flags & ACC_STATIC != 0 {
                    let name = class.get_utf8(method.name_index).unwrap_or("");
                    if name == method_name {
                        return Some((class, method));
                    }
                }
            }
        }
        None
    }

    /// Resolve a field: search the class and its superclasses.
    pub fn resolve_field(&self, class_name: &str, field_name: &str) -> Option<(&ClassFile, &FieldInfo)> {
        let mut current = class_name.to_string();
        loop {
            if let Some(class) = self.classes.get(&current) {
                for field in &class.fields {
                    let name = class.get_utf8(field.name_index).unwrap_or("");
                    if name == field_name {
                        return Some((class, field));
                    }
                }
                if let Some(super_name) = class.super_class_name() {
                    current = super_name.to_string();
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        None
    }

    /// Check if a class implements an interface (simplified).
    pub fn implements_interface(&self, class_name: &str, interface_name: &str) -> bool {
        if let Some(class) = self.classes.get(class_name) {
            for &iface_idx in &class.interfaces {
                if let Some(iface_name) = {
                    if let Some(crate::classfile::CpEntry::Class(name_idx)) = class.constant_pool.get(iface_idx as usize) {
                        class.get_utf8(*name_idx)
                    } else {
                        None
                    }
                } {
                    if iface_name == interface_name {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// List all loaded class names.
    pub fn loaded_classes(&self) -> Vec<String> {
        self.classes.keys().cloned().collect()
    }
}
