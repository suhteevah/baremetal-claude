//! Itanium C++ ABI name mangling (simplified).
//!
//! C++ allows function overloading (same name, different parameter types), so
//! the linker needs unique symbol names. The Itanium ABI defines a mangling
//! scheme that encodes the function name, parameter types, and qualifiers
//! into a single ASCII symbol.
//!
//! ## Mangling Format
//!
//! All mangled names start with `_Z`:
//!
//! | Pattern | Meaning |
//! |---------|---------|
//! | `_Z<len><name><params>` | Free function |
//! | `_ZN<len><class><len><method>E<params>` | Member function |
//! | `_ZN<len><class>C1E<params>` | Complete object constructor |
//! | `_ZN<len><class>D1Ev` | Complete object destructor |
//!
//! ## Type Encodings
//!
//! | Type | Code | Type | Code |
//! |------|------|------|------|
//! | void | `v` | bool | `b` |
//! | char | `c` | int | `i` |
//! | long | `l` | long long | `x` |
//! | float | `f` | double | `d` |
//! | `T*` | `P<T>` | `T&` | `R<T>` |
//! | `T&&` | `O<T>` | `const T` | `K<T>` |
//! | Named type | `<len><name>` | e.g., `3Foo` |

use alloc::format;
use alloc::string::String;

use crate::ast::CppType;

/// Mangle a free (non-member) C++ function name.
///
/// Format: `_Z<name_len><name><param_encodings>`
///
/// Example: `foo(int, int)` -> `_Z3fooii`
/// Example: `bar()` -> `_Z3barv` (void parameter list)
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

/// Mangle a C++ member function (method) name.
///
/// Format: `_ZN[K]<class_len><class><name_len><name>E<params>`
/// The `N...E` wrapper indicates a nested name. `K` prefix means `const` method.
///
/// Example: `Foo::bar(int)` -> `_ZN3Foo3barEi`
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

/// Mangle a constructor name. `C1` = complete object constructor.
///
/// Format: `_ZN<class_len><class>C1E<params>`
///
/// Example: `Foo::Foo(int)` -> `_ZN3FooC1Ei`
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

/// Mangle a destructor name. `D1` = complete object destructor, always `void` params.
///
/// Format: `_ZN<class_len><class>D1Ev`
///
/// Example: `Foo::~Foo()` -> `_ZN3FooD1Ev`
pub fn mangle_destructor(class_name: &str) -> String {
    let mut mangled = String::from("_ZN");
    mangled.push_str(&format!("{}{}", class_name.len(), class_name));
    mangled.push_str("D1Ev");
    mangled
}

/// Encode a single C++ type as its Itanium ABI mangled form.
///
/// Recursively encodes compound types: `Pointer(Int)` -> `"Pi"`,
/// `Template { name: "vector", args: [Int] }` -> `"6vectorIiE"`.
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
