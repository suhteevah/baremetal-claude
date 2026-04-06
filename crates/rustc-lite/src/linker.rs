//! Post-compilation linker for rustc-lite.
//!
//! After all functions are compiled to machine code, this module patches
//! the placeholder call addresses with real function pointers, enabling
//! inter-function calls to work.
//!
//! ## How it works
//!
//! The codegen emits `call_indirect` with a placeholder address of
//! `0xDEAD_0000 + name_hash` for each inter-function call. The hash is:
//!
//! ```ignore
//! let mut h: i64 = 0;
//! for b in name.bytes() { h = h.wrapping_mul(31).wrapping_add(b as i64); }
//! h & 0x0FFF_FFFF
//! ```
//!
//! After all functions are compiled and loaded into executable memory,
//! this linker scans each function's machine code for these placeholder
//! 64-bit immediates and replaces them with the real load addresses.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// A compiled function with its code and metadata needed for linking.
pub struct LinkableFunction {
    pub name: String,
    pub code: Vec<u8>,
    /// The address where this function's code is loaded in memory.
    pub load_addr: usize,
}

/// Patch all inter-function call sites in the compiled code.
///
/// Builds a map from placeholder addresses to real load addresses,
/// then scans each function's machine code for the placeholders and
/// patches them in-place.
///
/// After calling this, the caller must copy the patched code back to
/// the executable memory regions (or patch in-place if the memory is
/// writable).
pub fn link_functions(functions: &mut [LinkableFunction]) -> usize {
    // Build placeholder -> real address map
    let mut addr_map: BTreeMap<u64, usize> = BTreeMap::new();
    for func in functions.iter() {
        let placeholder = compute_placeholder(&func.name);
        log::debug!(
            "[linker] registered '{}' placeholder=0x{:016X} addr=0x{:X}",
            func.name,
            placeholder,
            func.load_addr
        );
        addr_map.insert(placeholder, func.load_addr);
    }

    // Scan each function's code for placeholder addresses and patch them
    let mut total_patches = 0;
    for func in functions.iter_mut() {
        let patches = patch_code(&mut func.code, func.load_addr, &addr_map);
        if patches > 0 {
            log::info!(
                "[linker] patched {} call site(s) in '{}'",
                patches,
                func.name
            );
        }
        total_patches += patches;
    }

    log::info!(
        "[linker] linking complete: {} total patches across {} functions",
        total_patches,
        functions.len()
    );
    total_patches
}

/// Compute the placeholder address for a function name.
/// Must match the codegen's hash function exactly.
pub fn compute_placeholder(name: &str) -> u64 {
    // Java-style string hash (factor 31) — simple, deterministic, no_std friendly
    let mut h: i64 = 0;
    for b in name.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as i64);
    }
    // Mask to 28 bits so placeholder fits in range [0xDEAD_0000, 0xDEAD_0FFF_FFFF]
    let name_hash = h & 0x0FFF_FFFF;
    0xDEAD_0000_u64.wrapping_add(name_hash as u64)
}

/// Scan machine code for 64-bit immediate loads of placeholder addresses
/// and replace them with actual addresses.
///
/// On x86_64, `iconst(I64, addr)` typically compiles to:
/// - `movabs rXX, imm64` (REX.W + B8+rd + 8-byte immediate)
///   Encoding: 48/49 B8-BF followed by 8 bytes little-endian
///
/// We scan for any 8-byte sequence matching a known placeholder and
/// replace it with the real address. This is conservative -- it will
/// also match data that happens to look like a placeholder, but since
/// the placeholder range (0xDEAD_0000..0xDEAD_0FFF_FFFF) is unlikely
/// to appear as real data, false positives are extremely rare.
fn patch_code(code: &mut [u8], _self_addr: usize, addr_map: &BTreeMap<u64, usize>) -> usize {
    if code.len() < 8 {
        return 0;
    }

    let mut patches = 0;
    // Slide a byte-at-a-time window looking for 8-byte placeholder immediates.
    // Byte-granular scan is needed because x86_64 instructions are variable-length
    // and the immediate can appear at any offset within a movabs encoding.
    let mut i = 0;
    while i <= code.len() - 8 {
        // Read 8 bytes as little-endian u64
        let val = u64::from_le_bytes([
            code[i],
            code[i + 1],
            code[i + 2],
            code[i + 3],
            code[i + 4],
            code[i + 5],
            code[i + 6],
            code[i + 7],
        ]);

        // Check if this matches any placeholder
        if let Some(&real_addr) = addr_map.get(&val) {
            let addr_bytes = (real_addr as u64).to_le_bytes();
            code[i..i + 8].copy_from_slice(&addr_bytes);
            log::debug!(
                "[linker] patched 0x{:016X} -> 0x{:016X} at offset {}",
                val,
                real_addr,
                i
            );
            patches += 1;
            // Advance past patched bytes so the new address isn't re-scanned
            i += 8;
        } else {
            // No match at this offset — advance one byte (not 8) since the next
            // placeholder could start at any alignment within the instruction stream
            i += 1;
        }
    }

    patches
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_placeholder_deterministic() {
        let p1 = compute_placeholder("foo");
        let p2 = compute_placeholder("foo");
        assert_eq!(p1, p2);

        // Different names should (almost certainly) produce different placeholders
        let p3 = compute_placeholder("bar");
        assert_ne!(p1, p3);
    }

    #[test]
    fn test_compute_placeholder_in_range() {
        let p = compute_placeholder("test_function");
        assert!(p >= 0xDEAD_0000);
        assert!(p < 0xDEAD_0000 + 0x1000_0000);
    }

    #[test]
    fn test_patch_simple() {
        // Create a fake function "callee" at address 0x1234_5678_9ABC_DEF0
        let callee_placeholder = compute_placeholder("callee");
        let callee_addr: usize = 0x0000_7FFF_1234_5678;

        // Create code containing the placeholder as an 8-byte little-endian immediate
        let placeholder_bytes = callee_placeholder.to_le_bytes();
        let mut code = Vec::new();
        // Some prefix bytes (like a REX.W + MOV opcode)
        code.push(0x48);
        code.push(0xB8);
        code.extend_from_slice(&placeholder_bytes);
        // Some suffix bytes
        code.push(0xCC);
        code.push(0xCC);

        let mut functions = vec![
            LinkableFunction {
                name: String::from("caller"),
                code,
                load_addr: 0x0000_7FFF_0000_0000,
            },
            LinkableFunction {
                name: String::from("callee"),
                code: vec![0xC3], // ret
                load_addr: callee_addr,
            },
        ];

        let patches = link_functions(&mut functions);
        assert_eq!(patches, 1);

        // Verify the placeholder was replaced with the real address
        let patched = &functions[0].code;
        let patched_val = u64::from_le_bytes([
            patched[2], patched[3], patched[4], patched[5], patched[6], patched[7],
            patched[8], patched[9],
        ]);
        assert_eq!(patched_val, callee_addr as u64);
    }

    #[test]
    fn test_no_false_patches() {
        // Code with no placeholder values should not be patched
        let mut functions = vec![LinkableFunction {
            name: String::from("lonely"),
            code: vec![0x48, 0x89, 0xC0, 0xC3], // mov rax, rax; ret
            load_addr: 0x1000,
        }];

        let patches = link_functions(&mut functions);
        assert_eq!(patches, 0);
    }

    #[test]
    fn test_multiple_call_sites() {
        let p_foo = compute_placeholder("foo");
        let p_bar = compute_placeholder("bar");

        let mut code = Vec::new();
        // First call to foo
        code.push(0x48);
        code.push(0xB8);
        code.extend_from_slice(&p_foo.to_le_bytes());
        // Some instructions between calls
        code.extend_from_slice(&[0xFF, 0xD0]); // call rax
        // Second call to bar
        code.push(0x48);
        code.push(0xB8);
        code.extend_from_slice(&p_bar.to_le_bytes());
        code.extend_from_slice(&[0xFF, 0xD0]); // call rax

        let mut functions = vec![
            LinkableFunction {
                name: String::from("main"),
                code,
                load_addr: 0x1000,
            },
            LinkableFunction {
                name: String::from("foo"),
                code: vec![0xC3],
                load_addr: 0x2000,
            },
            LinkableFunction {
                name: String::from("bar"),
                code: vec![0xC3],
                load_addr: 0x3000,
            },
        ];

        let patches = link_functions(&mut functions);
        assert_eq!(patches, 2);
    }
}
