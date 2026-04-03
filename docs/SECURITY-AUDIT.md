# ClaudioOS Security & Code Quality Audit

**Date**: 2026-04-03
**Auditor**: Claude Opus 4.6 (automated forensic audit)
**Scope**: All 36 workspace crates + kernel — ~112,000 lines of Rust (excluding Cranelift forks)
**Codebase**: ClaudioOS bare-metal OS, x86_64 UEFI, no_std

---

## Executive Summary

ClaudioOS is an ambitious bare-metal Rust OS with a surprisingly mature architecture. The single-address-space design eliminates an entire class of kernel/user boundary bugs. However, the audit identified **5 critical**, **11 important**, and **9 minor** findings, primarily concentrated in:

1. **Cryptographic implementations** — placeholder crypto in the SSH stack, non-constant-time password comparison, weak RNG
2. **`unsafe` usage** — 763 total `unsafe` occurrences (170 in kernel, 593 in crates), with several unsound patterns
3. **Credential leakage** — session cookies and API keys logged to serial output
4. **Missing encryption** — SSH transport falls back to plaintext when ChaCha20-Poly1305 is "not yet wired"

The Rust type system prevents many traditional C/C++ vulnerabilities (buffer overflows, use-after-free), but `unsafe` blocks bypass these guarantees and require manual verification. The project's `no_std` constraint forces custom crypto implementations that have not undergone the scrutiny of established libraries.

---

## Critical Findings (Must Fix)

### CRIT-01: SSH Key Exchange Uses Placeholder Crypto — Complete Protocol Compromise

**File**: `crates/sshd/src/kex.rs` lines 363-504
**File**: `crates/sshd/src/hostkey.rs` lines 42-66, 88-110

Both the X25519 key exchange and Ed25519 host key operations use **placeholder implementations** that substitute SHA-256 hashes for actual elliptic curve operations. The code comments explicitly state "TODO: Wire up x25519_dalek" and "TODO: Wire up ed25519_dalek".

**Impact**: Any SSH connection has **zero cryptographic security**. The "shared secret" is deterministically derived from the raw key bytes using SHA-256, meaning an attacker who observes the key exchange can trivially compute the session keys. The host key signature is an HMAC-SHA256, not an Ed25519 signature — any client accepting this server's host key is trusting a forgeable identity.

**Evidence**:
- `kex.rs:388-398` — X25519 "public key" is just random bytes, not `secret * G`
- `kex.rs:481-486` — X25519 "shared secret" is `SHA-256(server_secret || client_public)`, not a DH computation
- `hostkey.rs:55-59` — Ed25519 "public key" is `SHA-256(secret)`, not the actual Ed25519 public key
- `hostkey.rs:98-100` — Ed25519 "signature" is `SHA-256(secret || data)`, trivially forgeable

**Recommendation**: Wire in `ed25519-dalek` (no_std compatible) and `x25519-dalek` before exposing port 22 to any network. Until then, the SSH server MUST be disabled or firewalled.

---

### CRIT-02: SSH Transport Encryption Falls Back to Plaintext

**File**: `crates/sshd/src/transport.rs` lines 287-314, 362-381

The `frame_packet_encrypted()` and `parse_packet_encrypted()` functions are supposed to implement ChaCha20-Poly1305@openssh.com, but both contain only `TODO` comments and fall back to sending/receiving plaintext packets with a `log::warn!` message.

**Impact**: Even after key exchange completes, all SSH traffic (including passwords, commands, and channel data) is transmitted in the clear. Combined with CRIT-01, the SSH server provides **no confidentiality or integrity**.

**Evidence**:
```
transport.rs:310-311:
    log::warn!("transport: ChaCha20-Poly1305 encryption not yet wired — sending plaintext");
    Ok(frame_packet(payload, rng_fill))
```

**Recommendation**: Implement `chacha20poly1305` AEAD using the `chacha20poly1305` crate (no_std compatible) or disable the SSH server entirely.

---

### CRIT-03: Non-Cryptographic RNG for SSH Host Keys and Session Keys

**File**: `kernel/src/ssh_server.rs` lines 500-514

The SSH server's RNG (`rng_fill`) uses a **xorshift64** PRNG seeded from the PIT timer tick counter plus a monotonic counter. This is not cryptographically secure.

**Impact**: An attacker who knows the approximate boot time of the system can predict the RNG state and thus predict all SSH host keys, key exchange ephemeral keys, and packet padding. This enables complete session compromise even if the real crypto were wired up.

**Evidence**:
```rust
fn rng_fill(buf: &mut [u8]) {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ticks = crate::interrupts::tick_count();
    let mut state = ticks ^ COUNTER.fetch_add(1, Ordering::Relaxed);
    for byte in buf.iter_mut() {
        state ^= state << 13;  // xorshift64 — NOT cryptographically secure
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
}
```

**Recommendation**: Use RDRAND/RDSEED instructions (available on the target i9-11900K) to seed a proper CSPRNG (e.g., ChaCha20Rng). The code already has CPUID detection — add an RDRAND check and use it as the primary entropy source.

---

### CRIT-04: Timing-Unsafe Password Comparison

**File**: `kernel/src/users.rs` line 200
**File**: `crates/sshd/src/auth.rs` line 365

Password verification uses `==` comparison on hex strings and byte slices, which is timing-vulnerable. An attacker can measure response time to determine how many leading bytes of the hash match, enabling incremental brute-force.

**Evidence**:
```rust
// users.rs:200 — timing-unsafe comparison
hex == self.password_hash

// auth.rs:365 — timing-unsafe comparison
hash.as_slice() == expected_hash.as_slice()
```

**Impact**: Remote password brute-force is accelerated by timing side-channels. Over SSH (even with the current broken crypto), response time differences on the order of nanoseconds are measurable over many samples.

**Recommendation**: Implement constant-time comparison:
```rust
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
```

---

### CRIT-05: Session Cookie and API Key Logged to Serial Output

**File**: `kernel/src/main.rs` lines 815, 830, 909

Session cookies (containing `sessionKey=sk-ant-...`) and API keys are logged to serial output at `info` level. Serial output goes to QEMU's stdout and any serial log files.

**Evidence**:
```rust
log::info!("[oauth] cookies: {}", &session_cookies[..session_cookies.len().min(500)]);
log::info!("[oauth] SAVE_SESSION:{}", session_cookie_buf);
log::info!("[auth] token: {}...{} ({} chars)", &api_key_buf[..6], &api_key_buf[api_key_buf.len()-4..], api_key_buf.len());
```

**Impact**: Anyone with access to serial logs (the many `serial_*.txt` files in the project root) can extract valid Anthropic API keys and claude.ai session cookies. The `SAVE_SESSION` marker intentionally outputs the full cookie for the host script, but this is logged alongside diagnostic output.

**Recommendation**:
1. Redact credentials in log output — show only first/last 4 chars
2. Move the `SAVE_SESSION` marker to a dedicated serial channel or use a structured protocol that the log sink filters out
3. Add the `serial_*.txt` and `proxy*.log` files to `.gitignore` (they currently are not in a git repo, but should be protected)

---

## Important Findings (Should Fix)

### IMP-01: Unsound `static mut` Global State Without Synchronization

**Files**: Multiple locations (15 instances in kernel, 4 in crates)

The codebase uses `static mut` for global state in multiple modules:

| Location | Variable | Risk |
|---|---|---|
| `kernel/src/agent_loop.rs:53-54` | `BUILD_STACK`, `BUILD_NOW_FN` | Raw pointer to NetworkStack |
| `kernel/src/agent_loop.rs:673` | `AUTH_MODE` | Auth credentials |
| `kernel/src/agent_memory.rs:1216` | `MEMORY_STORE` | Agent memory |
| `kernel/src/git.rs:1981` | `REPOS` | Git repositories |
| `kernel/src/conversations.rs:238` | `ACTIVE_CONVS` | Conversation state |
| `kernel/src/vectordb.rs:897` | `VECTOR_STORE` | Vector embeddings |
| `crates/net/src/tls.rs:627` | `LOCAL_PORT_COUNTER` | Port allocator |
| `crates/api-client/src/tools.rs:757` | `COMPILE_RUST_HANDLER` | Function pointer |

**Impact**: While ClaudioOS is currently single-threaded, `static mut` is unsound in Rust even in single-threaded code (it creates undefined behavior per the Rust reference). With the SMP crate in the workspace (`crates/smp/`), future multi-core support would turn these into data races.

**Recommendation**: Replace all `static mut` with `spin::Mutex<Option<T>>` or `spin::Once<T>`. The pattern is already used correctly in `kernel/src/users.rs:385` — apply it everywhere.

---

### IMP-02: Unbounded SSH Version String Buffer (Denial of Service)

**File**: `kernel/src/ssh_server.rs` lines 386-407

The SSH server accumulates incoming TCP data into `version_buf` (a `Vec<u8>`) until it finds `\r\n`. There is no size limit on this buffer.

**Impact**: A malicious client can send megabytes of data without `\r\n`, exhausting the 16 MiB kernel heap and causing a panic (out-of-memory). This is a denial-of-service vulnerability.

**Recommendation**: Add a maximum version string length check (RFC 4253 says version strings MUST be <= 255 characters):
```rust
if conn.version_buf.len() > 255 {
    conn.session.disconnect(2, "version string too long");
    continue;
}
```

---

### IMP-03: `SendPtr` Wrapper Circumvents Rust Safety Guarantees

**File**: `kernel/src/ssh_server.rs` lines 523-525

```rust
struct SendPtr(*mut SshListener);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}
```

This pattern wraps a raw pointer and blanket-implements `Send` and `Sync`. The safety comment says "only accessed from the single-threaded executor" — but this is an invariant that the compiler cannot verify.

**Impact**: If the code path is ever called from multiple threads (e.g., when SMP support is activated), this becomes a data race with undefined behavior.

**Recommendation**: Use `spin::Mutex<Option<Box<SshListener>>>` instead, which provides safe interior mutability.

---

### IMP-04: AES-256 Software Implementation Vulnerable to Cache Timing Attacks

**File**: `kernel/src/encryption.rs` lines 80-216 (encrypt), 235-387 (decrypt)

The AES implementation uses lookup tables (`SBOX`, `INV_SBOX`) indexed by data-dependent values. On x86_64, cache line timing differences during S-box lookups can leak the secret key.

**Impact**: A co-located attacker (same physical machine, e.g., via SMP) can perform cache-timing attacks to extract the AES-256 encryption keys. For a single-address-space OS, any code running concurrently can observe cache state.

**Recommendation**: Use AES-NI instructions (the target CPU supports them — hence the `-cpu Haswell` requirement). The `aes` crate with the `aes-ni` feature provides constant-time AES using hardware instructions. Alternatively, use a bitsliced software AES implementation.

---

### IMP-05: No Salt Uniqueness Enforcement for Disk Encryption

**File**: `kernel/src/encryption.rs` line 744

The `format()` function accepts a caller-provided salt but does not verify its randomness or uniqueness. The only RNG available (`rng_fill` from ssh_server.rs) is the non-cryptographic xorshift PRNG from CRIT-03.

**Impact**: Predictable salts weaken the PBKDF2 key derivation. Two devices formatted at similar boot times could have identical salts, enabling precomputed dictionary attacks.

**Recommendation**: Require RDRAND-sourced entropy for the salt. Refuse to format if RDRAND is unavailable.

---

### IMP-06: Key Material Not Securely Erased

**File**: `kernel/src/encryption.rs` line 768-773

The `lock()` function zeroes key material with simple assignment:
```rust
self.key1 = [0u8; 32];
self.key2 = [0u8; 32];
```

The compiler may optimize away these writes since the values are not subsequently read. The same applies to PBKDF2 intermediate values and the derived key in `unlock()`.

**Recommendation**: Use `core::ptr::write_volatile` or the `zeroize` crate to ensure key material is actually zeroed:
```rust
use core::ptr::write_volatile;
unsafe {
    write_volatile(&mut self.key1, [0u8; 32]);
    write_volatile(&mut self.key2, [0u8; 32]);
}
```

---

### IMP-07: Plain SHA-256 Password Hashing (No Salting, No Key Stretching)

**File**: `kernel/src/users.rs` lines 186-201

Passwords are hashed with a single round of SHA-256 — no salt, no iterations. This is vulnerable to rainbow table attacks and offline brute-force.

**Impact**: If the user database is ever persisted to disk or leaked, passwords are trivially crackable. Modern GPUs can compute billions of SHA-256 hashes per second.

**Recommendation**: Use PBKDF2-SHA256 (already implemented in `encryption.rs`) with per-user random salts and >=100,000 iterations. The infrastructure exists — it just needs to be wired into the user module.

---

### IMP-08: Default Firewall Policy is ALLOW

**File**: `kernel/src/firewall.rs` line 569

```rust
pub static FIREWALL: Mutex<RuleSet> = Mutex::new(RuleSet {
    ...
    default_policy: Action::Allow,
    ...
});
```

**Impact**: Until rules are explicitly configured, all inbound and outbound traffic is permitted. This means the broken SSH server (CRIT-01/02) is exposed by default.

**Recommendation**: Change the default policy to `Deny` and add explicit allow rules for required traffic (DHCP, DNS, HTTPS to api.anthropic.com).

---

### IMP-09: VFS Path Resolution Does Not Canonicalize Before Operations

**Files**: `crates/shell/src/builtin.rs` line 139-150, `crates/vfs/src/path.rs`

The `resolve_path()` function in the shell joins user input with PWD but does **not** normalize the result. While `Path::normalize()` exists and handles `..` correctly (preventing escape above `/`), it is not called in the shell's resolve path.

**Impact**: A user typing `cat ../../etc/shadow` from `/home/matt` would attempt to read `/etc/shadow` (or the VFS equivalent). The VFS layer may or may not call normalize — it depends on the filesystem adapter.

**Recommendation**: Always normalize paths before passing to VFS operations:
```rust
fn resolve_path(path: &str, env: &Environment) -> String {
    let full = if path.starts_with('/') { ... } else { ... };
    vfs::Path::new(&full).normalize().as_str().to_string()
}
```

---

### IMP-10: `Box::from_raw` Deallocation Mismatch

**File**: `crates/net/src/tls.rs` line 681

The code allocates a buffer with alignment 16 via `alloc::alloc::alloc_zeroed(Layout { size, align: 16 })` but then converts it to `Box<[u8]>`, which will deallocate with `Layout { size, align: 1 }`.

**Evidence** (from the source comment):
```
// SAFETY: ... Box<[u8]>::drop will deallocate with Layout { size, align: 1 }, which
// is technically a mismatch. This is sound with linked_list_allocator because it
// tracks allocations by address and size only...
```

**Impact**: This is technically undefined behavior per the Rust allocator API, which requires deallocation with the same layout as allocation. It works with `linked_list_allocator` but would break with other allocators.

**Recommendation**: Use a wrapper type that stores the original layout and implements `Drop` with the correct deallocation layout.

---

### IMP-11: Session Cookie Transmitted Without Integrity Protection

**File**: `kernel/src/agent_loop.rs` lines 770-775

When using claude.ai auth mode, the session cookie is included as a plain HTTP `Cookie` header over TLS. While TLS protects the transport, the session cookie is:
1. Logged to serial (CRIT-05)
2. Stored in a `static mut` without protection (IMP-01)
3. Not bound to the TLS session (no channel binding)

**Impact**: Cookie theft via serial log access enables session hijacking.

---

## Minor Findings (Nice to Fix)

### MIN-01: Numerous `unwrap()` Calls in Kernel Code

**Count**: 26 `unwrap()` and 10 `expect()` calls in `kernel/src/`

Key locations:
- `agent_memory.rs`: 10 unwrap calls
- `vectordb.rs`: 3 unwrap calls
- `dashboard.rs`: 3 unwrap calls
- `executor.rs`: 2 unwrap calls

**Impact**: Each `unwrap()` is a potential panic site. In a bare-metal OS, a panic halts the entire system.

**Recommendation**: Replace with explicit error handling (`match`, `if let`, `.unwrap_or_default()`).

---

### MIN-02: Large Functions Exceeding 100 Lines

Several files have functions that are excessively long:
- `kernel/src/main.rs`: `kernel_main` — the entire boot sequence in one function (~1200+ lines)
- `kernel/src/dashboard.rs`: likely the main event loop
- `kernel/src/git.rs`: 2120 lines total — likely monolithic functions

**Recommendation**: Refactor into smaller, testable functions.

---

### MIN-03: `panic!` in Production Code Paths

**File**: `kernel/src/intel_nic.rs` — 4 instances of `panic!`

**Impact**: Network driver panics halt the OS. Drivers should return errors, not panic.

---

### MIN-04: Unsized Connection/Rate-Limit Tables in Firewall

**File**: `kernel/src/firewall.rs` lines 387-392, 439-441

The connection table and rate limit table are limited to 1024 and 256 entries respectively with LRU eviction. Under sustained attack, legitimate connections could be evicted.

**Recommendation**: Consider using a hash map for O(1) lookups and configurable size limits.

---

### MIN-05: SSH Version String Leaks Software Name and Version

**File**: `crates/sshd/src/transport.rs` line 23

```rust
pub const SSH_VERSION_STRING: &str = "SSH-2.0-ClaudioOS_0.1";
```

**Impact**: Identifies the exact software, enabling targeted attacks. Minor concern but standard hardening practice is to use a generic string.

---

### MIN-06: No Input Validation on Shell `grep` Pattern

**File**: `crates/shell/src/builtin.rs` line 397-399

The grep command uses `line.contains(pattern)` with no input length or complexity limits. A very long pattern or many files could cause excessive CPU usage.

---

### MIN-07: Epoch/Timestamp Handling Assumes 18.2 Hz PIT

Multiple files use `18` or `182` as a conversion factor for PIT ticks to seconds. If the PIT frequency changes or NTP adjusts the clock, timeouts will be wrong.

---

### MIN-08: Thread-Safety Markers on Non-Thread-Safe Types

**File**: `crates/vfs/src/adapters.rs` lines 58-59

```rust
unsafe impl Send for AhciBlockDeviceInner {}
unsafe impl Sync for AhciBlockDeviceInner {}
```

Contains raw pointers to MMIO registers. The `Mutex` serializes access, but the `Send`/`Sync` impls allow the wrapper to escape to other threads without the `Mutex`.

---

### MIN-09: Duplicate SHA-256 Implementations

SHA-256 is implemented independently in three locations:
1. `kernel/src/users.rs` — `sha256` module (for password hashing)
2. `kernel/src/encryption.rs` — `sha256()` function (for PBKDF2)
3. `crates/sshd/` — uses the `sha2` crate

**Recommendation**: Consolidate on the `sha2` crate everywhere. The hand-rolled implementations lack test vectors and have not been verified against NIST test data.

---

## Unsafe Code Analysis

### Summary

| Location | `unsafe` Count | Risk Level |
|---|---|---|
| `kernel/src/` (28 files) | 170 | Mixed |
| `crates/` (64 files, excl. Cranelift) | ~200 | Mixed |
| `crates/cranelift-*-nostd/` | ~393 | Low (forked, well-tested) |
| **Total** | **763** | |

### High-Risk Unsafe Patterns

1. **`static mut` globals** (19 instances) — Undefined behavior potential; see IMP-01
2. **`SendPtr` / blanket `Send+Sync`** (5 instances) — Bypasses thread-safety checks
3. **Raw pointer dereferences** (39 `as *mut`/`as *const` in kernel) — Hardware MMIO access, generally appropriate for a kernel
4. **`Box::from_raw` with mismatched layout** (1 instance in tls.rs) — Technically UB

### Acceptable Unsafe Usage

- Hardware MMIO access (PCI, NIC, AHCI, xHCI registers) — inherently unsafe, properly wrapped
- GDT/IDT/interrupt handler setup — CPU-mandated unsafe operations
- Page table manipulation — necessary for bare-metal memory management
- Inline assembly for port I/O, CR register access — kernel fundamentals

---

## Tool Recommendations

### 1. `cargo clippy` — Static Analysis

```bash
# Run with all warnings enabled
cargo clippy --workspace --all-targets -- -W clippy::all -W clippy::pedantic -W clippy::nursery
```

Focus on: `clippy::cast_possible_truncation`, `clippy::cast_sign_loss`, `clippy::unwrap_used`, `clippy::panic`.

### 2. `cargo audit` — Dependency Vulnerability Scan

```bash
cargo install cargo-audit
cargo audit
```

Check for known vulnerabilities in dependencies, especially `sha2`, `embedded-tls`, `smoltcp`, and the `spin` mutex.

### 3. MIRI — Memory Safety Verification

```bash
rustup +nightly component add miri
cargo +nightly miri test -p claudio-editor -p claudio-python-lite
```

MIRI cannot run on `no_std` kernel code, but it CAN check the pure-logic crates (editor, python-lite, wraith-dom, wraith-render, shell, vfs). These have ~130+ tests.

### 4. `cargo fuzz` — Fuzz Testing

Priority fuzz targets:
1. **SSH wire parser** (`crates/sshd/src/wire.rs` — `SshReader`) — protocol parsing is the #1 attack surface
2. **SSH transport** (`crates/sshd/src/transport.rs` — `parse_packet()`) — malformed packets
3. **HTTP parser** (`crates/net/src/http.rs`) — HTTP response parsing
4. **HTML parser** (`crates/wraith-dom/src/parser.rs`) — untrusted web content
5. **ELF loader** (`crates/elf-loader/`) — binary parsing

```bash
cargo install cargo-fuzz
# Create fuzz targets for each parser
cargo fuzz init
# Add targets for SshReader::read_string_raw, parse_packet, etc.
```

### 5. Semgrep — Custom Rust Rules

```bash
# Install semgrep
pip install semgrep

# Key rules to write:
# 1. Detect `static mut` usage
# 2. Detect `unsafe impl Send/Sync` without documentation
# 3. Detect `==` comparison on crypto material (timing attacks)
# 4. Detect logging of secrets (api_key, cookie, token, password)
```

### 6. Custom AES Test Vectors

Validate the hand-rolled AES-256 implementation against NIST FIPS 197 test vectors:
```rust
#[test]
fn test_aes256_nist_vector() {
    let key = hex!("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
    let mut block = hex!("00112233445566778899aabbccddeeff");
    aes256_encrypt_block(&key, &mut block);
    assert_eq!(block, hex!("8ea2b7ca516745bfeafc49904b496089"));
}
```

---

## Architecture Strengths

Despite the findings, the codebase demonstrates strong engineering in several areas:

1. **Proper error propagation** — Most functions return `Result` types; panics are rare
2. **Well-structured SSH protocol** — Clean separation of transport/kex/auth/channel layers
3. **Firewall design** — Stateful connection tracking with proper timeout handling
4. **VFS path normalization** — `..` resolution prevents escape above root (when called)
5. **PBKDF2 implementation** — Correct algorithm (in encryption.rs), just not used everywhere
6. **Auth attempt limiting** — SSH allows max 6 attempts, user DB checks for locked accounts
7. **Verbose logging** — Nearly every operation is logged, excellent for debugging
8. **Single address space** — Eliminates TOCTOU and kernel/user boundary bugs entirely

---

## Priority Remediation Plan

| Priority | Finding | Effort | Impact |
|---|---|---|---|
| **P0** | CRIT-01 + CRIT-02: Wire real crypto into SSH or disable it | 2-3 days | Eliminates SSH exposure |
| **P0** | CRIT-03: Replace xorshift with RDRAND-seeded CSPRNG | 1 day | Fixes all crypto randomness |
| **P0** | CRIT-05: Redact credentials in log output | 1 hour | Prevents credential leakage |
| **P1** | CRIT-04: Constant-time password comparison | 1 hour | Eliminates timing attacks |
| **P1** | IMP-01: Replace `static mut` with `spin::Mutex` | 1 day | Eliminates UB |
| **P1** | IMP-02: Bound SSH version buffer | 15 min | Prevents heap DoS |
| **P1** | IMP-07: Salt + PBKDF2 for user passwords | 2 hours | Proper password storage |
| **P1** | IMP-08: Default-deny firewall policy | 15 min | Defense in depth |
| **P2** | IMP-04: Use AES-NI instead of table-based AES | 1 day | Cache timing resistance |
| **P2** | IMP-06: Volatile key zeroing | 1 hour | Proper key cleanup |
| **P2** | IMP-09: Canonicalize VFS paths in shell | 30 min | Path traversal prevention |
| **P3** | MIN-01-09: Code quality improvements | 2-3 days | Reliability |

---

*This audit was performed through static code analysis. Dynamic testing (fuzzing, MIRI, actual network testing) is strongly recommended as a follow-up.*
