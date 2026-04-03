//! High-level API: load_wasm(bytes) -> WasmInstance, call_export(name, args) -> Vec<Value>.

use alloc::string::String;
use alloc::vec::Vec;

use crate::instance::WasmInstance;
use crate::types::Value;

/// Load a WASM binary and instantiate it, running the start function if present.
pub fn load_wasm(bytes: &[u8]) -> Result<WasmInstance, String> {
    log::info!("wasm: loading module ({} bytes)", bytes.len());
    let mut instance = WasmInstance::new(bytes)?;
    log::info!(
        "wasm: module loaded: {} types, {} imports, {} functions, {} exports",
        instance.module.types.len(),
        instance.module.imports.len(),
        instance.module.functions.len(),
        instance.module.exports.len(),
    );
    instance.run_start()?;
    Ok(instance)
}

/// Call an exported function by name on an instance.
pub fn call_export(
    instance: &mut WasmInstance,
    name: &str,
    args: &[Value],
) -> Result<Vec<Value>, String> {
    log::debug!("wasm: calling export '{}' with {} args", name, args.len());
    instance.call_export(name, args)
}

/// Call an exported function and capture any WASI stdout output.
pub fn call_export_with_stdout(
    instance: &mut WasmInstance,
    name: &str,
    args: &[Value],
) -> Result<(Vec<Value>, String), String> {
    let (results, stdout_bytes) = instance.call_export_with_stdout(name, args)?;
    let stdout = core::str::from_utf8(&stdout_bytes)
        .unwrap_or("<invalid utf-8>")
        .into();
    Ok((results, stdout))
}

/// Run a WASM module's _start function (WASI convention) and return stdout.
pub fn run_wasi(bytes: &[u8]) -> Result<String, String> {
    let mut instance = load_wasm(bytes)?;
    match instance.call_export_with_stdout("_start", &[]) {
        Ok((_, stdout)) => {
            let output = core::str::from_utf8(&stdout)
                .unwrap_or("<invalid utf-8>")
                .into();
            Ok(output)
        }
        Err(e) => {
            // proc_exit is signaled as an error
            if e.starts_with("proc_exit(0)") {
                let output = core::str::from_utf8(&instance.wasi_ctx.stdout_buf)
                    .unwrap_or("")
                    .into();
                Ok(output)
            } else if e.starts_with("proc_exit(") {
                Err(e)
            } else {
                Err(e)
            }
        }
    }
}
