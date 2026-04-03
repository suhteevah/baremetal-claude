//! JIT compilation: CIL to x86_64 machine code via Cranelift.
//!
//! Method-at-a-time compilation with a cache of compiled methods.
//! Reuses the Cranelift infrastructure from crates/rustc-lite.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::il::MethodBody;

/// Cache of JIT-compiled methods.
///
/// Maps method token -> compiled machine code.
static JIT_CACHE: Mutex<Option<JitCache>> = Mutex::new(None);

/// Compiled method information.
#[derive(Debug, Clone)]
pub struct CompiledMethod {
    /// The method's metadata token.
    pub token: u32,
    /// Fully qualified method name (for debugging).
    pub name: String,
    /// Compiled x86_64 machine code bytes.
    pub machine_code: Vec<u8>,
    /// Entry point offset within machine_code.
    pub entry_offset: usize,
    /// Size of the stack frame in bytes.
    pub frame_size: u32,
    /// Whether this method is currently valid (not invalidated).
    pub valid: bool,
}

impl CompiledMethod {
    /// Get the entry point address (when loaded at a base address).
    pub fn entry_point(&self, base: usize) -> usize {
        base + self.entry_offset
    }
}

/// JIT compilation cache.
pub struct JitCache {
    /// Compiled methods indexed by metadata token.
    methods: BTreeMap<u32, CompiledMethod>,
    /// Total bytes of compiled code.
    total_code_bytes: usize,
    /// Number of compilations performed.
    compilations: u64,
    /// Number of cache hits.
    cache_hits: u64,
}

impl JitCache {
    /// Create a new empty JIT cache.
    pub fn new() -> Self {
        Self {
            methods: BTreeMap::new(),
            total_code_bytes: 0,
            compilations: 0,
            cache_hits: 0,
        }
    }

    /// Look up a compiled method by token.
    pub fn get(&mut self, token: u32) -> Option<&CompiledMethod> {
        if self.methods.contains_key(&token) {
            self.cache_hits += 1;
        }
        self.methods.get(&token)
    }

    /// Insert a compiled method into the cache.
    pub fn insert(&mut self, method: CompiledMethod) {
        self.total_code_bytes += method.machine_code.len();
        self.compilations += 1;
        log::debug!(
            "[dotnet-jit] Cached method token=0x{:08X} '{}' ({} bytes)",
            method.token, method.name, method.machine_code.len()
        );
        self.methods.insert(method.token, method);
    }

    /// Invalidate a compiled method (e.g., after hot-reload).
    pub fn invalidate(&mut self, token: u32) {
        if let Some(method) = self.methods.get_mut(&token) {
            method.valid = false;
        }
    }

    /// Clear the entire cache.
    pub fn clear(&mut self) {
        self.methods.clear();
        self.total_code_bytes = 0;
    }

    /// Get cache statistics.
    pub fn stats(&self) -> JitStats {
        JitStats {
            cached_methods: self.methods.len(),
            total_code_bytes: self.total_code_bytes,
            compilations: self.compilations,
            cache_hits: self.cache_hits,
        }
    }
}

/// JIT compilation statistics.
#[derive(Debug, Clone)]
pub struct JitStats {
    pub cached_methods: usize,
    pub total_code_bytes: usize,
    pub compilations: u64,
    pub cache_hits: u64,
}

/// JIT compilation error.
#[derive(Debug, Clone)]
pub enum JitError {
    /// The method body is invalid or unsupported.
    InvalidMethodBody(String),
    /// Cranelift compilation failed.
    CompilationFailed(String),
    /// Unsupported CIL opcode.
    UnsupportedOpcode(String),
    /// Memory allocation for code failed.
    AllocationFailed,
}

/// JIT compile a CIL method body to x86_64 machine code.
///
/// This is a stub implementation that will eventually use Cranelift from
/// crates/rustc-lite to generate native code.
///
/// # Arguments
/// * `token` — Metadata token of the method.
/// * `name` — Fully qualified method name.
/// * `body` — Parsed CIL method body.
///
/// # Returns
/// A `CompiledMethod` with the generated machine code, or an error.
pub fn jit_compile(
    token: u32,
    name: String,
    _body: &MethodBody,
) -> Result<CompiledMethod, JitError> {
    log::info!(
        "[dotnet-jit] JIT compiling method token=0x{:08X} '{}'",
        token, name
    );

    // TODO: Use Cranelift to translate CIL -> IR -> x86_64 machine code.
    //
    // The compilation pipeline would be:
    // 1. Create a Cranelift Function with the method's signature.
    // 2. Walk the CIL bytecode and emit Cranelift IR instructions:
    //    - CIL evaluation stack maps to SSA values
    //    - CIL locals map to Cranelift stack slots
    //    - CIL branch targets map to Cranelift blocks
    //    - Method calls emit Cranelift call instructions
    // 3. Run Cranelift's optimization + register allocation passes.
    // 4. Emit x86_64 machine code.
    // 5. Copy to executable memory (W^X: allocate RW, write, then flip to RX).
    //
    // For now, return a stub that just does `ret`.

    // x86_64 stub: xor eax, eax; ret (returns 0)
    let machine_code = alloc::vec![
        0x31, 0xC0, // xor eax, eax
        0xC3,       // ret
    ];

    Ok(CompiledMethod {
        token,
        name,
        machine_code,
        entry_offset: 0,
        frame_size: 0,
        valid: true,
    })
}

/// Initialize the JIT compiler subsystem.
pub fn init() {
    let mut cache = JIT_CACHE.lock();
    if cache.is_none() {
        *cache = Some(JitCache::new());
        log::info!("[dotnet-jit] JIT compiler initialized");
    }
}

/// JIT compile and cache a method, or return the cached version.
pub fn compile_or_cached(
    token: u32,
    name: String,
    body: &MethodBody,
) -> Result<CompiledMethod, JitError> {
    let mut cache = JIT_CACHE.lock();
    let cache = cache.as_mut().ok_or(JitError::CompilationFailed(
        String::from("JIT cache not initialized"),
    ))?;

    // Check cache first
    if let Some(cached) = cache.get(token) {
        if cached.valid {
            return Ok(cached.clone());
        }
    }

    // Compile and cache
    let compiled = jit_compile(token, name, body)?;
    cache.insert(compiled.clone());
    Ok(compiled)
}

/// Get JIT statistics.
pub fn stats() -> JitStats {
    JIT_CACHE
        .lock()
        .as_ref()
        .map(|c| c.stats())
        .unwrap_or(JitStats {
            cached_methods: 0,
            total_code_bytes: 0,
            compilations: 0,
            cache_hits: 0,
        })
}
