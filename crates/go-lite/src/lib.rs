//! claudio-go-lite — Minimal Go compiler targeting x86_64 via Cranelift.
//!
//! Compiles a useful subset of Go: packages, functions, variables, control flow,
//! goroutines (cooperative), channels, defer/panic/recover, slices, maps, structs,
//! interfaces. Enough for an AI agent to write and run Go on bare metal.
//!
//! This is NOT gc-compatible. It is "Go-shaped" enough for practical use.

#![no_std]
#![allow(unused_variables, unused_assignments)]

extern crate alloc;

pub mod lexer;
pub mod parser;
pub mod ast;
pub mod types;
pub mod codegen;
pub mod runtime;
pub mod stdlib;
pub mod driver;

pub use driver::{compile, execute, CompileError, CompiledProgram};
