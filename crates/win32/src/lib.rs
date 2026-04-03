//! # claudio-win32
//!
//! Native Win32 API compatibility layer for ClaudioOS.
//!
//! This crate implements Windows API functions (kernel32, user32, gdi32, ntdll,
//! ws2_32, advapi32, ole32, msvcrt) as pure Rust `no_std` code that runs directly
//! on ClaudioOS bare metal. No Wine, no Linux, no POSIX — just a direct mapping
//! from Win32 API calls to ClaudioOS kernel services.
//!
//! ## Architecture
//!
//! When a PE executable imports "kernel32.dll!CreateFileW", the PE loader patches
//! the Import Address Table to point to our Rust implementation of `CreateFileW`.
//! The function signature matches the Windows ABI (x64 calling convention: RCX,
//! RDX, R8, R9, stack) and returns values in RAX.
//!
//! ## Implemented DLLs
//!
//! - **ntdll.dll** — NT Native API (NtCreateFile, NtAllocateVirtualMemory, etc.)
//! - **kernel32.dll** — Core Win32 API (file I/O, process, thread, memory, sync)
//! - **user32.dll** — Window management and message loop (maps to framebuffer panes)
//! - **gdi32.dll** — 2D graphics (renders to framebuffer back buffer)
//! - **ws2_32.dll** — Winsock2 networking (maps to smoltcp TCP/IP stack)
//! - **advapi32.dll** — Registry and security (registry as VFS key-value tree)
//! - **ole32.dll** — COM runtime basics
//! - **msvcrt.dll** — C runtime (malloc, printf, fopen, string functions)

#![no_std]

extern crate alloc;

pub mod ntdll;
pub mod kernel32;
pub mod user32;
pub mod gdi32;
pub mod ws2_32;
pub mod advapi32;
pub mod ole32;
pub mod msvcrt;
pub mod registry;
pub mod teb_peb;
pub mod handles;
pub mod unicode;
pub mod directwrite;
pub mod direct2d;
pub mod wasapi;
pub mod xinput;
pub mod wic;
pub mod shell32;
pub mod winmm;
pub mod dispatcher;
