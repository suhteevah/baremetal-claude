//! Cranelift control plane — minimal no_std stub.
//! The real implementation provides chaos mode for fuzzing.
//! We just need the ControlPlane type to exist (zero-sized, no-op).

#![no_std]

/// A zero-sized control plane that does nothing.
/// In the real crate, this drives randomized compilation decisions for fuzzing.
pub struct ControlPlane;

impl ControlPlane {
    pub fn default() -> Self { Self }
    pub fn new() -> Self { Self }
}

impl Default for ControlPlane {
    fn default() -> Self { Self }
}
