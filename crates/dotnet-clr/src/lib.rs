//! # claudio-dotnet-clr
//!
//! A `no_std` .NET Common Language Runtime (CLR) for ClaudioOS.
//!
//! Provides the ability to load and execute .NET assemblies (.dll/.exe) on bare
//! metal. Implements the ECMA-335 standard: PE metadata parsing, CIL interpreter,
//! .NET type system, mark-and-sweep garbage collector, Base Class Library stubs,
//! JIT compilation via Cranelift, assembly loading, and P/Invoke interop.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │           DotNetRuntime (driver)         │
//! ├──────────┬──────────┬───────────────────┤
//! │ Assembly │   JIT    │     Interop       │
//! │ Loader   │ Compiler │    (P/Invoke)     │
//! ├──────────┴──────────┴───────────────────┤
//! │  CIL Interpreter  │  Base Class Library │
//! ├───────────────────┬─────────────────────┤
//! │   Type System     │  Garbage Collector  │
//! ├───────────────────┴─────────────────────┤
//! │         PE Metadata Parser              │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## How it works
//!
//! 1. **Assembly Loading**: Parse the PE file, locate CLI header (data directory 14),
//!    read ECMA-335 metadata (streams, tables).
//! 2. **Type Resolution**: Build the type graph from TypeDef/TypeRef/MemberRef tables.
//! 3. **Execution**: Interpret CIL bytecode or JIT-compile hot methods via Cranelift.
//! 4. **GC**: Mark-and-sweep collector for managed heap objects.
//! 5. **BCL**: Stub implementations of System.Console, System.String, System.Math, etc.
//! 6. **P/Invoke**: Marshal managed calls to native functions (DllImport).

#![no_std]

extern crate alloc;

pub mod pe_metadata;
pub mod il;
pub mod types;
pub mod gc;
pub mod bcl;
pub mod jit;
pub mod assembly;
pub mod interop;
pub mod driver;
