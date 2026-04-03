//! DotNetRuntime: top-level driver for the .NET CLR.
//!
//! Provides init, load_assembly, run_entry_point, and create_appdomain
//! functionality.

use alloc::string::String;
use alloc::vec::Vec;

use crate::assembly::{self, AssemblyError};
use crate::bcl;
use crate::gc;
use crate::interop;
use crate::jit;
use crate::types;

/// .NET runtime initialization error.
#[derive(Debug, Clone)]
pub enum RuntimeError {
    /// Assembly loading failed.
    AssemblyError(AssemblyError),
    /// No entry point found in the loaded assembly.
    NoEntryPoint,
    /// Execution error.
    ExecutionError(String),
    /// Runtime not initialized.
    NotInitialized,
}

impl From<AssemblyError> for RuntimeError {
    fn from(e: AssemblyError) -> Self {
        RuntimeError::AssemblyError(e)
    }
}

/// AppDomain: an isolated execution environment for .NET code.
///
/// In the real CLR, AppDomains provide isolation. In our bare-metal
/// implementation, they serve as a logical container for loaded assemblies
/// and their state.
#[derive(Debug)]
pub struct AppDomain {
    /// Domain name.
    pub name: String,
    /// Loaded assembly names in this domain.
    pub assemblies: Vec<String>,
    /// Whether this domain is the default domain.
    pub is_default: bool,
}

impl AppDomain {
    /// Create a new AppDomain.
    pub fn new(name: String) -> Self {
        Self {
            name,
            assemblies: Vec::new(),
            is_default: false,
        }
    }
}

/// The .NET runtime state.
static RUNTIME_STATE: spin::Mutex<Option<DotNetRuntimeState>> = spin::Mutex::new(None);

struct DotNetRuntimeState {
    initialized: bool,
    default_domain: AppDomain,
    domains: Vec<AppDomain>,
}

/// Initialize the .NET CLR runtime.
///
/// Sets up the type system, garbage collector, BCL, JIT compiler,
/// assembly loader, and P/Invoke interop.
pub fn init() {
    log::info!("[dotnet-clr] Initializing .NET Common Language Runtime");

    // Initialize subsystems
    types::init();
    gc::init();
    bcl::init();
    bcl::init_filesystem();
    jit::init();
    assembly::init();
    interop::init();

    // Create default AppDomain
    let default_domain = AppDomain {
        name: String::from("DefaultDomain"),
        assemblies: Vec::new(),
        is_default: true,
    };

    let state = DotNetRuntimeState {
        initialized: true,
        default_domain,
        domains: Vec::new(),
    };

    *RUNTIME_STATE.lock() = Some(state);

    // Log type registry stats
    let type_count = types::with_registry(|reg| reg.type_count());
    log::info!(
        "[dotnet-clr] CLR initialized: {} built-in types, GC ready, JIT ready",
        type_count
    );
}

/// Load a .NET assembly from raw PE bytes.
///
/// # Arguments
/// * `pe_data` — Raw PE file bytes (.exe or .dll).
/// * `is_entry` — Whether this is the entry assembly (contains Main).
///
/// # Returns
/// The assembly name, or an error.
pub fn load_assembly(pe_data: &[u8], is_entry: bool) -> Result<String, RuntimeError> {
    let mut state = RUNTIME_STATE.lock();
    let state = state.as_mut().ok_or(RuntimeError::NotInitialized)?;

    if !state.initialized {
        return Err(RuntimeError::NotInitialized);
    }

    let name = assembly::with_store_mut(|store| store.load_assembly(pe_data, is_entry))?;
    state.default_domain.assemblies.push(name.clone());

    log::info!("[dotnet-clr] Assembly '{}' loaded into DefaultDomain", name);
    Ok(name)
}

/// Run the entry point (Main method) of the loaded entry assembly.
///
/// # Arguments
/// * `args` — Command-line arguments to pass to Main.
///
/// # Returns
/// The process exit code.
pub fn run_entry_point(args: &[&str]) -> Result<i32, RuntimeError> {
    let state = RUNTIME_STATE.lock();
    let state = state.as_ref().ok_or(RuntimeError::NotInitialized)?;

    if !state.initialized {
        return Err(RuntimeError::NotInitialized);
    }

    // Find the entry assembly
    let entry_token = assembly::with_store(|store| {
        store
            .entry_assembly()
            .map(|asm| asm.entry_point_token)
            .unwrap_or(0)
    });

    if entry_token == 0 {
        return Err(RuntimeError::NoEntryPoint);
    }

    log::info!(
        "[dotnet-clr] Running entry point token=0x{:08X} with {} args",
        entry_token,
        args.len()
    );

    // In a full implementation, we would:
    // 1. Resolve the entry point token to a MethodDef.
    // 2. Parse the method body.
    // 3. Set up the args (string[] args for Main).
    // 4. Execute via CIL interpreter or JIT.
    //
    // For now, log and return success.

    log::info!("[dotnet-clr] Entry point execution placeholder — returning 0");
    Ok(0)
}

/// Create a new AppDomain.
///
/// # Arguments
/// * `name` — Domain name.
///
/// # Returns
/// The domain index.
pub fn create_appdomain(name: &str) -> Result<usize, RuntimeError> {
    let mut state = RUNTIME_STATE.lock();
    let state = state.as_mut().ok_or(RuntimeError::NotInitialized)?;

    let domain = AppDomain::new(String::from(name));
    state.domains.push(domain);
    let index = state.domains.len() - 1;

    log::info!("[dotnet-clr] Created AppDomain '{}' (index={})", name, index);
    Ok(index)
}

/// Get runtime statistics.
pub fn stats() -> RuntimeStats {
    let type_count = types::with_registry(|r| r.type_count());
    let gc_stats = gc::with_heap(|h| h.stats());
    let jit_stats = jit::stats();
    let assembly_count = assembly::with_store(|s| s.assembly_count());
    let pinvoke_count = interop::with_registry(|r| r.function_count());

    RuntimeStats {
        type_count,
        assembly_count,
        gc_object_count: gc_stats.object_count,
        gc_collections: gc_stats.collections,
        jit_compiled_methods: jit_stats.cached_methods,
        jit_code_bytes: jit_stats.total_code_bytes,
        pinvoke_functions: pinvoke_count,
    }
}

/// Runtime statistics.
#[derive(Debug, Clone)]
pub struct RuntimeStats {
    pub type_count: usize,
    pub assembly_count: usize,
    pub gc_object_count: usize,
    pub gc_collections: u64,
    pub jit_compiled_methods: usize,
    pub jit_code_bytes: usize,
    pub pinvoke_functions: usize,
}
