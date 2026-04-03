//! .winmd metadata parsing.
//!
//! Windows Metadata (.winmd) files use the same ECMA-335 metadata format as
//! .NET assemblies. This module provides a thin wrapper over standard metadata
//! parsing to extract WinRT-specific type information: runtime classes,
//! interfaces, delegates, enums, and structs.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

/// A WinRT type kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinRtTypeKind {
    /// Runtime class (activatable).
    RuntimeClass,
    /// Interface (IFoo).
    Interface,
    /// Delegate (callback type).
    Delegate,
    /// Enum.
    Enum,
    /// Struct (value type).
    Struct,
    /// Attribute.
    Attribute,
}

/// A WinRT type definition extracted from .winmd metadata.
#[derive(Debug, Clone)]
pub struct WinRtTypeDef {
    /// Fully qualified name (e.g., "Windows.Foundation.Uri").
    pub full_name: String,
    /// Namespace (e.g., "Windows.Foundation").
    pub namespace: String,
    /// Short name (e.g., "Uri").
    pub name: String,
    /// Type kind.
    pub kind: WinRtTypeKind,
    /// Base type name (if any).
    pub base_type: Option<String>,
    /// Interfaces implemented.
    pub interfaces: Vec<String>,
    /// Methods.
    pub methods: Vec<WinRtMethodDef>,
    /// Properties.
    pub properties: Vec<WinRtPropertyDef>,
    /// Events.
    pub events: Vec<WinRtEventDef>,
    /// Whether this class is activatable (has a default constructor).
    pub is_activatable: bool,
    /// Whether this class is sealed.
    pub is_sealed: bool,
    /// Whether this class is static (only static members).
    pub is_static: bool,
    /// Generic parameters.
    pub generic_params: Vec<String>,
}

/// A WinRT method definition.
#[derive(Debug, Clone)]
pub struct WinRtMethodDef {
    /// Method name.
    pub name: String,
    /// Return type name.
    pub return_type: String,
    /// Parameter names and types.
    pub parameters: Vec<(String, String)>,
    /// Whether this is a static method.
    pub is_static: bool,
    /// Whether this method is overloaded.
    pub is_overload: bool,
    /// Default overload name (if overloaded).
    pub default_overload: Option<String>,
}

/// A WinRT property definition.
#[derive(Debug, Clone)]
pub struct WinRtPropertyDef {
    /// Property name.
    pub name: String,
    /// Property type name.
    pub property_type: String,
    /// Whether the property has a getter.
    pub has_getter: bool,
    /// Whether the property has a setter.
    pub has_setter: bool,
    /// Whether this is a static property.
    pub is_static: bool,
}

/// A WinRT event definition.
#[derive(Debug, Clone)]
pub struct WinRtEventDef {
    /// Event name.
    pub name: String,
    /// Delegate type name.
    pub delegate_type: String,
}

/// Parsed WinRT metadata from a .winmd file.
#[derive(Debug, Clone)]
pub struct WinRtMetadata {
    /// All type definitions.
    pub types: Vec<WinRtTypeDef>,
    /// Types indexed by full name.
    pub type_index: BTreeMap<String, usize>,
    /// Namespaces found.
    pub namespaces: Vec<String>,
}

impl WinRtMetadata {
    /// Create empty metadata.
    pub fn new() -> Self {
        Self {
            types: Vec::new(),
            type_index: BTreeMap::new(),
            namespaces: Vec::new(),
        }
    }

    /// Add a type definition.
    pub fn add_type(&mut self, typedef: WinRtTypeDef) {
        let idx = self.types.len();
        self.type_index.insert(typedef.full_name.clone(), idx);
        if !self.namespaces.contains(&typedef.namespace) {
            self.namespaces.push(typedef.namespace.clone());
        }
        self.types.push(typedef);
    }

    /// Look up a type by fully qualified name.
    pub fn get_type(&self, full_name: &str) -> Option<&WinRtTypeDef> {
        self.type_index.get(full_name).map(|&idx| &self.types[idx])
    }

    /// Get all types in a namespace.
    pub fn types_in_namespace(&self, namespace: &str) -> Vec<&WinRtTypeDef> {
        self.types
            .iter()
            .filter(|t| t.namespace == namespace)
            .collect()
    }

    /// Get all runtime classes (activatable types).
    pub fn runtime_classes(&self) -> Vec<&WinRtTypeDef> {
        self.types
            .iter()
            .filter(|t| t.kind == WinRtTypeKind::RuntimeClass)
            .collect()
    }

    /// Get all interfaces.
    pub fn interfaces(&self) -> Vec<&WinRtTypeDef> {
        self.types
            .iter()
            .filter(|t| t.kind == WinRtTypeKind::Interface)
            .collect()
    }
}

/// Parse WinRT metadata from raw .winmd file bytes.
///
/// .winmd files use ECMA-335 format (same as .NET assemblies), so we reuse
/// the same PE metadata parser but interpret the results as WinRT types.
pub fn parse_winmd(_data: &[u8]) -> Result<WinRtMetadata, WinMdError> {
    log::info!("[winrt-metadata] Parsing .winmd metadata ({} bytes)", _data.len());

    // In a full implementation, we would:
    // 1. Parse using pe_metadata::parse_dotnet_metadata()
    // 2. Iterate TypeDef table entries
    // 3. Classify each type as RuntimeClass/Interface/Delegate/Enum/Struct
    //    based on base type and attributes
    // 4. Extract methods, properties, events from MethodDef/Property/Event tables
    // 5. Resolve interface implementations from InterfaceImpl table
    // 6. Check custom attributes for [Activatable], [Static], etc.

    let metadata = WinRtMetadata::new();
    Ok(metadata)
}

/// Create built-in Windows.Foundation metadata without parsing a .winmd file.
///
/// Provides type definitions for commonly-used WinRT types.
pub fn builtin_metadata() -> WinRtMetadata {
    let mut meta = WinRtMetadata::new();

    // IStringable
    meta.add_type(WinRtTypeDef {
        full_name: String::from("Windows.Foundation.IStringable"),
        namespace: String::from("Windows.Foundation"),
        name: String::from("IStringable"),
        kind: WinRtTypeKind::Interface,
        base_type: None,
        interfaces: Vec::new(),
        methods: alloc::vec![WinRtMethodDef {
            name: String::from("ToString"),
            return_type: String::from("String"),
            parameters: Vec::new(),
            is_static: false,
            is_overload: false,
            default_overload: None,
        }],
        properties: Vec::new(),
        events: Vec::new(),
        is_activatable: false,
        is_sealed: false,
        is_static: false,
        generic_params: Vec::new(),
    });

    // IClosable (IDisposable equivalent)
    meta.add_type(WinRtTypeDef {
        full_name: String::from("Windows.Foundation.IClosable"),
        namespace: String::from("Windows.Foundation"),
        name: String::from("IClosable"),
        kind: WinRtTypeKind::Interface,
        base_type: None,
        interfaces: Vec::new(),
        methods: alloc::vec![WinRtMethodDef {
            name: String::from("Close"),
            return_type: String::from("Void"),
            parameters: Vec::new(),
            is_static: false,
            is_overload: false,
            default_overload: None,
        }],
        properties: Vec::new(),
        events: Vec::new(),
        is_activatable: false,
        is_sealed: false,
        is_static: false,
        generic_params: Vec::new(),
    });

    // Uri
    meta.add_type(WinRtTypeDef {
        full_name: String::from("Windows.Foundation.Uri"),
        namespace: String::from("Windows.Foundation"),
        name: String::from("Uri"),
        kind: WinRtTypeKind::RuntimeClass,
        base_type: Some(String::from("System.Object")),
        interfaces: alloc::vec![
            String::from("Windows.Foundation.IStringable"),
        ],
        methods: alloc::vec![
            WinRtMethodDef {
                name: String::from(".ctor"),
                return_type: String::from("Void"),
                parameters: alloc::vec![(String::from("uri"), String::from("String"))],
                is_static: false,
                is_overload: false,
                default_overload: None,
            },
            WinRtMethodDef {
                name: String::from("ToString"),
                return_type: String::from("String"),
                parameters: Vec::new(),
                is_static: false,
                is_overload: false,
                default_overload: None,
            },
        ],
        properties: alloc::vec![
            WinRtPropertyDef {
                name: String::from("AbsoluteUri"),
                property_type: String::from("String"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
            WinRtPropertyDef {
                name: String::from("Host"),
                property_type: String::from("String"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
            WinRtPropertyDef {
                name: String::from("Path"),
                property_type: String::from("String"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
            WinRtPropertyDef {
                name: String::from("Port"),
                property_type: String::from("Int32"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
            WinRtPropertyDef {
                name: String::from("SchemeName"),
                property_type: String::from("String"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
        ],
        events: Vec::new(),
        is_activatable: true,
        is_sealed: true,
        is_static: false,
        generic_params: Vec::new(),
    });

    // DateTime
    meta.add_type(WinRtTypeDef {
        full_name: String::from("Windows.Foundation.DateTime"),
        namespace: String::from("Windows.Foundation"),
        name: String::from("DateTime"),
        kind: WinRtTypeKind::Struct,
        base_type: None,
        interfaces: Vec::new(),
        methods: Vec::new(),
        properties: alloc::vec![WinRtPropertyDef {
            name: String::from("UniversalTime"),
            property_type: String::from("Int64"),
            has_getter: true,
            has_setter: true,
            is_static: false,
        }],
        events: Vec::new(),
        is_activatable: false,
        is_sealed: true,
        is_static: false,
        generic_params: Vec::new(),
    });

    // TimeSpan
    meta.add_type(WinRtTypeDef {
        full_name: String::from("Windows.Foundation.TimeSpan"),
        namespace: String::from("Windows.Foundation"),
        name: String::from("TimeSpan"),
        kind: WinRtTypeKind::Struct,
        base_type: None,
        interfaces: Vec::new(),
        methods: Vec::new(),
        properties: alloc::vec![WinRtPropertyDef {
            name: String::from("Duration"),
            property_type: String::from("Int64"),
            has_getter: true,
            has_setter: true,
            is_static: false,
        }],
        events: Vec::new(),
        is_activatable: false,
        is_sealed: true,
        is_static: false,
        generic_params: Vec::new(),
    });

    // Windows.Storage.StorageFile
    meta.add_type(WinRtTypeDef {
        full_name: String::from("Windows.Storage.StorageFile"),
        namespace: String::from("Windows.Storage"),
        name: String::from("StorageFile"),
        kind: WinRtTypeKind::RuntimeClass,
        base_type: Some(String::from("System.Object")),
        interfaces: alloc::vec![
            String::from("Windows.Foundation.IStringable"),
            String::from("Windows.Foundation.IClosable"),
        ],
        methods: alloc::vec![WinRtMethodDef {
            name: String::from("GetFileFromPathAsync"),
            return_type: String::from("IAsyncOperation<StorageFile>"),
            parameters: alloc::vec![(String::from("path"), String::from("String"))],
            is_static: true,
            is_overload: false,
            default_overload: None,
        }],
        properties: alloc::vec![
            WinRtPropertyDef {
                name: String::from("Name"),
                property_type: String::from("String"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
            WinRtPropertyDef {
                name: String::from("Path"),
                property_type: String::from("String"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
            WinRtPropertyDef {
                name: String::from("FileType"),
                property_type: String::from("String"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
        ],
        events: Vec::new(),
        is_activatable: false,
        is_sealed: true,
        is_static: false,
        generic_params: Vec::new(),
    });

    // Windows.Storage.StorageFolder
    meta.add_type(WinRtTypeDef {
        full_name: String::from("Windows.Storage.StorageFolder"),
        namespace: String::from("Windows.Storage"),
        name: String::from("StorageFolder"),
        kind: WinRtTypeKind::RuntimeClass,
        base_type: Some(String::from("System.Object")),
        interfaces: alloc::vec![
            String::from("Windows.Foundation.IStringable"),
        ],
        methods: alloc::vec![WinRtMethodDef {
            name: String::from("GetFolderFromPathAsync"),
            return_type: String::from("IAsyncOperation<StorageFolder>"),
            parameters: alloc::vec![(String::from("path"), String::from("String"))],
            is_static: true,
            is_overload: false,
            default_overload: None,
        }],
        properties: alloc::vec![
            WinRtPropertyDef {
                name: String::from("Name"),
                property_type: String::from("String"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
            WinRtPropertyDef {
                name: String::from("Path"),
                property_type: String::from("String"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
        ],
        events: Vec::new(),
        is_activatable: false,
        is_sealed: true,
        is_static: false,
        generic_params: Vec::new(),
    });

    // Windows.UI.Colors
    meta.add_type(WinRtTypeDef {
        full_name: String::from("Windows.UI.Colors"),
        namespace: String::from("Windows.UI"),
        name: String::from("Colors"),
        kind: WinRtTypeKind::RuntimeClass,
        base_type: Some(String::from("System.Object")),
        interfaces: Vec::new(),
        methods: Vec::new(),
        properties: Vec::new(), // Static color properties would go here
        events: Vec::new(),
        is_activatable: false,
        is_sealed: true,
        is_static: true,
        generic_params: Vec::new(),
    });

    // Windows.Networking.HostName
    meta.add_type(WinRtTypeDef {
        full_name: String::from("Windows.Networking.HostName"),
        namespace: String::from("Windows.Networking"),
        name: String::from("HostName"),
        kind: WinRtTypeKind::RuntimeClass,
        base_type: Some(String::from("System.Object")),
        interfaces: alloc::vec![
            String::from("Windows.Foundation.IStringable"),
        ],
        methods: alloc::vec![WinRtMethodDef {
            name: String::from(".ctor"),
            return_type: String::from("Void"),
            parameters: alloc::vec![(String::from("hostName"), String::from("String"))],
            is_static: false,
            is_overload: false,
            default_overload: None,
        }],
        properties: alloc::vec![
            WinRtPropertyDef {
                name: String::from("DisplayName"),
                property_type: String::from("String"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
            WinRtPropertyDef {
                name: String::from("RawName"),
                property_type: String::from("String"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
            WinRtPropertyDef {
                name: String::from("Type"),
                property_type: String::from("HostNameType"),
                has_getter: true,
                has_setter: false,
                is_static: false,
            },
        ],
        events: Vec::new(),
        is_activatable: true,
        is_sealed: true,
        is_static: false,
        generic_params: Vec::new(),
    });

    log::info!(
        "[winrt-metadata] Built-in metadata: {} types in {} namespaces",
        meta.types.len(),
        meta.namespaces.len()
    );

    meta
}

/// .winmd parsing error.
#[derive(Debug, Clone)]
pub enum WinMdError {
    /// Invalid file format.
    InvalidFormat(String),
    /// Missing required metadata.
    MissingMetadata(String),
}
