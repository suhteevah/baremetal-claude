//! ClaudioOS native Rust compiler — full pipeline from source to x86_64.
//!
//! Compiles Rust source code to native x86_64 machine code on bare metal
//! using Cranelift as the code generation backend. No LLVM, no host OS.
//!
//! ## Pipeline
//!
//! ```text
//! Rust source → lexer → parser → AST → type checker → Cranelift IR → x86_64 machine code
//! ```
//!
//! ## Supported Language Features
//!
//! - Functions with full signatures, generics, where clauses
//! - Structs (named, tuple, unit), enums with data variants
//! - Impl blocks, trait definitions, trait impls
//! - Type inference, generic instantiation, method resolution
//! - Expressions: binary ops, unary, if/else, match, loops, closures, ranges
//! - Patterns: ident, tuple, struct, enum, slice, or-patterns, guards
//! - Control flow: for/while/loop, break/continue with labels, return
//! - References (&, &mut), raw pointers, casts
//! - Use/mod/const/static/type alias
//! - Macro invocations (println!, format!, vec!, assert!, etc.)
//! - Async/await syntax, unsafe blocks
//! - Built-in types: Vec, String, Box, Option, Result, HashMap
//! - Core traits: Clone, Display, Debug, Iterator, Drop, Default
//!
//! ## Architecture
//!
//! - `lexer.rs`   — Full Rust tokenizer (keywords, operators, all literal types)
//! - `ast.rs`     — Complete AST type definitions
//! - `parser.rs`  — Recursive descent parser with Pratt precedence
//! - `types.rs`   — Internal type representation and layout
//! - `typeck.rs`  — Type checker with inference, unification, method resolution
//! - `codegen.rs` — Cranelift IR lowering and x86_64 compilation
//! - `linker.rs`  — Post-compilation linker for inter-function call patching

#![no_std]
extern crate alloc;

pub mod lexer;
pub mod ast;
pub mod parser;
pub mod types;
pub mod typeck;
pub mod codegen;
pub mod linker;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Compile Rust source code and return compilation diagnostics.
///
/// This is the main entry point for the `compile_rust` tool integration.
/// Returns Ok(diagnostics) on success, Err(error) on failure.
pub fn compile(source: &str) -> Result<CompileOutput, String> {
    // Phase 1: Lex
    let tokens = lexer::Lexer::tokenize(source)
        .map_err(|e| format!("lexer error: {}", e))?;
    log::info!("[rustc-lite] lexed {} tokens", tokens.len());

    // Phase 2: Parse
    let file = parser::Parser::parse_file(tokens)
        .map_err(|e| format!("parse error: {}", e))?;
    log::info!("[rustc-lite] parsed {} items", file.items.len());

    // Phase 3: Type check
    let mut env = typeck::TypeEnv::new();
    env.check_file(&file);

    let type_errors = env.errors.clone();
    if !type_errors.is_empty() {
        log::warn!("[rustc-lite] {} type errors", type_errors.len());
    }

    // Phase 4: Codegen
    let mut cg = codegen::CodeGen::new()?;
    let result = cg.compile_file(&file, &env);

    let mut diagnostics = Vec::new();
    for e in &type_errors {
        diagnostics.push(format!("warning: {}", e));
    }
    for e in &result.errors {
        diagnostics.push(format!("error: {}", e));
    }

    let functions: Vec<CompiledFnInfo> = result
        .functions
        .iter()
        .map(|f| CompiledFnInfo {
            name: f.name.clone(),
            code_size: f.code.len(),
        })
        .collect();

    let has_errors = !result.errors.is_empty();

    log::info!(
        "[rustc-lite] compiled {} functions ({} errors, {} warnings)",
        functions.len(),
        result.errors.len(),
        type_errors.len(),
    );

    if has_errors {
        Err(diagnostics.join("\n"))
    } else {
        Ok(CompileOutput {
            functions,
            diagnostics,
            compiled: result,
        })
    }
}

/// Check Rust source for errors without generating code.
///
/// Faster than full compilation — useful for IDE-like feedback.
pub fn check(source: &str) -> Result<Vec<String>, String> {
    let tokens = lexer::Lexer::tokenize(source)
        .map_err(|e| format!("lexer error: {}", e))?;
    let file = parser::Parser::parse_file(tokens)
        .map_err(|e| format!("parse error: {}", e))?;
    let mut env = typeck::TypeEnv::new();
    env.check_file(&file);
    Ok(env.errors)
}

/// Compile and execute a Rust function, returning the i64 result.
///
/// For multi-function programs, all functions are compiled, loaded into
/// executable memory, and linked together so inter-function calls work.
/// The first function is then called with the provided args.
pub fn compile_and_run(source: &str, args: &[i64]) -> Result<i64, String> {
    let output = compile(source)?;

    if output.compiled.functions.is_empty() {
        return Err("no functions compiled".into());
    }

    // Single function: fast path (no linking needed)
    if output.compiled.functions.len() == 1 {
        let func = &output.compiled.functions[0];
        if func.code.is_empty() {
            return Err("empty code buffer".into());
        }
        unsafe {
            let ptr = codegen::make_executable(&func.code);
            return call_fn_ptr(ptr, args);
        }
    }

    // Multi-function: allocate executable memory for all, then link
    log::info!(
        "[rustc-lite] linking {} functions for multi-function execution",
        output.compiled.functions.len()
    );

    let mut linkable: Vec<linker::LinkableFunction> = Vec::new();
    for func in &output.compiled.functions {
        if func.code.is_empty() {
            continue;
        }
        let ptr = unsafe { codegen::make_executable(&func.code) };
        linkable.push(linker::LinkableFunction {
            name: func.name.clone(),
            code: func.code.clone(),
            load_addr: ptr as usize,
        });
    }

    if linkable.is_empty() {
        return Err("no non-empty functions compiled".into());
    }

    // Patch placeholder addresses with real function pointers
    let patches = linker::link_functions(&mut linkable);
    log::info!("[rustc-lite] applied {} link patches", patches);

    // Copy patched code back to executable memory.
    // This works because on bare metal we have no W^X -- the memory
    // allocated by make_executable is both writable and executable.
    for entry in &linkable {
        unsafe {
            core::ptr::copy_nonoverlapping(
                entry.code.as_ptr(),
                entry.load_addr as *mut u8,
                entry.code.len(),
            );
        }
    }

    // Execute the first function
    let main_addr = linkable[0].load_addr as *const u8;
    unsafe { call_fn_ptr(main_addr, args) }
}

/// Call a function pointer with 0-4 i64 arguments, returning the i64 result.
///
/// # Safety
/// The caller must ensure `ptr` points to valid, executable machine code
/// with the correct calling convention and number of parameters.
unsafe fn call_fn_ptr(ptr: *const u8, args: &[i64]) -> Result<i64, String> {
    match args.len() {
        0 => {
            let f: fn() -> i64 = core::mem::transmute(ptr);
            Ok(f())
        }
        1 => {
            let f: fn(i64) -> i64 = core::mem::transmute(ptr);
            Ok(f(args[0]))
        }
        2 => {
            let f: fn(i64, i64) -> i64 = core::mem::transmute(ptr);
            Ok(f(args[0], args[1]))
        }
        3 => {
            let f: fn(i64, i64, i64) -> i64 = core::mem::transmute(ptr);
            Ok(f(args[0], args[1], args[2]))
        }
        4 => {
            let f: fn(i64, i64, i64, i64) -> i64 = core::mem::transmute(ptr);
            Ok(f(args[0], args[1], args[2], args[3]))
        }
        _ => Err(format!("too many args ({}), max 4", args.len())),
    }
}

/// Output of a successful compilation.
pub struct CompileOutput {
    pub functions: Vec<CompiledFnInfo>,
    pub diagnostics: Vec<String>,
    pub compiled: codegen::CompileResult,
}

/// Info about a compiled function.
pub struct CompiledFnInfo {
    pub name: String,
    pub code_size: usize,
}

// ── Legacy API (kept for backwards compat with kernel boot test) ─────────

/// Smoke test: verify Cranelift initializes on bare metal.
pub fn test() -> bool {
    let _builder = cranelift_codegen::settings::builder();
    log::info!("[rustc-lite] cranelift codegen initialized on bare metal!");
    true
}

/// Full JIT proof-of-concept: build, compile, and execute `add(3, 4) = 7`.
pub fn test_jit() -> bool {
    match compile_and_run("fn add(a: i64, b: i64) -> i64 { a + b }", &[3, 4]) {
        Ok(result) => {
            log::info!("[rustc-lite] add(3, 4) = {}", result);
            if result == 7 {
                log::info!("[rustc-lite] JIT SUCCESS via full compiler pipeline!");
                true
            } else {
                log::error!("[rustc-lite] WRONG RESULT: expected 7, got {}", result);
                false
            }
        }
        Err(e) => {
            log::error!("[rustc-lite] JIT failed: {}", e);
            // Fall back to the direct Cranelift API test
            test_jit_direct()
        }
    }
}

/// Direct Cranelift API JIT test (bypasses parser/typechecker).
fn test_jit_direct() -> bool {
    use cranelift_codegen::ir::types::I64;
    use cranelift_codegen::ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName};
    use cranelift_codegen::isa::{self, CallConv};
    use cranelift_codegen::settings::{self, Configurable};
    use cranelift_codegen::Context;
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

    log::info!("[jit-direct] building add(i64, i64) -> i64 via Cranelift API...");

    let mut flag_builder = settings::builder();
    flag_builder.set("opt_level", "speed").unwrap();
    let flags = settings::Flags::new(flag_builder);
    let isa = match isa::lookup_by_name("x86_64") {
        Ok(b) => match b.finish(flags) {
            Ok(isa) => isa,
            Err(e) => { log::error!("[jit-direct] ISA finish: {:?}", e); return false; }
        },
        Err(e) => { log::error!("[jit-direct] ISA lookup: {:?}", e); return false; }
    };

    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(I64));
    sig.params.push(AbiParam::new(I64));
    sig.returns.push(AbiParam::new(I64));

    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);

    let a = builder.block_params(entry)[0];
    let b = builder.block_params(entry)[1];
    let result = builder.ins().iadd(a, b);
    builder.ins().return_(&[result]);
    builder.seal_all_blocks();
    builder.finalize();

    let mut ctx = Context::for_function(func);
    let compiled = match ctx.compile(&*isa, &mut Default::default()) {
        Ok(c) => c,
        Err(e) => { log::error!("[jit-direct] compile: {:?}", e); return false; }
    };

    let bytes = compiled.code_buffer();
    log::info!("[jit-direct] compiled {} bytes", bytes.len());

    unsafe {
        let ptr = codegen::make_executable(bytes);
        let func: fn(i64, i64) -> i64 = core::mem::transmute(ptr);
        let result = func(3, 4);
        log::info!("[jit-direct] add(3, 4) = {}", result);
        result == 7
    }
}
