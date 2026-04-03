//! WinRtRuntime: top-level driver for the Windows Runtime.
//!
//! Provides init and activate_instance functionality.

use alloc::string::String;

use crate::activation;
use crate::metadata;

/// WinRT runtime initialization error.
#[derive(Debug, Clone)]
pub enum WinRtError {
    /// Runtime not initialized.
    NotInitialized,
    /// Activation failed.
    ActivationFailed(i32),
    /// Class not found.
    ClassNotFound(String),
    /// Metadata error.
    MetadataError(String),
}

/// WinRT runtime state.
static RUNTIME_STATE: spin::Mutex<Option<WinRtState>> = spin::Mutex::new(None);

struct WinRtState {
    initialized: bool,
    metadata: metadata::WinRtMetadata,
}

/// Initialize the Windows Runtime.
///
/// Sets up the activation factory, registers built-in WinRT types,
/// and loads built-in metadata.
pub fn init() {
    log::info!("[winrt] Initializing Windows Runtime");

    // Initialize activation factory registry
    activation::init();

    // Register built-in factories
    activation::register_builtin_factories();

    // Load built-in metadata
    let meta = metadata::builtin_metadata();

    let factory_count = activation::with_registry(|r| r.factory_count());
    let type_count = meta.types.len();

    let state = WinRtState {
        initialized: true,
        metadata: meta,
    };
    *RUNTIME_STATE.lock() = Some(state);

    log::info!(
        "[winrt] Windows Runtime initialized: {} factories, {} types",
        factory_count, type_count,
    );
}

/// Activate a WinRT instance by class name.
///
/// Equivalent to RoActivateInstance.
///
/// # Arguments
/// * `class_name` — Fully qualified WinRT class name (e.g., "Windows.Foundation.Uri").
///
/// # Returns
/// Object handle, or an error.
pub fn activate_instance(class_name: &str) -> Result<u64, WinRtError> {
    let state = RUNTIME_STATE.lock();
    let state = state.as_ref().ok_or(WinRtError::NotInitialized)?;

    if !state.initialized {
        return Err(WinRtError::NotInitialized);
    }

    activation::ro_activate_instance(class_name)
        .map_err(WinRtError::ActivationFailed)
}

/// Check if a WinRT class is available for activation.
pub fn is_class_available(class_name: &str) -> bool {
    activation::ro_has_factory(class_name)
}

/// Get type information for a WinRT type.
pub fn get_type_info(full_name: &str) -> Option<metadata::WinRtTypeDef> {
    let state = RUNTIME_STATE.lock();
    state
        .as_ref()
        .and_then(|s| s.metadata.get_type(full_name).cloned())
}

/// Get runtime statistics.
pub fn stats() -> WinRtStats {
    let factory_count = activation::with_registry(|r| r.factory_count());
    let object_count = activation::with_registry(|r| r.object_count());
    let type_count = RUNTIME_STATE
        .lock()
        .as_ref()
        .map(|s| s.metadata.types.len())
        .unwrap_or(0);

    WinRtStats {
        factory_count,
        object_count,
        type_count,
    }
}

/// WinRT runtime statistics.
#[derive(Debug, Clone)]
pub struct WinRtStats {
    pub factory_count: usize,
    pub object_count: usize,
    pub type_count: usize,
}
