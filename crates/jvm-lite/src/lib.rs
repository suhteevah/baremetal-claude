//! claudio-jvm-lite — Minimal JVM bytecode interpreter for ClaudioOS.
//!
//! Parses .class files (magic 0xCAFEBABE), interprets JVM bytecodes on a
//! stack-based VM with operand stack, local variables, and exception handling.
//! Includes mark-sweep GC and basic java.lang/java.util stdlib stubs.
//!
//! This is NOT HotSpot. It is enough for an AI agent to run Java on bare metal.

#![no_std]
#![allow(unused_variables, unused_assignments)]

extern crate alloc;

pub mod classfile;
pub mod bytecode;
pub mod vm;
pub mod gc;
pub mod classloader;
pub mod stdlib;
pub mod driver;

pub use driver::JvmRuntime;
