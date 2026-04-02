//! # claudio-smp — Symmetric Multi-Processing for ClaudioOS
//!
//! This crate provides bare-metal SMP support including:
//! - Local APIC and I/O APIC drivers
//! - AP (Application Processor) boot via trampoline
//! - Per-CPU data structures
//! - SMP-safe synchronization primitives
//! - Multi-core task scheduler with work stealing
//! - High-level SMP controller API

#![no_std]

extern crate alloc;

pub mod apic;
pub mod driver;
pub mod ioapic;
pub mod percpu;
pub mod scheduler;
pub mod spinlock;
pub mod trampoline;

pub use apic::LocalApic;
pub use driver::SmpController;
pub use ioapic::IoApic;
pub use percpu::{get_current_cpu, PerCpu};
pub use scheduler::{Scheduler, Task, TaskId, TaskState};
pub use spinlock::{Once, RwLock, SpinLock, TicketLock};
pub use trampoline::ApTrampoline;
