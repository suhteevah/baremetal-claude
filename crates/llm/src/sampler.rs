//! Token sampling strategies for LLM inference.
//!
//! Supports temperature scaling, top-k filtering, top-p (nucleus) sampling,
//! repetition penalty, and greedy decoding. All `no_std` compatible.

use alloc::vec::Vec;

/// Configuration for token sampling from logits.
pub struct SamplerConfig {
    pub temperature: f32,
    pub top_k: usize,
    pub top_p: f32,
    pub repeat_penalty: f32,
    pub seed: u64,
}

impl Default for SamplerConfig {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            top_k: 40,
            top_p: 0.9,
            repeat_penalty: 1.1,
            seed: 42,
        }
    }
}

/// Simple PRNG for sampling (xorshift64).
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    /// Generate next random u64.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Generate random f32 in [0, 1).
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Sample a token from logits using the given configuration.
///
/// Applies repeat penalty, temperature scaling, top-k filtering,
/// softmax, top-p (nucleus) filtering, and multinomial sampling.
pub fn sample(
    logits: &mut [f32],
    config: &SamplerConfig,
    rng: &mut Rng,
    recent_tokens: &[u32],
) -> u32 {
    // 1. Apply repeat penalty — discourage recently generated tokens
    for &tok in recent_tokens {
        if (tok as usize) < logits.len() {
            if logits[tok as usize] > 0.0 {
                logits[tok as usize] /= config.repeat_penalty;
            } else {
                logits[tok as usize] *= config.repeat_penalty;
            }
        }
    }

    // 2. Temperature = 0 -> greedy (argmax)
    if config.temperature <= 0.0 || config.temperature < 1e-6 {
        return argmax(logits) as u32;
    }

    // 3. Apply temperature scaling
    for l in logits.iter_mut() {
        *l /= config.temperature;
    }

    // 4. Top-k filtering: keep only top_k highest-scoring candidates
    let n = logits.len();
    let mut indices: Vec<(usize, f32)> = (0..n).map(|i| (i, logits[i])).collect();
    let k = if config.top_k > 0 && config.top_k < n {
        config.top_k
    } else {
        n
    };
    // Sort descending by logit value
    indices.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
    indices.truncate(k);

    // 5. Softmax over top-k candidates
    let max_logit = indices[0].1;
    let mut probs: Vec<f32> = indices
        .iter()
        .map(|(_, l)| libm::expf(l - max_logit))
        .collect();
    let sum: f32 = probs.iter().sum();
    for p in probs.iter_mut() {
        *p /= sum;
    }

    // 6. Top-p (nucleus) filtering — keep smallest set whose cumulative prob >= top_p
    if config.top_p < 1.0 {
        let mut cumsum = 0.0f32;
        let mut cutoff = probs.len();
        for (i, &p) in probs.iter().enumerate() {
            cumsum += p;
            if cumsum >= config.top_p {
                cutoff = i + 1;
                break;
            }
        }
        probs.truncate(cutoff);
        indices.truncate(cutoff);
        // Renormalize after truncation
        let sum: f32 = probs.iter().sum();
        for p in probs.iter_mut() {
            *p /= sum;
        }
    }

    // 7. Multinomial sampling from the filtered distribution
    let r = rng.next_f32();
    let mut cumsum = 0.0f32;
    for (i, &p) in probs.iter().enumerate() {
        cumsum += p;
        if r < cumsum {
            return indices[i].0 as u32;
        }
    }

    // Fallback to last candidate (floating point edge case)
    indices.last().map(|&(idx, _)| idx as u32).unwrap_or(0)
}

/// Return the index of the maximum value in the slice.
fn argmax(x: &[f32]) -> usize {
    let mut best = 0;
    let mut best_val = f32::NEG_INFINITY;
    for (i, &v) in x.iter().enumerate() {
        if v > best_val {
            best_val = v;
            best = i;
        }
    }
    best
}
