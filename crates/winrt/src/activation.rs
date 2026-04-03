//! WinRT activation factory: RoActivateInstance, RoGetActivationFactory.
//!
//! Provides the COM-like activation infrastructure for creating WinRT objects
//! by their runtime class name. Also handles HSTRING creation/destruction.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// HRESULT values.
pub const S_OK: i32 = 0;
pub const E_FAIL: i32 = -2147467259; // 0x80004005
pub const E_INVALIDARG: i32 = -2147024809; // 0x80070057
pub const E_NOINTERFACE: i32 = -2147467262; // 0x80004002
pub const E_CLASSNOTREGISTERED: i32 = -2147221164; // 0x80040154
pub const RO_E_METADATA_NAME_NOT_FOUND: i32 = -2147483631; // 0x80000011

/// An HSTRING: WinRT immutable string handle.
///
/// In real WinRT, HSTRING is a handle to a ref-counted UTF-16 string header.
/// Our implementation wraps a Rust String.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HString {
    /// Internal UTF-16 data.
    data: Vec<u16>,
    /// Cached UTF-8 representation.
    utf8: String,
}

impl HString {
    /// Create a new HSTRING from a Rust string.
    pub fn create(s: &str) -> Self {
        let data: Vec<u16> = s.encode_utf16().collect();
        Self {
            data,
            utf8: String::from(s),
        }
    }

    /// Create an empty HSTRING.
    pub fn empty() -> Self {
        Self {
            data: Vec::new(),
            utf8: String::new(),
        }
    }

    /// Get the string as UTF-8.
    pub fn as_str(&self) -> &str {
        &self.utf8
    }

    /// Get the raw UTF-16 buffer.
    pub fn as_wide(&self) -> &[u16] {
        &self.data
    }

    /// Get the length in UTF-16 code units.
    pub fn len(&self) -> u32 {
        self.data.len() as u32
    }

    /// Check if the HSTRING is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Duplicate this HSTRING.
    pub fn duplicate(&self) -> Self {
        self.clone()
    }
}

impl core::fmt::Display for HString {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.utf8)
    }
}

/// A WinRT interface identifier (IID).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    /// Create a GUID from components.
    pub const fn new(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Self {
        Self {
            data1: d1,
            data2: d2,
            data3: d3,
            data4: d4,
        }
    }

    /// The null GUID.
    pub const ZERO: Guid = Guid::new(0, 0, 0, [0; 8]);
}

impl core::fmt::Display for Guid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            self.data1, self.data2, self.data3,
            self.data4[0], self.data4[1],
            self.data4[2], self.data4[3], self.data4[4],
            self.data4[5], self.data4[6], self.data4[7],
        )
    }
}

// Well-known IIDs
/// IUnknown
pub const IID_IUNKNOWN: Guid = Guid::new(
    0x00000000, 0x0000, 0x0000, [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
/// IInspectable
pub const IID_IINSPECTABLE: Guid = Guid::new(
    0xAF86E2E0, 0xB12D, 0x4C6A, [0x9C, 0x5A, 0xD7, 0xAA, 0x65, 0x10, 0x1E, 0x90],
);
/// IActivationFactory
pub const IID_IACTIVATION_FACTORY: Guid = Guid::new(
    0x00000035, 0x0000, 0x0000, [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);

/// A WinRT object instance.
#[derive(Debug, Clone)]
pub struct WinRtObject {
    /// Runtime class name (e.g., "Windows.Foundation.Uri").
    pub class_name: String,
    /// Object handle (unique ID).
    pub handle: u64,
    /// Properties (key-value store for this object's state).
    pub properties: BTreeMap<String, WinRtValue>,
}

/// A WinRT value (for property storage).
#[derive(Debug, Clone)]
pub enum WinRtValue {
    Null,
    Boolean(bool),
    Int32(i32),
    Int64(i64),
    UInt32(u32),
    UInt64(u64),
    Single(f32),
    Double(f64),
    String(String),
    DateTime(i64),     // 100-nanosecond intervals since Jan 1, 1601
    TimeSpan(i64),     // 100-nanosecond intervals
    Guid(Guid),
    Object(u64),       // Handle to another WinRT object
}

/// Activation factory: creates instances of WinRT runtime classes.
///
/// Each runtime class has an associated factory that knows how to construct it.
pub trait IActivationFactory {
    /// Get the runtime class name this factory creates.
    fn class_name(&self) -> &str;

    /// Create a default instance of the class.
    fn activate_instance(&self) -> Result<WinRtObject, i32>;

    /// Create an instance with constructor arguments.
    fn create_instance(&self, args: &[WinRtValue]) -> Result<WinRtObject, i32>;
}

/// A default activation factory implementation.
struct DefaultFactory {
    name: String,
}

impl IActivationFactory for DefaultFactory {
    fn class_name(&self) -> &str {
        &self.name
    }

    fn activate_instance(&self) -> Result<WinRtObject, i32> {
        let handle = alloc_handle();
        Ok(WinRtObject {
            class_name: self.name.clone(),
            handle,
            properties: BTreeMap::new(),
        })
    }

    fn create_instance(&self, _args: &[WinRtValue]) -> Result<WinRtObject, i32> {
        self.activate_instance()
    }
}

/// Global activation factory registry.
static FACTORY_REGISTRY: Mutex<Option<FactoryRegistry>> = Mutex::new(None);

/// Next object handle.
static NEXT_HANDLE: Mutex<u64> = Mutex::new(1);

fn alloc_handle() -> u64 {
    let mut h = NEXT_HANDLE.lock();
    let handle = *h;
    *h += 1;
    handle
}

/// Registry of activation factories.
pub struct FactoryRegistry {
    /// Factories indexed by runtime class name.
    factories: BTreeMap<String, alloc::boxed::Box<dyn IActivationFactory + Send>>,
    /// Live objects indexed by handle.
    objects: BTreeMap<u64, WinRtObject>,
}

unsafe impl Send for FactoryRegistry {}

impl FactoryRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            factories: BTreeMap::new(),
            objects: BTreeMap::new(),
        }
    }

    /// Register an activation factory.
    pub fn register_factory(
        &mut self,
        class_name: String,
        factory: alloc::boxed::Box<dyn IActivationFactory + Send>,
    ) {
        log::trace!("[winrt] Registered factory: {}", class_name);
        self.factories.insert(class_name, factory);
    }

    /// Register a default factory for a class name.
    pub fn register_default(&mut self, class_name: &str) {
        let factory = alloc::boxed::Box::new(DefaultFactory {
            name: String::from(class_name),
        });
        self.factories.insert(String::from(class_name), factory);
    }

    /// RoActivateInstance: create a new instance of a runtime class.
    pub fn activate_instance(&mut self, class_name: &str) -> Result<u64, i32> {
        let factory = self.factories.get(class_name).ok_or(E_CLASSNOTREGISTERED)?;
        let obj = factory.activate_instance()?;
        let handle = obj.handle;
        self.objects.insert(handle, obj);
        log::debug!("[winrt] Activated instance of '{}' -> handle={}", class_name, handle);
        Ok(handle)
    }

    /// RoGetActivationFactory: get the factory for a runtime class.
    pub fn get_factory(&self, class_name: &str) -> Result<&(dyn IActivationFactory + Send), i32> {
        self.factories
            .get(class_name)
            .map(|f| &**f)
            .ok_or(E_CLASSNOTREGISTERED)
    }

    /// Get a live object by handle.
    pub fn get_object(&self, handle: u64) -> Option<&WinRtObject> {
        self.objects.get(&handle)
    }

    /// Get a mutable reference to a live object.
    pub fn get_object_mut(&mut self, handle: u64) -> Option<&mut WinRtObject> {
        self.objects.get_mut(&handle)
    }

    /// Release an object by handle.
    pub fn release_object(&mut self, handle: u64) -> bool {
        self.objects.remove(&handle).is_some()
    }

    /// Get the number of registered factories.
    pub fn factory_count(&self) -> usize {
        self.factories.len()
    }

    /// Get the number of live objects.
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }
}

/// Initialize the activation factory registry.
pub fn init() {
    let mut reg = FACTORY_REGISTRY.lock();
    if reg.is_none() {
        *reg = Some(FactoryRegistry::new());
        log::info!("[winrt] Activation factory registry initialized");
    }
}

/// Register default factories for common WinRT types.
pub fn register_builtin_factories() {
    with_registry_mut(|reg| {
        // Windows.Foundation types
        reg.register_default("Windows.Foundation.Uri");
        reg.register_default("Windows.Foundation.PropertyValue");

        // Windows.Storage types
        reg.register_default("Windows.Storage.StorageFile");
        reg.register_default("Windows.Storage.StorageFolder");
        reg.register_default("Windows.Storage.ApplicationData");

        // Windows.UI types
        reg.register_default("Windows.UI.Colors");

        // Windows.Networking types
        reg.register_default("Windows.Networking.HostName");

        log::info!("[winrt] Registered {} built-in factories", reg.factory_count());
    });
}

/// RoActivateInstance: create a new instance of a WinRT runtime class.
pub fn ro_activate_instance(class_name: &str) -> Result<u64, i32> {
    with_registry_mut(|reg| reg.activate_instance(class_name))
}

/// RoGetActivationFactory: check if a factory exists for a class name.
pub fn ro_has_factory(class_name: &str) -> bool {
    with_registry(|reg| reg.get_factory(class_name).is_ok())
}

/// WindowsCreateString: create an HSTRING from a UTF-8 string.
pub fn windows_create_string(s: &str) -> HString {
    HString::create(s)
}

/// WindowsDeleteString: destroy an HSTRING (no-op in our implementation since
/// Rust handles memory via Drop).
pub fn windows_delete_string(_hstring: HString) {
    // Dropped automatically
}

/// WindowsGetStringRawBuffer: get the UTF-16 buffer of an HSTRING.
pub fn windows_get_string_raw_buffer(hstring: &HString) -> &[u16] {
    hstring.as_wide()
}

/// Access the factory registry.
pub fn with_registry<F, R>(f: F) -> R
where
    F: FnOnce(&FactoryRegistry) -> R,
{
    let reg = FACTORY_REGISTRY.lock();
    f(reg.as_ref().expect("WinRT factory registry not initialized"))
}

/// Access the factory registry mutably.
pub fn with_registry_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut FactoryRegistry) -> R,
{
    let mut reg = FACTORY_REGISTRY.lock();
    f(reg.as_mut().expect("WinRT factory registry not initialized"))
}
