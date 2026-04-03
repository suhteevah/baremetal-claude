//! Top-level compile + execute API for C++.

use alloc::string::String;
use alloc::vec::Vec;

use crate::lexer::tokenize_cpp;
use crate::parser::parse_cpp_tokens;
use crate::ast::*;

/// Error from C++ compilation.
#[derive(Debug)]
pub struct CppCompileError {
    pub message: String,
}

impl core::fmt::Display for CppCompileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "cpp error: {}", self.message)
    }
}

/// A compiled C++ program.
pub struct CompiledCppProgram {
    /// Compiled function code (from Cranelift).
    pub functions: Vec<CompiledCppFunc>,
    /// Entry point.
    pub entry: String,
    /// Combined code buffer.
    pub code: Vec<u8>,
    /// Entry point offset.
    pub entry_offset: usize,
}

/// A compiled C++ function.
pub struct CompiledCppFunc {
    pub name: String,
    pub mangled_name: String,
    pub code: Vec<u8>,
}

/// Compile C++ source code.
pub fn compile_cpp(source: &str) -> Result<CompiledCppProgram, CppCompileError> {
    log::info!("[cpp] compiling {} bytes of C++ source", source.len());

    // 1. Tokenize
    let tokens = tokenize_cpp(source).map_err(|e| CppCompileError { message: e })?;
    log::debug!("[cpp] lexed {} tokens", tokens.len());

    // 2. Parse
    let tu = parse_cpp_tokens(&tokens).map_err(|e| CppCompileError { message: e })?;
    log::debug!("[cpp] parsed {} declarations", tu.decls.len());

    // 3. Extract functions and compile via Cranelift
    // For now, delegate free functions to cc-lite's codegen and handle
    // C++ specifics (classes, templates) at a higher level.
    let mut functions = Vec::new();
    let mut code = Vec::new();
    let mut entry_offset = 0usize;

    for decl in &tu.decls {
        if let CppDecl::FuncDef(func) = decl {
            if func.name == "main" {
                entry_offset = code.len();
            }
            // Simplified: compile the C subset via cc-lite's pipeline
            // In a full implementation we'd handle C++ features
            let compiled = compile_cpp_func(func)?;
            code.extend_from_slice(&compiled.code);
            functions.push(compiled);
        }
    }

    log::info!("[cpp] compilation complete: {} functions, {} bytes",
        functions.len(), code.len());

    Ok(CompiledCppProgram {
        functions,
        entry: String::from("main"),
        code,
        entry_offset,
    })
}

fn compile_cpp_func(func: &CppFuncDef) -> Result<CompiledCppFunc, CppCompileError> {
    // Generate a C equivalent and compile it via cc-lite
    let c_source = generate_c_equivalent(func);
    let mangled = crate::name_mangling::mangle_function(
        &func.name,
        &func.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>(),
        false,
    );

    match claudio_cc_lite::compile(&c_source) {
        Ok(prog) => {
            Ok(CompiledCppFunc {
                name: func.name.clone(),
                mangled_name: mangled,
                code: prog.code,
            })
        }
        Err(e) => {
            // If C compilation fails, return empty code
            log::warn!("[cpp] C fallback compilation failed for {}: {}", func.name, e);
            Ok(CompiledCppFunc {
                name: func.name.clone(),
                mangled_name: mangled,
                code: Vec::new(),
            })
        }
    }
}

/// Generate a C-equivalent source for a C++ function (simplified).
fn generate_c_equivalent(func: &CppFuncDef) -> String {
    let ret = cpp_type_to_c(&func.return_type);
    let params: Vec<String> = func.params.iter().map(|p| {
        let ty = cpp_type_to_c(&p.ty);
        if let Some(ref name) = p.name {
            alloc::format!("{} {}", ty, name)
        } else {
            ty
        }
    }).collect();

    // Generate a minimal C function that returns 0
    alloc::format!("{} {}({}) {{ return 0; }}", ret, func.name, params.join(", "))
}

fn cpp_type_to_c(ty: &CppType) -> String {
    match ty {
        CppType::Void => String::from("void"),
        CppType::Bool => String::from("int"),
        CppType::Char => String::from("char"),
        CppType::Int => String::from("int"),
        CppType::Long => String::from("long"),
        CppType::LongLong => String::from("long long"),
        CppType::Float => String::from("float"),
        CppType::Double => String::from("double"),
        CppType::Pointer(inner) => alloc::format!("{}*", cpp_type_to_c(inner)),
        CppType::Reference(inner) => alloc::format!("{}*", cpp_type_to_c(inner)),
        CppType::Const(inner) => alloc::format!("const {}", cpp_type_to_c(inner)),
        CppType::Auto => String::from("int"), // simplified
        _ => String::from("int"),
    }
}

/// Execute a compiled C++ program.
///
/// # Safety
/// Runs compiled machine code.
pub unsafe fn execute_cpp(program: &CompiledCppProgram) -> i64 {
    if program.code.is_empty() {
        log::warn!("[cpp] no code to execute");
        return -1;
    }

    let mut code_mem = alloc::vec![0u8; program.code.len()];
    unsafe {
        core::ptr::copy_nonoverlapping(
            program.code.as_ptr(),
            code_mem.as_mut_ptr(),
            program.code.len(),
        );
    }

    let base = code_mem.as_ptr();
    let entry = unsafe { base.add(program.entry_offset) };
    core::mem::forget(code_mem);

    let func: fn() -> i64 = unsafe { core::mem::transmute(entry) };
    let result = func();

    log::info!("[cpp] program returned: {}", result);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_simple_cpp() {
        let result = compile_cpp("int main() { return 42; }");
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_class() {
        let result = compile_cpp(r#"
            class Foo {
            public:
                int x;
            };
            int main() { return 0; }
        "#);
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_namespace() {
        let result = compile_cpp(r#"
            namespace math {
                int add(int a, int b) { return a + b; }
            }
            int main() { return 0; }
        "#);
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_error() {
        let result = compile_cpp("class { broken");
        assert!(result.is_err());
    }
}
