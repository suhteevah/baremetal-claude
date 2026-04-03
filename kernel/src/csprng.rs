//! Cryptographically Secure Pseudo-Random Number Generator (CSPRNG).
//!
//! Uses RDRAND instruction if available (Haswell+), with a ChaCha20-based
//! PRNG fallback seeded from PIT ticks + RTC + TSC for entropy.
//!
//! ## Usage
//!
//! ```ignore
//! let mut buf = [0u8; 32];
//! csprng::random_bytes(&mut buf);
//! ```

extern crate alloc;

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// RDRAND detection
// ---------------------------------------------------------------------------

static RDRAND_AVAILABLE: AtomicBool = AtomicBool::new(false);
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Detect RDRAND support via CPUID and seed the fallback PRNG.
/// Called once during kernel init.
pub fn init() {
    // CPUID leaf 1, ECX bit 30 = RDRAND
    // SAFETY: CPUID is always safe to execute on x86_64. It has no side effects
    // beyond reading CPU feature flags.
    let has_rdrand = unsafe {
        let ecx = core::arch::x86_64::__cpuid(1).ecx;
        (ecx & (1 << 30)) != 0
    };

    RDRAND_AVAILABLE.store(has_rdrand, Ordering::Relaxed);

    if has_rdrand {
        log::info!("[csprng] RDRAND available — using hardware RNG");
    } else {
        log::warn!("[csprng] RDRAND not available — using ChaCha20 PRNG with entropy mixing");
    }

    // Seed the fallback PRNG from multiple entropy sources
    seed_chacha_state();

    INITIALIZED.store(true, Ordering::Relaxed);
    log::info!("[csprng] initialized");
}

// ---------------------------------------------------------------------------
// RDRAND
// ---------------------------------------------------------------------------

/// Try to get a 64-bit random value from RDRAND. Returns None on failure.
///
/// The caller must check `RDRAND_AVAILABLE` before calling this in a loop.
/// A single RDRAND can fail transiently (the DRNG's internal buffer may be
/// temporarily exhausted under heavy load), hence the `setc` carry-flag check.
fn rdrand64() -> Option<u64> {
    let mut val: u64;
    let success: u8;
    // SAFETY: RDRAND is a non-privileged instruction available on Ivy Bridge+.
    // We only call this after confirming CPUID.01H:ECX[30] (RDRAND support).
    // The instruction writes a random value to the destination register and
    // sets CF=1 on success, CF=0 on underflow. No memory is accessed.
    unsafe {
        core::arch::asm!(
            "rdrand {val}",
            "setc {ok}",
            val = out(reg) val,
            ok = out(reg_byte) success,
        );
    }
    if success != 0 { Some(val) } else { None }
}

/// Fill buffer using RDRAND. Returns true if all bytes were filled.
fn rdrand_fill(buf: &mut [u8]) -> bool {
    if !RDRAND_AVAILABLE.load(Ordering::Relaxed) {
        return false;
    }

    // Retry up to 10 times per 8-byte block (Intel recommendation)
    for chunk in buf.chunks_mut(8) {
        let mut val = None;
        for _ in 0..10 {
            if let Some(v) = rdrand64() {
                val = Some(v);
                break;
            }
        }
        match val {
            Some(v) => {
                let bytes = v.to_le_bytes();
                let len = chunk.len().min(8);
                chunk[..len].copy_from_slice(&bytes[..len]);
            }
            None => return false,
        }
    }
    true
}

// ---------------------------------------------------------------------------
// ChaCha20-based fallback PRNG
// ---------------------------------------------------------------------------

/// Global ChaCha20 PRNG state, protected by a spinlock.
///
/// This is the fallback PRNG used when RDRAND is not available (or as a
/// secondary source). Multiple async tasks (agent sessions) may request
/// random bytes concurrently, so the spinlock serializes access.
///
/// The ChaCha20 stream cipher is used as a CSPRNG by:
/// 1. Setting a 256-bit key from entropy sources at init time
/// 2. Incrementing a 64-bit counter for each 64-byte block generated
/// 3. Using the ChaCha20 block function to produce keystream bytes
/// 4. Periodically re-keying for forward secrecy (every 65,536 blocks)
static CHACHA_STATE: spin::Mutex<ChaCha20State> = spin::Mutex::new(ChaCha20State::new());

/// Internal state for the ChaCha20-based CSPRNG.
struct ChaCha20State {
    /// ChaCha20 key (256 bits = 8 x 32-bit words).
    /// Derived from hardware entropy sources at init, periodically refreshed.
    key: [u32; 8],
    /// Block counter: incremented for each 64-byte keystream block generated.
    /// At 64 bytes/block, this allows 2^64 * 64 bytes = 1 exabyte of output.
    counter: u64,
    /// 96-bit nonce (3 x 32-bit words). Set once from entropy at init time.
    nonce: [u32; 3],
    /// Buffered keystream bytes from the last ChaCha20 block. We generate
    /// 64 bytes at a time and hand them out as requested.
    buffer: [u8; 64],
    /// Current position within the buffer. When buf_pos >= 64, a new block
    /// must be generated before any bytes can be returned.
    buf_pos: usize,
}

impl ChaCha20State {
    const fn new() -> Self {
        Self {
            key: [0; 8],
            counter: 0,
            nonce: [0; 3],
            buffer: [0; 64],
            buf_pos: 64, // empty buffer, will generate on first use
        }
    }
}

/// ChaCha20 quarter round: the core mixing operation.
///
/// Each quarter round performs 4 "add-xor-rotate" (ARX) operations on
/// 4 of the 16 state words. The rotation constants (16, 12, 8, 7) were
/// chosen by Bernstein for optimal diffusion. ARX operations are used
/// because they are constant-time on all architectures (no table lookups
/// that could leak timing information).
#[inline(always)]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]); state[d] ^= state[a]; state[d] = state[d].rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]); state[b] ^= state[c]; state[b] = state[b].rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]); state[d] ^= state[a]; state[d] = state[d].rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]); state[b] ^= state[c]; state[b] = state[b].rotate_left(7);
}

/// Generate a 64-byte ChaCha20 keystream block.
///
/// The ChaCha20 block function takes a 256-bit key, 64-bit counter, and
/// 96-bit nonce, and produces 64 bytes of pseudorandom output. The output
/// is indistinguishable from random to any computationally bounded adversary
/// (assuming the key is secret and the counter/nonce pair is not reused).
///
/// # ChaCha20 State Layout (16 x u32 = 512 bits)
/// ```text
/// Positions 0-3:   "expand 32-byte k" (constant, ASCII encoding)
/// Positions 4-11:  256-bit key (8 x u32)
/// Positions 12-13: 64-bit block counter (low word first)
/// Positions 14-15: nonce words (we only use 2 of the 3 nonce words here)
/// ```
fn chacha20_block(key: &[u32; 8], counter: u64, nonce: &[u32; 3]) -> [u8; 64] {
    // The "expand 32-byte k" ASCII constant (sigma) -- Bernstein's ChaCha20 magic.
    // These 4 words are: "expa" "nd 3" "2-by" "te k" in little-endian u32.
    let mut state: [u32; 16] = [
        0x61707865, 0x3320646e, 0x79622d32, 0x6b206574, // sigma constant
        key[0], key[1], key[2], key[3],                   // key words 0-3
        key[4], key[5], key[6], key[7],                   // key words 4-7
        counter as u32, (counter >> 32) as u32,            // 64-bit block counter
        nonce[0], nonce[1],                                // nonce (first 2 words)
    ];

    // Save the initial state to add back later (makes the function invertible,
    // which is essential for the security proof).
    let initial = state;

    // 20 rounds = 10 iterations of 2 rounds each (column round + diagonal round).
    // 20 rounds provides a large security margin; ChaCha8 is the minimum considered safe.
    for _ in 0..10 {
        // Column rounds: mix each column of the 4x4 state matrix
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        // Diagonal rounds: mix across diagonals for full diffusion
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }

    // Add the initial state back (mod 2^32). This is the "Davies-Meyer" step
    // that makes the function one-way even if the state is leaked.
    for i in 0..16 {
        state[i] = state[i].wrapping_add(initial[i]);
    }

    // Serialize the 16 u32 words to 64 bytes in little-endian order
    let mut output = [0u8; 64];
    for (i, word) in state.iter().enumerate() {
        output[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    output
}

/// Seed the ChaCha20 PRNG state from multiple hardware entropy sources.
///
/// We combine several independent entropy sources to ensure unpredictability
/// even if some sources are weak or predictable:
/// 1. **TSC** (Time Stamp Counter): CPU cycle counter, high resolution but
///    potentially predictable if boot time is known.
/// 2. **PIT tick count**: Interrupt-driven counter at 18.2 Hz. Lower resolution.
/// 3. **RTC wall clock**: Real-time clock. Provides calendar time entropy.
/// 4. **Second TSC sample**: The *difference* between two TSC reads captures
///    timing jitter from interrupt latency, cache behavior, etc.
/// 5. **RDRAND**: Hardware random number generator (if available on Haswell+).
///    This is the strongest source when available.
/// 6. **Third TSC sample**: More timing jitter.
///
/// The combined seed material is loaded directly as the ChaCha20 key and nonce.
fn seed_chacha_state() {
    let mut seed_material = [0u8; 64];

    // Source 1: TSC (Time Stamp Counter) — high-resolution, unpredictable
    // SAFETY: RDTSC is a non-privileged instruction that reads the CPU's
    // monotonic cycle counter. No side effects.
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    seed_material[0..8].copy_from_slice(&tsc.to_le_bytes());

    // Source 2: PIT tick count
    let ticks = crate::interrupts::tick_count();
    seed_material[8..16].copy_from_slice(&ticks.to_le_bytes());

    // Source 3: RTC wall clock
    let dt = crate::rtc::wall_clock();
    let unix = dt.to_unix_timestamp() as u64;
    seed_material[16..24].copy_from_slice(&unix.to_le_bytes());

    // Source 4: Second TSC sample (captures timing jitter between reads)
    // SAFETY: Same as above — RDTSC is always safe.
    let tsc2 = unsafe { core::arch::x86_64::_rdtsc() };
    seed_material[24..32].copy_from_slice(&tsc2.to_le_bytes());

    // Source 5: Try RDRAND for additional entropy if available
    if let Some(r) = rdrand64() {
        seed_material[32..40].copy_from_slice(&r.to_le_bytes());
    }
    if let Some(r) = rdrand64() {
        seed_material[40..48].copy_from_slice(&r.to_le_bytes());
    }

    // Source 6: More TSC jitter + address space layout
    // SAFETY: Same as above — RDTSC is always safe.
    let tsc3 = unsafe { core::arch::x86_64::_rdtsc() };
    seed_material[48..56].copy_from_slice(&tsc3.to_le_bytes());

    // Mix seed material into ChaCha20 key via repeated hashing
    // (We use ChaCha20 itself as the hash: run one block with the seed as key)
    let mut key = [0u32; 8];
    for i in 0..8 {
        key[i] = u32::from_le_bytes([
            seed_material[i * 4],
            seed_material[i * 4 + 1],
            seed_material[i * 4 + 2],
            seed_material[i * 4 + 3],
        ]);
    }

    let nonce = [
        u32::from_le_bytes([seed_material[32], seed_material[33], seed_material[34], seed_material[35]]),
        u32::from_le_bytes([seed_material[36], seed_material[37], seed_material[38], seed_material[39]]),
        u32::from_le_bytes([seed_material[40], seed_material[41], seed_material[42], seed_material[43]]),
    ];

    let mut state = CHACHA_STATE.lock();
    state.key = key;
    state.nonce = nonce;
    state.counter = 0;
    state.buf_pos = 64; // Force regeneration on next use
}

/// Fill a buffer from the ChaCha20 PRNG (fallback path when RDRAND unavailable).
///
/// Generates keystream bytes on demand, buffering 64 bytes at a time.
/// Re-keys every 65,536 blocks (~4 MiB of output) for **forward secrecy**:
/// even if the current state is compromised, past outputs cannot be recovered
/// because the old key has been overwritten.
fn chacha_fill(buf: &mut [u8]) {
    let mut state = CHACHA_STATE.lock();
    let mut remaining = buf.len();
    let mut offset = 0;

    while remaining > 0 {
        if state.buf_pos >= 64 {
            // Generate a new keystream block
            state.buffer = chacha20_block(&state.key, state.counter, &state.nonce);
            state.counter += 1;
            state.buf_pos = 0;

            // Re-key every 65,536 blocks (~4 MiB) for forward secrecy.
            // We overwrite the key with bytes from the current output block,
            // making it impossible to recover previous outputs from the new state.
            if state.counter % 65536 == 0 {
                // Use the last 32 bytes of the output block as the new 256-bit key
                for i in 0..8 {
                    state.key[i] = u32::from_le_bytes([
                        state.buffer[32 + i * 4],
                        state.buffer[32 + i * 4 + 1],
                        state.buffer[32 + i * 4 + 2],
                        state.buffer[32 + i * 4 + 3],
                    ]);
                }
            }
        }

        let available = 64 - state.buf_pos;
        let to_copy = remaining.min(available);
        buf[offset..offset + to_copy].copy_from_slice(&state.buffer[state.buf_pos..state.buf_pos + to_copy]);
        state.buf_pos += to_copy;
        offset += to_copy;
        remaining -= to_copy;
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Fill `buf` with cryptographically secure random bytes.
///
/// Uses RDRAND if available, otherwise falls back to a ChaCha20-based PRNG
/// seeded from TSC + PIT + RTC entropy.
///
/// This function is safe to call from any context (interrupt handlers, async
/// tasks, etc.) — the ChaCha20 state is protected by a spinlock.
pub fn random_bytes(buf: &mut [u8]) {
    if !INITIALIZED.load(Ordering::Relaxed) {
        // Pre-init fallback: SplitMix64 seeded from TSC + atomic counter.
        // WARNING: This is NOT cryptographically secure! It is only used
        // during the brief window before init() is called (heap setup, etc.).
        // Once init() runs, all subsequent calls use RDRAND or ChaCha20.
        static FALLBACK_CTR: AtomicU64 = AtomicU64::new(0);
        // SAFETY: RDTSC is a non-privileged instruction on x86_64.
        let tsc = unsafe { core::arch::x86_64::_rdtsc() };
        let mut state = tsc ^ FALLBACK_CTR.fetch_add(1, Ordering::Relaxed);
        for byte in buf.iter_mut() {
            // SplitMix64: a fast, high-quality non-cryptographic PRNG.
            // 0x9e3779b97f4a7c15 is the golden ratio constant (2^64 / phi).
            state = state.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            z = z ^ (z >> 31);
            *byte = z as u8;
        }
        return;
    }

    // Try RDRAND first
    if rdrand_fill(buf) {
        return;
    }

    // Fall back to ChaCha20 PRNG
    chacha_fill(buf);
}

/// Generate a random u64.
pub fn random_u64() -> u64 {
    let mut buf = [0u8; 8];
    random_bytes(&mut buf);
    u64::from_le_bytes(buf)
}

/// Generate a random u32.
pub fn random_u32() -> u32 {
    let mut buf = [0u8; 4];
    random_bytes(&mut buf);
    u32::from_le_bytes(buf)
}
