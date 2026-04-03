//! Top-level JVM runtime API.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::classfile::{self, ClassFile};
use crate::classloader::ClassLoader;
use crate::vm::Vm;

/// The JVM runtime: loads classes, runs main methods.
pub struct JvmRuntime {
    loader: ClassLoader,
    vm: Vm,
}

impl JvmRuntime {
    pub fn new() -> Self {
        Self {
            loader: ClassLoader::new(),
            vm: Vm::new(),
        }
    }

    /// Load a class from .class file bytes.
    pub fn load_class(&mut self, data: &[u8]) -> Result<String, String> {
        let class = classfile::parse_class(data)?;
        let name = class.class_name()
            .ok_or_else(|| String::from("class has no name"))?
            .to_string();
        log::info!("[jvm] loading class: {}", name);
        self.loader.load(class.clone())?;
        self.vm.load_class(class);
        Ok(name)
    }

    /// Run the main method of a loaded class, returning captured output.
    pub fn run_main(&mut self, class_name: &str) -> Result<String, String> {
        log::info!("[jvm] running main in {}", class_name);
        self.vm.run_main(class_name)?;
        Ok(self.vm.take_output())
    }

    /// Create an instance of a class (simplified).
    pub fn create_instance(&mut self, class_name: &str) -> Result<usize, String> {
        let obj = crate::gc::JvmObject::new(String::from(class_name));
        Ok(self.vm.heap.allocate_object(obj))
    }

    /// List loaded classes.
    pub fn loaded_classes(&self) -> Vec<String> {
        self.loader.loaded_classes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_runtime() {
        let rt = JvmRuntime::new();
        assert!(rt.loaded_classes().is_empty());
    }

    #[test]
    fn test_invalid_class() {
        let mut rt = JvmRuntime::new();
        let result = rt.load_class(&[0x00, 0x01, 0x02, 0x03]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cafebabe_too_short() {
        let mut rt = JvmRuntime::new();
        let result = rt.load_class(&[0xCA, 0xFE, 0xBA, 0xBE]);
        assert!(result.is_err()); // too short to parse
    }
}
