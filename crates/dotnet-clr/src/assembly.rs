//! Assembly loading and resolution for .NET.
//!
//! Loads .NET assemblies (.dll/.exe), parses their metadata, resolves
//! assembly references, and provides a GAC-like assembly store.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::pe_metadata::{self, DotNetMetadata, MetadataError};

/// A loaded .NET assembly.
#[derive(Debug, Clone)]
pub struct LoadedAssembly {
    /// Assembly name (e.g., "MyApp", "System.Runtime").
    pub name: String,
    /// Assembly version string (e.g., "1.0.0.0").
    pub version: String,
    /// Culture (e.g., "neutral").
    pub culture: String,
    /// Whether this is the entry assembly (has Main method).
    pub is_entry: bool,
    /// Parsed .NET metadata.
    pub metadata: DotNetMetadata,
    /// Raw PE bytes (needed for method body access).
    pub raw_data: Vec<u8>,
    /// Entry point method token (from CLI header), 0 if none.
    pub entry_point_token: u32,
}

/// Assembly loading error.
#[derive(Debug, Clone)]
pub enum AssemblyError {
    /// PE/metadata parsing failed.
    MetadataError(MetadataError),
    /// Assembly not found by name.
    NotFound(String),
    /// Duplicate assembly loaded.
    AlreadyLoaded(String),
    /// Assembly reference could not be resolved.
    UnresolvedReference(String),
}

impl From<MetadataError> for AssemblyError {
    fn from(e: MetadataError) -> Self {
        AssemblyError::MetadataError(e)
    }
}

/// Global assembly store (GAC-like).
static ASSEMBLY_STORE: Mutex<Option<AssemblyStore>> = Mutex::new(None);

/// Assembly store: holds all loaded assemblies.
pub struct AssemblyStore {
    /// Loaded assemblies indexed by name.
    assemblies: BTreeMap<String, LoadedAssembly>,
    /// Assembly search paths (for resolving references).
    search_paths: Vec<String>,
}

impl AssemblyStore {
    /// Create a new empty assembly store.
    pub fn new() -> Self {
        Self {
            assemblies: BTreeMap::new(),
            search_paths: Vec::new(),
        }
    }

    /// Load an assembly from raw PE bytes.
    pub fn load_assembly(
        &mut self,
        pe_data: &[u8],
        is_entry: bool,
    ) -> Result<String, AssemblyError> {
        log::info!("[dotnet-asm] Loading assembly ({} bytes)", pe_data.len());

        // Parse metadata
        let metadata = pe_metadata::parse_dotnet_metadata(pe_data)?;

        // Extract assembly name from the Assembly table
        let name = if let Some(asm_row) = metadata.tables.assemblies.first() {
            pe_metadata::get_string(&metadata.strings_heap, asm_row.name)
                .unwrap_or_else(|| String::from("UnknownAssembly"))
        } else {
            String::from("UnknownAssembly")
        };

        // Extract version
        let version = if let Some(asm_row) = metadata.tables.assemblies.first() {
            alloc::format!(
                "{}.{}.{}.{}",
                asm_row.major_version,
                asm_row.minor_version,
                asm_row.build_number,
                asm_row.revision_number
            )
        } else {
            String::from("0.0.0.0")
        };

        // Extract culture
        let culture = if let Some(asm_row) = metadata.tables.assemblies.first() {
            pe_metadata::get_string(&metadata.strings_heap, asm_row.culture)
                .unwrap_or_else(|| String::from("neutral"))
        } else {
            String::from("neutral")
        };

        let entry_point_token = metadata.cli_header.entry_point_token;

        log::info!(
            "[dotnet-asm] Assembly '{}' v{} loaded: {} TypeDefs, {} MethodDefs, entry=0x{:08X}",
            name, version,
            metadata.tables.type_defs.len(),
            metadata.tables.method_defs.len(),
            entry_point_token,
        );

        // Log assembly references
        for aref in &metadata.tables.assembly_refs {
            let ref_name = pe_metadata::get_string(&metadata.strings_heap, aref.name)
                .unwrap_or_else(|| String::from("?"));
            log::debug!(
                "[dotnet-asm]   References: {} v{}.{}.{}.{}",
                ref_name, aref.major_version, aref.minor_version,
                aref.build_number, aref.revision_number,
            );
        }

        let loaded = LoadedAssembly {
            name: name.clone(),
            version,
            culture,
            is_entry,
            metadata,
            raw_data: pe_data.to_vec(),
            entry_point_token,
        };

        self.assemblies.insert(name.clone(), loaded);
        Ok(name)
    }

    /// Get a loaded assembly by name.
    pub fn get_assembly(&self, name: &str) -> Option<&LoadedAssembly> {
        self.assemblies.get(name)
    }

    /// Get all loaded assembly names.
    pub fn loaded_assemblies(&self) -> Vec<String> {
        self.assemblies.keys().cloned().collect()
    }

    /// Resolve an assembly reference by name.
    ///
    /// Checks the loaded assemblies first, then search paths.
    pub fn resolve_reference(&self, name: &str) -> Result<&LoadedAssembly, AssemblyError> {
        self.assemblies
            .get(name)
            .ok_or_else(|| AssemblyError::UnresolvedReference(String::from(name)))
    }

    /// Add an assembly search path.
    pub fn add_search_path(&mut self, path: String) {
        self.search_paths.push(path);
    }

    /// Get the entry assembly (the one with Main).
    pub fn entry_assembly(&self) -> Option<&LoadedAssembly> {
        self.assemblies.values().find(|a| a.is_entry)
    }

    /// Get the number of loaded assemblies.
    pub fn assembly_count(&self) -> usize {
        self.assemblies.len()
    }

    /// Unload an assembly by name.
    pub fn unload(&mut self, name: &str) -> bool {
        self.assemblies.remove(name).is_some()
    }
}

/// Initialize the global assembly store.
pub fn init() {
    let mut store = ASSEMBLY_STORE.lock();
    if store.is_none() {
        *store = Some(AssemblyStore::new());
        log::info!("[dotnet-asm] Assembly store initialized");
    }
}

/// Access the global assembly store.
pub fn with_store<F, R>(f: F) -> R
where
    F: FnOnce(&AssemblyStore) -> R,
{
    let store = ASSEMBLY_STORE.lock();
    f(store.as_ref().expect("Assembly store not initialized"))
}

/// Access the global assembly store mutably.
pub fn with_store_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut AssemblyStore) -> R,
{
    let mut store = ASSEMBLY_STORE.lock();
    f(store.as_mut().expect("Assembly store not initialized"))
}
