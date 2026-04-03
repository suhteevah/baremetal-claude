//! # claudio-pe-loader
//!
//! A `no_std` PE/COFF executable loader for ClaudioOS.
//!
//! Parses Windows PE32+ (x86_64) binaries: DOS header, PE signature, COFF header,
//! optional header, section headers, import/export tables, and base relocations.
//! Loads sections into memory, resolves imports against a provided symbol table,
//! applies relocations, sets up TEB/PEB structures, and returns the entry point.
//!
//! This is the Windows equivalent of our ELF loader (`claudio-elf-loader`).

#![no_std]

extern crate alloc;

pub mod dos_header;
pub mod pe_header;
pub mod sections;
pub mod imports;
pub mod exports;
pub mod relocations;
pub mod loader;

pub use dos_header::DosHeader;
pub use pe_header::{PeSignature, CoffHeader, OptionalHeader, DataDirectory, ImageSubsystem};
pub use sections::{SectionHeader, SectionFlags, MappedSection};
pub use imports::{ImportDirectory, ImportEntry, ImportLookup};
pub use exports::{ExportDirectory, ExportEntry};
pub use relocations::{BaseRelocation, RelocationType};
pub use loader::{LoadedPe, PeError, load_pe};
