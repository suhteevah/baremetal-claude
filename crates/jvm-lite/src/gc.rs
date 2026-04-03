//! Simple mark-sweep garbage collector for the JVM heap.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::vm::JvmValue;

/// A heap-allocated JVM object.
#[derive(Debug, Clone)]
pub struct JvmObject {
    /// Class name of this object.
    pub class_name: String,
    /// Instance fields: name -> value.
    pub fields: BTreeMap<String, JvmValue>,
    /// If this is a String object, its value.
    pub string_value: Option<String>,
    /// If this is an array, its elements.
    pub array_elements: Option<Vec<JvmValue>>,
    /// GC mark bit.
    pub marked: bool,
}

impl JvmObject {
    pub fn new(class_name: String) -> Self {
        Self {
            class_name,
            fields: BTreeMap::new(),
            string_value: None,
            array_elements: None,
            marked: false,
        }
    }

    pub fn new_string(value: String) -> Self {
        Self {
            class_name: String::from("java/lang/String"),
            fields: BTreeMap::new(),
            string_value: Some(value),
            array_elements: None,
            marked: false,
        }
    }

    pub fn new_array(length: usize) -> Self {
        Self {
            class_name: String::from("["),
            fields: BTreeMap::new(),
            string_value: None,
            array_elements: Some((0..length).map(|_| JvmValue::Null).collect()),
            marked: false,
        }
    }
}

/// The JVM object heap with mark-sweep GC.
pub struct JvmHeap {
    /// All allocated objects, indexed by reference ID.
    objects: Vec<Option<JvmObject>>,
    /// Free list of available indices.
    free_list: Vec<usize>,
    /// Total number of allocations since last GC.
    alloc_count: usize,
    /// GC threshold: collect when alloc_count exceeds this.
    gc_threshold: usize,
}

impl JvmHeap {
    pub fn new() -> Self {
        Self {
            objects: Vec::new(),
            free_list: Vec::new(),
            alloc_count: 0,
            gc_threshold: 1024,
        }
    }

    /// Allocate a new object, returning its reference ID.
    pub fn allocate(&mut self) -> usize {
        self.alloc_count += 1;
        self.allocate_object(JvmObject::new(String::from("java/lang/Object")))
    }

    /// Allocate a specific object.
    pub fn allocate_object(&mut self, obj: JvmObject) -> usize {
        if let Some(idx) = self.free_list.pop() {
            self.objects[idx] = Some(obj);
            idx
        } else {
            let idx = self.objects.len();
            self.objects.push(Some(obj));
            idx
        }
    }

    /// Get a reference to an object.
    pub fn get(&self, reference: usize) -> Option<&JvmObject> {
        self.objects.get(reference).and_then(|o| o.as_ref())
    }

    /// Get a mutable reference to an object.
    pub fn get_mut(&mut self, reference: usize) -> Option<&mut JvmObject> {
        self.objects.get_mut(reference).and_then(|o| o.as_mut())
    }

    /// Mark phase: mark all objects reachable from roots.
    pub fn mark(&mut self, roots: &[usize]) {
        // Clear all marks
        for obj in self.objects.iter_mut().flatten() {
            obj.marked = false;
        }

        // Mark from roots
        let mut worklist: Vec<usize> = roots.to_vec();
        while let Some(idx) = worklist.pop() {
            if let Some(obj) = self.objects.get_mut(idx).and_then(|o| o.as_mut()) {
                if obj.marked {
                    continue;
                }
                obj.marked = true;

                // Trace references in fields
                for val in obj.fields.values() {
                    if let JvmValue::Ref(r) = val {
                        worklist.push(*r);
                    }
                }

                // Trace array elements
                if let Some(ref elems) = obj.array_elements {
                    for val in elems {
                        if let JvmValue::Ref(r) = val {
                            worklist.push(*r);
                        }
                    }
                }
            }
        }
    }

    /// Sweep phase: free all unmarked objects.
    pub fn sweep(&mut self) -> usize {
        let mut freed = 0;
        for i in 0..self.objects.len() {
            if let Some(ref obj) = self.objects[i] {
                if !obj.marked {
                    self.objects[i] = None;
                    self.free_list.push(i);
                    freed += 1;
                }
            }
        }
        freed
    }

    /// Run a full GC cycle.
    pub fn collect(&mut self, roots: &[usize]) -> usize {
        self.mark(roots);
        let freed = self.sweep();
        self.alloc_count = 0;
        log::debug!("[jvm-gc] collected {} objects, {} remaining",
            freed, self.objects.iter().flatten().count());
        freed
    }

    /// Check if GC should run.
    pub fn should_collect(&self) -> bool {
        self.alloc_count >= self.gc_threshold
    }

    /// Total allocated objects.
    pub fn object_count(&self) -> usize {
        self.objects.iter().flatten().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allocate_and_get() {
        let mut heap = JvmHeap::new();
        let obj = JvmObject::new(String::from("Test"));
        let idx = heap.allocate_object(obj);
        assert!(heap.get(idx).is_some());
        assert_eq!(heap.get(idx).unwrap().class_name, "Test");
    }

    #[test]
    fn test_gc_collect() {
        let mut heap = JvmHeap::new();
        let a = heap.allocate_object(JvmObject::new(String::from("A")));
        let b = heap.allocate_object(JvmObject::new(String::from("B")));
        let c = heap.allocate_object(JvmObject::new(String::from("C")));

        // Only 'a' is a root
        let freed = heap.collect(&[a]);
        assert_eq!(freed, 2); // b and c should be freed
        assert!(heap.get(a).is_some());
        assert!(heap.get(b).is_none());
        assert!(heap.get(c).is_none());
    }

    #[test]
    fn test_gc_reachable_chain() {
        let mut heap = JvmHeap::new();
        let a = heap.allocate_object(JvmObject::new(String::from("A")));
        let b = heap.allocate_object(JvmObject::new(String::from("B")));

        // a -> b via field
        heap.get_mut(a).unwrap().fields.insert(
            String::from("ref"),
            JvmValue::Ref(b),
        );

        let freed = heap.collect(&[a]);
        assert_eq!(freed, 0); // both reachable
        assert!(heap.get(a).is_some());
        assert!(heap.get(b).is_some());
    }
}
