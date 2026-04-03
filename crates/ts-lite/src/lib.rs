//! claudio-ts-lite — TypeScript type checker and transpiler for ClaudioOS.
//!
//! Layers TypeScript syntax (type annotations, interfaces, enums, generics)
//! on top of js-lite. Type checks with warnings (gradual typing), then strips
//! types and transforms to plain JS for execution via js-lite.
//!
//! This is NOT tsc-compatible. It is "TypeScript-shaped" enough for practical use.

#![no_std]
#![allow(unused_variables, unused_assignments)]

extern crate alloc;

pub mod lexer;
pub mod type_checker;
pub mod transformer;
pub mod driver;

pub use driver::execute_ts;
