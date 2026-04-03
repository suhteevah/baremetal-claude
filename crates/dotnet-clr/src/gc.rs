//! Simple mark-and-sweep garbage collector for the .NET managed heap.
//!
//! Manages object allocation and collection. Objects are allocated on a flat
//! heap with handles (u64 IDs). GC roots include the evaluation stack, locals,
//! args, and static fields.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::types::{ClrValue, ManagedObject, ManagedArray};

/// Next object handle counter.
static NEXT_HANDLE: Mutex<u64> = Mutex::new(1);

/// The managed heap: all live .NET objects and arrays.
static MANAGED_HEAP: Mutex<Option<ManagedHeap>> = Mutex::new(None);

/// A managed heap for .NET objects.
pub struct ManagedHeap {
    /// Object store (handle -> object).
    objects: BTreeMap<u64, ManagedObject>,
    /// Array store (handle -> array).
    arrays: BTreeMap<u64, ManagedArray>,
    /// Total bytes allocated (approximate).
    bytes_allocated: usize,
    /// GC threshold: collect when bytes_allocated exceeds this.
    gc_threshold: usize,
    /// Number of collections performed.
    collections: u64,
    /// Static field roots (type::field -> value).
    static_fields: BTreeMap<String, ClrValue>,
}

impl ManagedHeap {
    /// Create a new managed heap.
    pub fn new() -> Self {
        Self {
            objects: BTreeMap::new(),
            arrays: BTreeMap::new(),
            bytes_allocated: 0,
            gc_threshold: 4 * 1024 * 1024, // 4 MiB initial threshold
            collections: 0,
            static_fields: BTreeMap::new(),
        }
    }

    /// Allocate a new managed object and return its handle.
    pub fn alloc_object(&mut self, obj: ManagedObject) -> u64 {
        let handle = alloc_handle();
        let size = core::mem::size_of::<ManagedObject>() + obj.fields.len() * 32;
        self.bytes_allocated += size;
        self.objects.insert(handle, obj);
        log::trace!("[dotnet-gc] Allocated object handle={}, total={}B", handle, self.bytes_allocated);
        handle
    }

    /// Allocate a new managed array and return its handle.
    pub fn alloc_array(&mut self, arr: ManagedArray) -> u64 {
        let handle = alloc_handle();
        let size = core::mem::size_of::<ManagedArray>() + arr.elements.len() * 16;
        self.bytes_allocated += size;
        self.arrays.insert(handle, arr);
        log::trace!("[dotnet-gc] Allocated array handle={}, total={}B", handle, self.bytes_allocated);
        handle
    }

    /// Get a reference to a managed object by handle.
    pub fn get_object(&self, handle: u64) -> Option<&ManagedObject> {
        self.objects.get(&handle)
    }

    /// Get a mutable reference to a managed object by handle.
    pub fn get_object_mut(&mut self, handle: u64) -> Option<&mut ManagedObject> {
        self.objects.get_mut(&handle)
    }

    /// Get a reference to a managed array by handle.
    pub fn get_array(&self, handle: u64) -> Option<&ManagedArray> {
        self.arrays.get(&handle)
    }

    /// Get a mutable reference to a managed array by handle.
    pub fn get_array_mut(&mut self, handle: u64) -> Option<&mut ManagedArray> {
        self.arrays.get_mut(&handle)
    }

    /// Get a static field value.
    pub fn get_static_field(&self, key: &str) -> Option<&ClrValue> {
        self.static_fields.get(key)
    }

    /// Set a static field value.
    pub fn set_static_field(&mut self, key: String, value: ClrValue) {
        self.static_fields.insert(key, value);
    }

    /// Check if GC collection should be triggered.
    pub fn should_collect(&self) -> bool {
        self.bytes_allocated >= self.gc_threshold
    }

    /// Perform a mark-and-sweep collection.
    ///
    /// `roots` are additional GC root values (from eval stack, locals, args)
    /// that should keep objects alive.
    pub fn collect(&mut self, roots: &[ClrValue]) {
        self.collections += 1;
        let before = self.objects.len() + self.arrays.len();

        log::debug!(
            "[dotnet-gc] Collection #{}: {} objects, {} arrays, {}B allocated",
            self.collections, self.objects.len(), self.arrays.len(), self.bytes_allocated,
        );

        // Phase 1: Clear all marks
        for obj in self.objects.values_mut() {
            obj.gc_marked = false;
        }
        for arr in self.arrays.values_mut() {
            arr.gc_marked = false;
        }

        // Phase 2: Mark from roots
        let mut worklist = Vec::new();

        // Mark from provided roots
        for root in roots {
            self.extract_handles(root, &mut worklist);
        }

        // Mark from static fields
        let static_handles: Vec<u64> = {
            let mut handles = Vec::new();
            for val in self.static_fields.values() {
                Self::extract_handles_from_value(val, &mut handles);
            }
            handles
        };
        worklist.extend(static_handles);

        // Process worklist
        while let Some(handle) = worklist.pop() {
            // Try as object
            if let Some(obj) = self.objects.get_mut(&handle) {
                if !obj.gc_marked {
                    obj.gc_marked = true;
                    // Mark fields
                    let field_handles: Vec<u64> = {
                        let mut h = Vec::new();
                        for val in obj.fields.values() {
                            Self::extract_handles_from_value(val, &mut h);
                        }
                        h
                    };
                    worklist.extend(field_handles);
                }
            }
            // Try as array
            if let Some(arr) = self.arrays.get_mut(&handle) {
                if !arr.gc_marked {
                    arr.gc_marked = true;
                    let elem_handles: Vec<u64> = {
                        let mut h = Vec::new();
                        for val in &arr.elements {
                            Self::extract_handles_from_value(val, &mut h);
                        }
                        h
                    };
                    worklist.extend(elem_handles);
                }
            }
        }

        // Phase 3: Sweep unmarked objects
        let dead_objects: Vec<u64> = self
            .objects
            .iter()
            .filter(|(_, obj)| !obj.gc_marked)
            .map(|(&handle, _)| handle)
            .collect();
        for handle in &dead_objects {
            self.objects.remove(handle);
        }

        let dead_arrays: Vec<u64> = self
            .arrays
            .iter()
            .filter(|(_, arr)| !arr.gc_marked)
            .map(|(&handle, _)| handle)
            .collect();
        for handle in &dead_arrays {
            self.arrays.remove(handle);
        }

        // Recalculate allocated bytes
        self.bytes_allocated = self.objects.len() * 128 + self.arrays.len() * 64;

        let after = self.objects.len() + self.arrays.len();
        let freed = before - after;

        log::debug!(
            "[dotnet-gc] Collection #{} complete: freed {} objects, {} remaining, {}B",
            self.collections, freed, after, self.bytes_allocated,
        );

        // Grow threshold if we're still using a lot of memory
        if self.bytes_allocated > self.gc_threshold / 2 {
            self.gc_threshold *= 2;
        }
    }

    /// Extract object/array handles from a ClrValue.
    fn extract_handles(&self, val: &ClrValue, out: &mut Vec<u64>) {
        Self::extract_handles_from_value(val, out);
    }

    fn extract_handles_from_value(val: &ClrValue, out: &mut Vec<u64>) {
        match val {
            ClrValue::ObjectRef(h) => {
                if *h != 0 {
                    out.push(*h);
                }
            }
            ClrValue::ArrayRef(h) => {
                if *h != 0 {
                    out.push(*h);
                }
            }
            ClrValue::Boxed(inner) => {
                Self::extract_handles_from_value(inner, out);
            }
            _ => {}
        }
    }

    /// Get stats about the managed heap.
    pub fn stats(&self) -> GcStats {
        GcStats {
            object_count: self.objects.len(),
            array_count: self.arrays.len(),
            bytes_allocated: self.bytes_allocated,
            collections: self.collections,
            gc_threshold: self.gc_threshold,
        }
    }
}

/// GC statistics.
#[derive(Debug, Clone)]
pub struct GcStats {
    pub object_count: usize,
    pub array_count: usize,
    pub bytes_allocated: usize,
    pub collections: u64,
    pub gc_threshold: usize,
}

/// Allocate a unique handle.
fn alloc_handle() -> u64 {
    let mut counter = NEXT_HANDLE.lock();
    let handle = *counter;
    *counter += 1;
    handle
}

/// Initialize the global managed heap.
pub fn init() {
    let mut heap = MANAGED_HEAP.lock();
    if heap.is_none() {
        *heap = Some(ManagedHeap::new());
        log::info!("[dotnet-gc] Managed heap initialized (threshold=4MiB)");
    }
}

/// Access the global managed heap.
pub fn with_heap<F, R>(f: F) -> R
where
    F: FnOnce(&ManagedHeap) -> R,
{
    let heap = MANAGED_HEAP.lock();
    f(heap.as_ref().expect("Managed heap not initialized"))
}

/// Access the global managed heap mutably.
pub fn with_heap_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut ManagedHeap) -> R,
{
    let mut heap = MANAGED_HEAP.lock();
    f(heap.as_mut().expect("Managed heap not initialized"))
}
