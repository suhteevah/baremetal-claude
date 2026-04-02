//! # claudio-sshd — Post-Quantum SSH Daemon for ClaudioOS
//!
//! A `#![no_std]` SSH-2 server implementation (RFC 4253, 4252, 4254) with
//! hybrid post-quantum key exchange (ML-KEM-768 + X25519) and hybrid host
//! keys (ML-DSA-65 + Ed25519).
//!
//! This crate is designed to run bare-metal on ClaudioOS with no POSIX, no
//! libc, and no Linux kernel. It consumes raw TCP byte streams from the
//! smoltcp network stack.
//!
//! ## Protocol Stack
//!
//! ```text
//! ┌──────────────────────────────────┐
//! │         Channel Layer            │  RFC 4254: sessions, exec, pty
//! ├──────────────────────────────────┤
//! │       Authentication Layer       │  RFC 4252: publickey, password
//! ├──────────────────────────────────┤
//! │        Transport Layer           │  RFC 4253: packets, encryption
//! ├──────────────────────────────────┤
//! │    Key Exchange (Hybrid PQ)      │  ML-KEM-768 + X25519
//! ├──────────────────────────────────┤
//! │        Wire Format               │  SSH binary encoding
//! └──────────────────────────────────┘
//! ```

#![no_std]

extern crate alloc;

pub mod auth;
pub mod channel;
pub mod hostkey;
pub mod kex;
pub mod server;
pub mod session;
pub mod transport;
pub mod wire;

// Re-export primary types for ergonomic use from kernel
pub use server::{PaneCallback, SshConfig, SshServer};
pub use session::{SessionState, SshSession};
