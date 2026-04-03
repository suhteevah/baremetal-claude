//! Top-level compile + execute API for Go.

use alloc::string::String;
use alloc::vec::Vec;

use crate::ast::TopLevelDecl;
use crate::codegen::{CodeGen, CompiledFunc};
use crate::lexer::tokenize;
use crate::parser::parse_tokens;

/// Error from compilation.
#[derive(Debug)]
pub struct CompileError {
    pub message: String,
}

impl core::fmt::Display for CompileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "go error: {}", self.message)
    }
}

/// A compiled Go program ready to execute.
pub struct CompiledProgram {
    /// Compiled function code.
    pub functions: Vec<CompiledFunc>,
    /// Entry point function name (default: "main").
    pub entry: String,
    /// Combined code buffer (all functions concatenated).
    pub code: Vec<u8>,
    /// Entry point offset in the code buffer.
    pub entry_offset: usize,
}

/// Compile Go source code to executable machine code.
pub fn compile(source: &str) -> Result<CompiledProgram, CompileError> {
    log::info!("[go] compiling {} bytes of Go source", source.len());

    // 1. Tokenize
    let tokens = tokenize(source).map_err(|e| CompileError { message: e })?;
    log::debug!("[go] lexed {} tokens", tokens.len());

    // 2. Parse
    let pkg = parse_tokens(&tokens).map_err(|e| CompileError { message: e })?;
    log::debug!("[go] parsed package '{}' with {} declarations", pkg.name, pkg.decls.len());

    // 3. Code generation
    let mut codegen = CodeGen::new();
    let mut functions = Vec::new();

    for decl in &pkg.decls {
        if let TopLevelDecl::Func(func) = decl {
            let compiled = codegen
                .compile_function(func)
                .map_err(|e| CompileError { message: e })?;
            functions.push(compiled);
        }
    }

    // 4. Combine into a single code buffer
    let mut code = Vec::new();
    let mut entry_offset = 0usize;
    let entry_name = String::from("main");

    for func in &functions {
        if func.name == entry_name {
            entry_offset = code.len();
        }
        code.extend_from_slice(&func.code);
    }

    log::info!(
        "[go] compilation complete: {} functions, {} bytes total",
        functions.len(),
        code.len()
    );

    Ok(CompiledProgram {
        functions,
        entry: entry_name,
        code,
        entry_offset,
    })
}

/// Execute a compiled Go program.
///
/// # Safety
/// Runs compiled machine code with full kernel privileges.
pub unsafe fn execute(program: &CompiledProgram) -> i64 {
    if program.code.is_empty() {
        log::warn!("[go] no code to execute");
        return -1;
    }

    log::info!(
        "[go] executing compiled Go program (entry: {}, offset: {}, {} bytes)",
        program.entry,
        program.entry_offset,
        program.code.len()
    );

    // Copy to executable memory
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

    // Leak to prevent deallocation while executing
    core::mem::forget(code_mem);

    let func: fn() -> i64 = unsafe { core::mem::transmute(entry) };
    let result = func();

    log::info!("[go] program returned: {}", result);
    result
}

/// Compile and immediately execute Go source code.
///
/// # Safety
/// See `execute`.
pub unsafe fn compile_and_run(source: &str) -> Result<i64, CompileError> {
    let program = compile(source)?;
    Ok(unsafe { execute(&program) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_hello() {
        let result = compile(r#"
            package main
            func main() int {
                return 42
            }
        "#);
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert!(!prog.code.is_empty());
        assert_eq!(prog.entry, "main");
    }

    #[test]
    fn test_compile_with_variables() {
        let result = compile(r#"
            package main
            func main() int {
                x := 10
                y := 20
                return x + y
            }
        "#);
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_if() {
        let result = compile(r#"
            package main
            func main() int {
                x := 5
                if x > 3 {
                    return 1
                } else {
                    return 0
                }
            }
        "#);
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_for_loop() {
        let result = compile(r#"
            package main
            func main() int {
                sum := 0
                for i := 0; i < 10; i++ {
                    sum += i
                }
                return sum
            }
        "#);
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_multiple_functions() {
        let result = compile(r#"
            package main
            func add(a int, b int) int {
                return a + b
            }
            func main() int {
                return add(3, 4)
            }
        "#);
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.functions.len(), 2);
    }

    #[test]
    fn test_compile_error() {
        let result = compile("not valid go");
        assert!(result.is_err());
    }
}
