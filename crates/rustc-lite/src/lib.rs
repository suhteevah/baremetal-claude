//! ClaudioOS native Rust compiler -- Cranelift backend, no LLVM.
//!
//! Compiles a subset of Rust to x86_64 machine code directly on bare metal.
//! This crate proves that Cranelift (a code generator originally built for
//! Firefox's WebAssembly engine) can run in a `no_std` environment with no
//! operating system support.
//!
//! ## Architecture
//!
//! ```text
//! Rust source -> (future: parser + type checker) -> Cranelift IR -> x86_64 machine code
//! ```
//!
//! Currently this crate provides a JIT proof-of-concept that:
//! 1. Constructs Cranelift IR programmatically (no parser yet)
//! 2. Compiles IR to native x86_64 machine code via `cranelift-codegen`
//! 3. Copies the machine code to heap memory (which is executable on bare metal
//!    since we have no W^X page protection)
//! 4. Casts the code pointer to a function pointer and calls it
//!
//! ## Cranelift JIT Pipeline
//!
//! 1. **ISA creation**: `isa::lookup_by_name("x86_64")` creates an x86_64 backend
//!    with optimization level "speed"
//! 2. **Signature**: Define the function's ABI (System V AMD64 calling convention)
//! 3. **IR construction**: Use `FunctionBuilder` to create basic blocks, define
//!    parameters as block params, emit instructions (`iadd`, `return_`)
//! 4. **Compilation**: `Context::compile()` runs register allocation, instruction
//!    selection, and machine code emission
//! 5. **Execution**: Copy `code_buffer()` to executable memory and call via `transmute`

#![no_std]
extern crate alloc;

use alloc::vec;
use cranelift_codegen::ir::types::I64;
use cranelift_codegen::ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName};
use cranelift_codegen::isa::{self, CallConv};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

/// Smoke test: verify that Cranelift's settings builder can be created on bare metal.
///
/// This is the simplest possible test that the Cranelift crate links and
/// initializes correctly in a `no_std` environment.
pub fn test() -> bool {
    let _builder = cranelift_codegen::settings::builder();
    log::info!("[rustc-lite] cranelift codegen initialized on bare metal!");
    true
}

/// Full JIT proof-of-concept: build, compile, and execute a function at runtime.
///
/// Constructs `fn add(a: i64, b: i64) -> i64 { a + b }` using Cranelift IR,
/// compiles it to x86_64 machine code, copies the code to executable memory,
/// calls `add(3, 4)`, and asserts the result is 7.
///
/// ## Why This Works on Bare Metal
///
/// ClaudioOS uses a simple identity-mapped page table with no W^X enforcement,
/// so heap-allocated memory is executable. The compiled code is leaked
/// (`core::mem::forget`) to prevent the allocator from freeing it while the
/// CPU is executing it.
///
/// Returns `true` on success, `false` if any step fails.
pub fn test_jit() -> bool {
    log::info!("[jit] ============================================");
    log::info!("[jit] CRANELIFT JIT PROOF-OF-CONCEPT");
    log::info!("[jit] ============================================");

    // 1. Create ISA (instruction set architecture) for x86_64
    log::info!("[jit] step 1: creating x86_64 ISA...");
    let mut flag_builder = settings::builder();
    flag_builder.set("opt_level", "speed").unwrap();
    let flags = settings::Flags::new(flag_builder);
    let isa = match isa::lookup_by_name("x86_64") {
        Ok(builder) => match builder.finish(flags) {
            Ok(isa) => isa,
            Err(e) => {
                log::error!("[jit] ISA finish failed: {:?}", e);
                return false;
            }
        },
        Err(e) => {
            log::error!("[jit] ISA lookup failed: {:?}", e);
            return false;
        }
    };
    log::info!("[jit] ISA created: {}", isa.triple());

    // 2. Create function signature: fn(i64, i64) -> i64
    log::info!("[jit] step 2: creating function signature...");
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(I64));
    sig.params.push(AbiParam::new(I64));
    sig.returns.push(AbiParam::new(I64));
    log::info!("[jit] signature: (i64, i64) -> i64");

    // 3. Create function and build IR using FunctionBuilder
    log::info!("[jit] step 3: building Cranelift IR...");
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
    log::info!("[jit] IR built: fn add(a, b) => iadd(a, b)");

    // 4. Compile the function to machine code
    log::info!("[jit] step 4: compiling to x86_64 machine code...");
    let mut ctx = Context::for_function(func);
    let compiled = match ctx.compile(&*isa, &mut Default::default()) {
        Ok(code) => code,
        Err(e) => {
            log::error!("[jit] compilation failed: {:?}", e);
            return false;
        }
    };

    let bytes = compiled.code_buffer();
    log::info!("[jit] compilation succeeded! {} bytes of machine code", bytes.len());

    // Log first few bytes of machine code as hex for debugging
    if bytes.len() > 0 {
        let mut hex = alloc::string::String::new();
        for (i, b) in bytes.iter().enumerate().take(32) {
            if i > 0 {
                hex.push(' ');
            }
            use core::fmt::Write;
            let _ = write!(hex, "{:02x}", b);
        }
        if bytes.len() > 32 {
            hex.push_str("...");
        }
        log::info!("[jit] machine code: {}", hex);
    }

    // 5. Copy machine code to executable memory and call it
    // On bare metal, heap memory is executable (no W^X enforcement in our
    // simple page table), so we just allocate, copy, and jump.
    log::info!("[jit] step 5: copying to executable memory...");
    let mut code_mem = vec![0u8; bytes.len()];
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), code_mem.as_mut_ptr(), bytes.len());
    }
    let ptr = code_mem.as_ptr();
    // Leak the memory so it isn't freed while we execute it
    core::mem::forget(code_mem);

    log::info!("[jit] code at address: {:#x}", ptr as usize);

    // 6. Cast to function pointer and call
    log::info!("[jit] step 6: calling add(3, 4)...");
    let func_ptr: fn(i64, i64) -> i64 = unsafe { core::mem::transmute(ptr) };
    let result = func_ptr(3, 4);
    log::info!("[jit] add(3, 4) = {}", result);

    if result == 7 {
        log::info!("[jit] ============================================");
        log::info!("[jit] JIT SUCCESS: Cranelift generated working x86_64 code!");
        log::info!("[jit] ============================================");
        true
    } else {
        log::error!("[jit] WRONG RESULT: expected 7, got {}", result);
        false
    }
}
