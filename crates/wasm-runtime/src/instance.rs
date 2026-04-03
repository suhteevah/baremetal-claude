//! WasmInstance: instantiate module, link imports, execute start function.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

use crate::binary::parse_wasm;
use crate::interpreter::{eval_const_expr, Interpreter};
use crate::memory::LinearMemory;
use crate::module::*;
use crate::table::Table;
use crate::types::*;
use crate::wasi::WasiCtx;

/// A host-provided import function.
pub type HostFn = fn(&[Value], &mut WasiCtx, &mut Option<LinearMemory>) -> Result<Vec<Value>, String>;

/// An import binding.
pub struct ImportBinding {
    pub module: String,
    pub name: String,
    pub func: HostFn,
}

/// A running WebAssembly instance.
pub struct WasmInstance {
    pub module: WasmModule,
    pub memories: Vec<LinearMemory>,
    pub tables: Vec<Table>,
    pub globals: Vec<Value>,
    pub import_bindings: Vec<ImportBinding>,
    pub wasi_ctx: WasiCtx,
}

impl WasmInstance {
    /// Instantiate a WASM module from binary data.
    pub fn new(wasm_bytes: &[u8]) -> Result<Self, String> {
        let module = parse_wasm(wasm_bytes)?;
        Self::from_module(module)
    }

    /// Instantiate from a parsed module.
    pub fn from_module(module: WasmModule) -> Result<Self, String> {
        let mut inst = Self {
            module,
            memories: Vec::new(),
            tables: Vec::new(),
            globals: Vec::new(),
            import_bindings: Vec::new(),
            wasi_ctx: WasiCtx::new(),
        };

        // Initialize memories
        for mem_type in &inst.module.memories.clone() {
            inst.memories.push(LinearMemory::new(mem_type.limits.min, mem_type.limits.max)?);
        }

        // Also check imports for memory
        for import in &inst.module.imports.clone() {
            if let ImportDesc::Memory(mem_type) = &import.desc {
                inst.memories.push(LinearMemory::new(mem_type.limits.min, mem_type.limits.max)?);
            }
        }

        // Initialize tables
        for table_type in &inst.module.tables.clone() {
            inst.tables.push(Table::new(&table_type.limits));
        }

        // Initialize globals
        for global in &inst.module.globals.clone() {
            let val = eval_const_expr(&global.init, &inst.globals)?;
            inst.globals.push(val);
        }

        // Initialize data segments
        let data_segs = inst.module.data.clone();
        for seg in &data_segs {
            let offset = eval_const_expr(&seg.offset, &inst.globals)?.as_i32()? as u32;
            if let Some(mem) = inst.memories.get_mut(seg.memory_idx as usize) {
                mem.write_bytes(offset, &seg.data)?;
            }
        }

        // Initialize element segments
        let elem_segs = inst.module.elements.clone();
        for seg in &elem_segs {
            let offset = eval_const_expr(&seg.offset, &inst.globals)?.as_i32()? as u32;
            if let Some(table) = inst.tables.get_mut(seg.table_idx as usize) {
                for (i, &func_idx) in seg.init.iter().enumerate() {
                    table.set(offset + i as u32, Some(func_idx))?;
                }
            }
        }

        // Register default WASI imports
        inst.register_wasi_imports();

        Ok(inst)
    }

    fn register_wasi_imports(&mut self) {
        use crate::wasi;

        let wasi_imports: &[(&str, &str, HostFn)] = &[
            ("wasi_snapshot_preview1", "fd_write", wasi::fd_write),
            ("wasi_snapshot_preview1", "fd_read", wasi::fd_read),
            ("wasi_snapshot_preview1", "fd_close", wasi::fd_close),
            ("wasi_snapshot_preview1", "fd_seek", wasi::fd_seek),
            ("wasi_snapshot_preview1", "fd_prestat_get", wasi::fd_prestat_get),
            ("wasi_snapshot_preview1", "fd_prestat_dir_name", wasi::fd_prestat_dir_name),
            ("wasi_snapshot_preview1", "args_get", wasi::args_get),
            ("wasi_snapshot_preview1", "args_sizes_get", wasi::args_sizes_get),
            ("wasi_snapshot_preview1", "environ_get", wasi::environ_get),
            ("wasi_snapshot_preview1", "environ_sizes_get", wasi::environ_sizes_get),
            ("wasi_snapshot_preview1", "clock_time_get", wasi::clock_time_get),
            ("wasi_snapshot_preview1", "proc_exit", wasi::proc_exit),
            ("wasi_snapshot_preview1", "path_open", wasi::path_open),
        ];

        for &(module, name, func) in wasi_imports {
            self.import_bindings.push(ImportBinding {
                module: String::from(module),
                name: String::from(name),
                func,
            });
        }
    }

    /// Get the default memory (index 0).
    pub fn memory(&self) -> Result<&LinearMemory, String> {
        self.memories.first()
            .ok_or_else(|| String::from("no memory defined"))
    }

    /// Get the default memory mutably.
    pub fn memory_mut(&mut self) -> Result<&mut LinearMemory, String> {
        self.memories.first_mut()
            .ok_or_else(|| String::from("no memory defined"))
    }

    /// Call an imported function.
    pub fn call_import(
        &mut self,
        import_idx: u32,
        args: &[Value],
        interp: &mut Interpreter,
    ) -> Result<Vec<Value>, String> {
        // Find the import
        let import = self.module.imports.iter()
            .filter(|i| matches!(i.desc, ImportDesc::Func(_)))
            .nth(import_idx as usize)
            .ok_or_else(|| format!("import function {} not found", import_idx))?;

        let module_name = import.module.clone();
        let field_name = import.name.clone();

        // Find matching binding
        let binding_idx = self.import_bindings.iter().position(|b| {
            b.module == module_name && b.name == field_name
        });

        match binding_idx {
            Some(idx) => {
                let func = self.import_bindings[idx].func;
                let mem = self.memories.first_mut().map(|m| m as *mut LinearMemory);

                // We need to pass memory separately for WASI calls
                let mut mem_opt = unsafe {
                    mem.map(|p| core::ptr::read(p))
                };
                let result = func(args, &mut self.wasi_ctx, &mut mem_opt);

                // Write stdout captures back to interpreter
                if !self.wasi_ctx.stdout_buf.is_empty() {
                    interp.stdout_buffer.extend_from_slice(&self.wasi_ctx.stdout_buf);
                    self.wasi_ctx.stdout_buf.clear();
                }

                // Copy memory back if it was modified
                if let (Some(new_mem), Some(old_mem)) = (mem_opt, self.memories.first_mut()) {
                    *old_mem = new_mem;
                }

                result
            }
            None => {
                log::warn!("wasm: unresolved import {}.{}, returning zero", module_name, field_name);
                // Return default values based on the function type
                let type_idx = match &self.module.imports.iter()
                    .filter(|i| matches!(i.desc, ImportDesc::Func(_)))
                    .nth(import_idx as usize)
                    .unwrap()
                    .desc
                {
                    ImportDesc::Func(idx) => *idx,
                    _ => unreachable!(),
                };
                let func_type = self.module.types.get(type_idx as usize)
                    .ok_or_else(|| format!("type {} not found", type_idx))?;
                let results: Vec<Value> = func_type.results.iter()
                    .map(|t| Value::default_for(*t))
                    .collect();
                Ok(results)
            }
        }
    }

    /// Run the start function if defined.
    pub fn run_start(&mut self) -> Result<(), String> {
        if let Some(start_idx) = self.module.start {
            let mut interp = Interpreter::new();
            interp.invoke(self, start_idx, &[])?;
        }
        Ok(())
    }

    /// Call an exported function by name.
    pub fn call_export(
        &mut self,
        name: &str,
        args: &[Value],
    ) -> Result<Vec<Value>, String> {
        let export = self.module.find_export(name)
            .ok_or_else(|| format!("export '{}' not found", name))?;

        let func_idx = match export.desc {
            ExportDesc::Func(idx) => idx,
            _ => return Err(format!("export '{}' is not a function", name)),
        };

        let mut interp = Interpreter::new();
        let result = interp.invoke(self, func_idx, args)?;
        Ok(result)
    }

    /// Call an exported function and capture stdout.
    pub fn call_export_with_stdout(
        &mut self,
        name: &str,
        args: &[Value],
    ) -> Result<(Vec<Value>, Vec<u8>), String> {
        let export = self.module.find_export(name)
            .ok_or_else(|| format!("export '{}' not found", name))?;

        let func_idx = match export.desc {
            ExportDesc::Func(idx) => idx,
            _ => return Err(format!("export '{}' is not a function", name)),
        };

        let mut interp = Interpreter::new();
        let result = interp.invoke(self, func_idx, args)?;
        Ok((result, interp.stdout_buffer))
    }
}
