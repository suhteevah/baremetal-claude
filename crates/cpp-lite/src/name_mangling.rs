//! Itanium C++ ABI name mangling for function overloading.
//!
//! Generates mangled names so overloaded functions get unique symbols.

use alloc::format;
use alloc::string::String;

use crate::ast::CppType;

/// Mangle a C++ function name following the Itanium ABI (simplified).
///
/// Format: _Z<name_len><name><param_encodings>
pub fn mangle_function(name: &str, params: &[CppType], is_const: bool) -> String {
    let mut mangled = String::from("_Z");
    mangled.push_str(&format!("{}{}", name.len(), name));

    if params.is_empty() {
        mangled.push('v'); // void
    } else {
        for param in params {
            mangled.push_str(&mangle_type(param));
        }
    }

    if is_const {
        mangled.push('K');
    }

    mangled
}

/// Mangle a C++ method name: _ZN<class_len><class><name_len><name>E<params>
pub fn mangle_method(class_name: &str, method_name: &str, params: &[CppType], is_const: bool) -> String {
    let mut mangled = String::from("_ZN");

    if is_const {
        mangled.push('K');
    }

    mangled.push_str(&format!("{}{}", class_name.len(), class_name));
    mangled.push_str(&format!("{}{}", method_name.len(), method_name));
    mangled.push('E');

    if params.is_empty() {
        mangled.push('v');
    } else {
        for param in params {
            mangled.push_str(&mangle_type(param));
        }
    }

    mangled
}

/// Mangle a constructor: _ZN<class_len><class>C1E<params>
pub fn mangle_constructor(class_name: &str, params: &[CppType]) -> String {
    let mut mangled = String::from("_ZN");
    mangled.push_str(&format!("{}{}", class_name.len(), class_name));
    mangled.push_str("C1E");

    if params.is_empty() {
        mangled.push('v');
    } else {
        for param in params {
            mangled.push_str(&mangle_type(param));
        }
    }

    mangled
}

/// Mangle a destructor: _ZN<class_len><class>D1Ev
pub fn mangle_destructor(class_name: &str) -> String {
    let mut mangled = String::from("_ZN");
    mangled.push_str(&format!("{}{}", class_name.len(), class_name));
    mangled.push_str("D1Ev");
    mangled
}

/// Encode a type for mangling.
fn mangle_type(ty: &CppType) -> String {
    match ty {
        CppType::Void => String::from("v"),
        CppType::Bool => String::from("b"),
        CppType::Char => String::from("c"),
        CppType::Int => String::from("i"),
        CppType::Long => String::from("l"),
        CppType::LongLong => String::from("x"),
        CppType::Float => String::from("f"),
        CppType::Double => String::from("d"),
        CppType::Pointer(inner) => format!("P{}", mangle_type(inner)),
        CppType::Reference(inner) => format!("R{}", mangle_type(inner)),
        CppType::RvalueRef(inner) => format!("O{}", mangle_type(inner)),
        CppType::Const(inner) => format!("K{}", mangle_type(inner)),
        CppType::Named(name) => format!("{}{}", name.len(), name),
        CppType::Qualified(ns, name) => format!("N{}{}{}{}E", ns.len(), ns, name.len(), name),
        CppType::Template { name, args } => {
            let mut s = format!("{}{}I", name.len(), name);
            for arg in args {
                s.push_str(&mangle_type(arg));
            }
            s.push('E');
            s
        }
        _ => String::from("v"), // fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mangle_function() {
        let mangled = mangle_function("foo", &[CppType::Int, CppType::Int], false);
        assert_eq!(mangled, "_Z3fooii");
    }

    #[test]
    fn test_mangle_void_function() {
        let mangled = mangle_function("bar", &[], false);
        assert_eq!(mangled, "_Z3barv");
    }

    #[test]
    fn test_mangle_method() {
        let mangled = mangle_method("Foo", "bar", &[CppType::Int], false);
        assert_eq!(mangled, "_ZN3Foo3barEi");
    }

    #[test]
    fn test_mangle_constructor() {
        let mangled = mangle_constructor("Foo", &[CppType::Int]);
        assert_eq!(mangled, "_ZN3FooC1Ei");
    }

    #[test]
    fn test_mangle_destructor() {
        let mangled = mangle_destructor("Foo");
        assert_eq!(mangled, "_ZN3FooD1Ev");
    }

    #[test]
    fn test_mangle_pointer_type() {
        let mangled = mangle_function("f", &[CppType::Pointer(alloc::boxed::Box::new(CppType::Int))], false);
        assert_eq!(mangled, "_Z1fPi");
    }
}
