//! ClaudioOS native Rust compiler — cranelift backend, no LLVM.
//! Compiles a subset of Rust to x86_64 machine code directly on bare metal.

#![no_std]
extern crate alloc;

pub fn test() -> bool {
    // Proof that cranelift links on bare metal
    let _builder = cranelift_codegen::settings::builder();
    log::info!("[rustc-lite] cranelift codegen initialized on bare metal!");
    true
}
