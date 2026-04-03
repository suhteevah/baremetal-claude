//! claudio-cpp-lite — Minimal C++17 compiler extending cc-lite.
//!
//! Adds classes (with inheritance, vtables, constructors/destructors),
//! templates (monomorphization), namespaces, operator overloading,
//! references, lambdas, RAII, and a mini STL (string, vector, map,
//! cout/cin, unique_ptr, shared_ptr).
//!
//! Targets x86_64 via Cranelift, reuses cc-lite's C frontend.

#![no_std]
#![allow(unused_variables, unused_assignments)]

extern crate alloc;

pub mod lexer;
pub mod parser;
pub mod ast;
pub mod name_mangling;
pub mod vtable;
pub mod templates;
pub mod stdlib;
pub mod driver;

pub use driver::{compile_cpp, execute_cpp, CppCompileError, CompiledCppProgram};
