//! .NET CLR and WinRT compatibility layer for ClaudioOS.
//!
//! Initializes the .NET Common Language Runtime and Windows Runtime subsystems,
//! enabling ClaudioOS to load and run .NET assemblies (.dll/.exe) and WinRT
//! components on bare metal.
//!
//! ## How it works
//!
//! 1. **Init**: Set up the CLR (type system, GC, BCL, JIT, assembly loader,
//!    P/Invoke interop) and WinRT (activation factories, type projections).
//! 2. **Assembly Loading**: Parse the .NET PE metadata, build the type graph.
//! 3. **Execution**: Run the entry point via CIL interpreter or JIT compiler.
//! 4. **WinRT**: Activate WinRT objects by class name for UWP-style APIs.

/// Initialize the .NET CLR and WinRT subsystems.
///
/// Must be called during kernel boot after Win32 subsystem is initialized.
pub fn init() {
    log::info!("[dotnet-compat] Initializing .NET CLR and WinRT subsystems");

    // Initialize the .NET CLR
    claudio_dotnet_clr::driver::init();

    // Initialize the Windows Runtime
    claudio_winrt::driver::init();

    // Log stats
    let clr_stats = claudio_dotnet_clr::driver::stats();
    let winrt_stats = claudio_winrt::driver::stats();

    log::info!(
        "[dotnet-compat] .NET CLR ready: {} types, GC initialized, JIT ready",
        clr_stats.type_count,
    );
    log::info!(
        "[dotnet-compat] WinRT ready: {} factories, {} type definitions",
        winrt_stats.factory_count,
        winrt_stats.type_count,
    );
}

/// Load and run a .NET assembly.
///
/// # Arguments
/// * `pe_data` — Raw PE file bytes (.exe or .dll).
/// * `args` — Command-line arguments.
///
/// # Returns
/// The process exit code, or an error string.
pub fn run_dotnet_assembly(pe_data: &[u8], args: &[&str]) -> Result<i32, &'static str> {
    log::info!("[dotnet-compat] Loading .NET assembly ({} bytes)", pe_data.len());

    // Load the assembly
    let name = claudio_dotnet_clr::driver::load_assembly(pe_data, true)
        .map_err(|e| {
            log::error!("[dotnet-compat] Assembly load failed: {:?}", e);
            "Failed to load .NET assembly"
        })?;

    log::info!("[dotnet-compat] Assembly '{}' loaded, running entry point", name);

    // Run entry point
    let exit_code = claudio_dotnet_clr::driver::run_entry_point(args)
        .map_err(|e| {
            log::error!("[dotnet-compat] Execution failed: {:?}", e);
            "Failed to execute .NET assembly"
        })?;

    log::info!("[dotnet-compat] .NET process exited with code {}", exit_code);
    Ok(exit_code)
}

/// Activate a WinRT object by class name.
///
/// # Arguments
/// * `class_name` — Fully qualified WinRT class name (e.g., "Windows.Foundation.Uri").
///
/// # Returns
/// Object handle, or an error string.
pub fn activate_winrt_instance(class_name: &str) -> Result<u64, &'static str> {
    claudio_winrt::driver::activate_instance(class_name)
        .map_err(|_| "WinRT activation failed")
}
