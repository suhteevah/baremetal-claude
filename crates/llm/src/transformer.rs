//! LLaMA-style transformer inference engine.
//!
//! Implements the full forward pass for LLaMA-family models (LLaMA 2/3,
//! Mistral, TinyLlama, Phi, Qwen, Gemma). Supports grouped-query attention
//! (GQA), RoPE positional embeddings, SwiGLU FFN, and RMSNorm.
//!
//! Weights are loaded from GGUF format with automatic dequantization from
//! F16, Q4_0, and Q8_0 quantization types.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::gguf::{GgmlType, GgufFile};
use crate::ModelConfig;

/// Per-layer weights for one transformer block.
pub struct LayerWeights {
    /// Attention RMSNorm weight: [dim]
    pub rms_attn: Vec<f32>,
    /// Query projection: [dim, dim]
    pub wq: Vec<f32>,
    /// Key projection: [dim, kv_dim]
    pub wk: Vec<f32>,
    /// Value projection: [dim, kv_dim]
    pub wv: Vec<f32>,
    /// Output projection: [dim, dim]
    pub wo: Vec<f32>,
    /// FFN RMSNorm weight: [dim]
    pub rms_ffn: Vec<f32>,
    /// FFN gate projection: [dim, hidden_dim]
    pub w_gate: Vec<f32>,
    /// FFN up projection: [dim, hidden_dim]
    pub w_up: Vec<f32>,
    /// FFN down projection: [hidden_dim, dim]
    pub w_down: Vec<f32>,
}

/// Weights for the full transformer model, dequantized from GGUF data.
pub struct TransformerWeights {
    /// Token embedding table: [vocab_size, dim]
    pub token_embedding: Vec<f32>,
    /// Per-layer weights.
    pub layers: Vec<LayerWeights>,
    /// Final RMSNorm weight: [dim]
    pub rms_final: Vec<f32>,
    /// Output projection: [vocab_size, dim] (often tied to token_embedding)
    pub output: Vec<f32>,
}

/// Full transformer model with hyperparameters and loaded weights.
pub struct TransformerModel {
    /// Model embedding dimension.
    pub dim: usize,
    /// FFN hidden dimension.
    pub hidden_dim: usize,
    /// Number of transformer layers.
    pub n_layers: usize,
    /// Number of attention heads.
    pub n_heads: usize,
    /// Number of key-value heads (for GQA; equals n_heads if no GQA).
    pub n_kv_heads: usize,
    /// Dimension per attention head (dim / n_heads).
    pub head_dim: usize,
    /// Vocabulary size.
    pub vocab_size: usize,
    /// Maximum sequence length for KV cache.
    pub max_seq_len: usize,
    /// RoPE frequency base (default 10000.0).
    pub rope_theta: f32,
    /// RMSNorm epsilon (default 1e-5).
    pub rms_norm_eps: f32,
    /// Model weights.
    pub weights: TransformerWeights,
}

/// Mutable inference state for autoregressive generation.
///
/// Holds all intermediate activations and the KV cache. Allocate once
/// via `TransformerModel::new_run_state()` and reuse across tokens.
pub struct RunState {
    /// Current token activation: [dim]
    pub x: Vec<f32>,
    /// Buffer after RMSNorm: [dim]
    pub xb: Vec<f32>,
    /// Buffer after second RMSNorm: [dim]
    pub xb2: Vec<f32>,
    /// Query vector: [dim]
    pub q: Vec<f32>,
    /// Key vector: [kv_dim]
    pub k: Vec<f32>,
    /// Value vector: [kv_dim]
    pub v: Vec<f32>,
    /// Attention scores: [n_heads, max_seq_len]
    pub att: Vec<f32>,
    /// FFN hidden buffer: [hidden_dim]
    pub hb: Vec<f32>,
    /// FFN hidden buffer 2: [hidden_dim]
    pub hb2: Vec<f32>,
    /// Output logits: [vocab_size]
    pub logits: Vec<f32>,
    /// Key cache: [n_layers, max_seq_len, kv_dim]
    pub key_cache: Vec<f32>,
    /// Value cache: [n_layers, max_seq_len, kv_dim]
    pub value_cache: Vec<f32>,
}

/// Load and dequantize a named tensor from GGUF data.
fn load_tensor(gguf: &GgufFile, name: &str) -> Result<Vec<f32>, String> {
    let info = gguf
        .get_tensor(name)
        .ok_or_else(|| format!("missing tensor: {}", name))?;
    let data = gguf.tensor_data(info);
    let n_elements: usize = info.shape.iter().map(|&d| d as usize).product();
    let mut out = vec![0.0f32; n_elements];

    match info.dtype {
        GgmlType::F32 => {
            for i in 0..n_elements {
                let offset = i * 4;
                out[i] = f32::from_le_bytes([
                    data[offset],
                    data[offset + 1],
                    data[offset + 2],
                    data[offset + 3],
                ]);
            }
        }
        GgmlType::F16 => {
            crate::tensor::dequantize_f16(&mut out, data, n_elements);
        }
        GgmlType::Q4_0 => {
            crate::tensor::dequantize_q4_0(&mut out, data, n_elements);
        }
        GgmlType::Q8_0 => {
            crate::tensor::dequantize_q8_0(&mut out, data, n_elements);
        }
        _ => {
            return Err(format!(
                "unsupported quantization: {:?} for tensor {}",
                info.dtype, name
            ));
        }
    }

    Ok(out)
}

/// Attempt to load a tensor, trying alternate naming conventions.
/// Some models use slightly different tensor names.
#[allow(dead_code)]
fn load_tensor_with_fallback(gguf: &GgufFile, primary: &str, fallback: &str) -> Result<Vec<f32>, String> {
    load_tensor(gguf, primary).or_else(|_| load_tensor(gguf, fallback))
}

impl TransformerModel {
    /// Load a transformer model from a parsed GGUF file.
    ///
    /// Reads model hyperparameters from GGUF metadata and dequantizes all
    /// weight tensors to f32. The `config` parameter provides runtime
    /// settings like maximum sequence length.
    pub fn from_gguf(gguf: &GgufFile, config: &ModelConfig) -> Result<Self, String> {
        let arch = gguf.architecture();
        log::info!("loading {} model from GGUF", arch);

        // Read model hyperparameters from GGUF metadata
        let dim = gguf
            .get_u32(&format!("{}.embedding_length", arch))
            .unwrap_or(512) as usize;
        let n_layers = gguf
            .get_u32(&format!("{}.block_count", arch))
            .unwrap_or(4) as usize;
        let n_heads = gguf
            .get_u32(&format!("{}.attention.head_count", arch))
            .unwrap_or(8) as usize;
        let n_kv_heads = gguf
            .get_u32(&format!("{}.attention.head_count_kv", arch))
            .unwrap_or(n_heads as u32) as usize;
        let hidden_dim = gguf
            .get_u32(&format!("{}.feed_forward_length", arch))
            .unwrap_or((dim * 4) as u32) as usize;
        let vocab_size = gguf
            .get_u32("tokenizer.ggml.vocab_size")
            .or_else(|| gguf.get_tensor("token_embd.weight").map(|t| t.shape[0] as u32))
            .unwrap_or(32000) as usize;
        let rope_theta = gguf
            .get_f32(&format!("{}.rope.freq_base", arch))
            .unwrap_or(10000.0);
        let rms_norm_eps = gguf
            .get_f32(&format!("{}.attention.layer_norm_rms_epsilon", arch))
            .unwrap_or(1e-5);

        let head_dim = dim / n_heads;
        let max_seq_len = config.max_seq_len;

        log::info!(
            "model params: dim={}, layers={}, heads={}, kv_heads={}, hidden={}, vocab={}, seq_len={}",
            dim, n_layers, n_heads, n_kv_heads, hidden_dim, vocab_size, max_seq_len
        );

        // Load token embeddings
        let token_embedding = load_tensor(gguf, "token_embd.weight")?;
        log::debug!("loaded token_embd.weight: {} elements", token_embedding.len());

        // Load per-layer weights
        let mut layers = Vec::with_capacity(n_layers);
        for l in 0..n_layers {
            log::debug!("loading layer {}/{}", l + 1, n_layers);

            let rms_attn = load_tensor(gguf, &format!("blk.{}.attn_norm.weight", l))?;
            let wq = load_tensor(gguf, &format!("blk.{}.attn_q.weight", l))?;
            let wk = load_tensor(gguf, &format!("blk.{}.attn_k.weight", l))?;
            let wv = load_tensor(gguf, &format!("blk.{}.attn_v.weight", l))?;
            let wo = load_tensor(gguf, &format!("blk.{}.attn_output.weight", l))?;
            let rms_ffn = load_tensor(gguf, &format!("blk.{}.ffn_norm.weight", l))?;
            let w_gate = load_tensor(gguf, &format!("blk.{}.ffn_gate.weight", l))?;
            let w_up = load_tensor(gguf, &format!("blk.{}.ffn_up.weight", l))?;
            let w_down = load_tensor(gguf, &format!("blk.{}.ffn_down.weight", l))?;

            layers.push(LayerWeights {
                rms_attn,
                wq,
                wk,
                wv,
                wo,
                rms_ffn,
                w_gate,
                w_up,
                w_down,
            });
        }

        // Load final RMSNorm
        let rms_final = load_tensor(gguf, "output_norm.weight")?;

        // Load output projection -- may be tied to token embeddings
        // Many models tie the output projection to the embedding table to save params
        let output = load_tensor(gguf, "output.weight").unwrap_or_else(|_| {
            log::info!("output.weight not found, using tied token embeddings");
            token_embedding.clone()
        });

        log::info!("model loaded successfully: {} layers, {} parameters estimated",
            n_layers,
            token_embedding.len() + output.len() + rms_final.len()
                + layers.iter().map(|l| {
                    l.rms_attn.len() + l.wq.len() + l.wk.len() + l.wv.len()
                        + l.wo.len() + l.rms_ffn.len() + l.w_gate.len()
                        + l.w_up.len() + l.w_down.len()
                }).sum::<usize>()
        );

        Ok(Self {
            dim,
            hidden_dim,
            n_layers,
            n_heads,
            n_kv_heads,
            head_dim,
            vocab_size,
            max_seq_len,
            rope_theta,
            rms_norm_eps,
            weights: TransformerWeights {
                token_embedding,
                layers,
                rms_final,
                output,
            },
        })
    }

    /// Allocate a fresh `RunState` sized for this model.
    ///
    /// The KV cache is zeroed and ready for autoregressive generation
    /// from position 0.
    pub fn new_run_state(&self) -> RunState {
        let kv_dim = self.n_kv_heads * self.head_dim;
        RunState {
            x: vec![0.0; self.dim],
            xb: vec![0.0; self.dim],
            xb2: vec![0.0; self.dim],
            q: vec![0.0; self.dim],
            k: vec![0.0; kv_dim],
            v: vec![0.0; kv_dim],
            att: vec![0.0; self.n_heads * self.max_seq_len],
            hb: vec![0.0; self.hidden_dim],
            hb2: vec![0.0; self.hidden_dim],
            logits: vec![0.0; self.vocab_size],
            key_cache: vec![0.0; self.n_layers * self.max_seq_len * kv_dim],
            value_cache: vec![0.0; self.n_layers * self.max_seq_len * kv_dim],
        }
    }

    /// Run one forward pass for a single token at the given sequence position.
    ///
    /// This updates the KV cache at `pos` and writes the output logits to
    /// `state.logits`. Call repeatedly with incrementing `pos` for
    /// autoregressive generation.
    ///
    /// # Arguments
    /// * `state` - Mutable run state (activations + KV cache)
    /// * `token` - Input token ID
    /// * `pos` - Position in the sequence (0-indexed)
    pub fn forward(&self, state: &mut RunState, token: u32, pos: usize) {
        let dim = self.dim;
        let kv_dim = self.n_kv_heads * self.head_dim;
        let head_dim = self.head_dim;

        // 1. Token embedding lookup
        let emb_offset = (token as usize) * dim;
        crate::tensor::copy(
            &mut state.x,
            &self.weights.token_embedding[emb_offset..emb_offset + dim],
        );

        // 2. Process each transformer layer
        for l in 0..self.n_layers {
            let layer = &self.weights.layers[l];

            // 2a. Attention RMSNorm
            crate::tensor::rmsnorm(
                &mut state.xb,
                &state.x,
                &layer.rms_attn,
                dim,
                self.rms_norm_eps,
            );

            // 2b. QKV projections
            crate::tensor::matvec(&mut state.q, &layer.wq, &state.xb, dim, dim);
            crate::tensor::matvec(&mut state.k, &layer.wk, &state.xb, kv_dim, dim);
            crate::tensor::matvec(&mut state.v, &layer.wv, &state.xb, kv_dim, dim);

            // 2c. Apply RoPE positional embeddings to Q and K
            crate::tensor::rope(
                &mut state.q,
                &mut state.k,
                head_dim,
                pos,
                self.n_heads,
                self.rope_theta,
            );

            // 2d. Store K and V in the cache for this layer and position
            let cache_offset = l * self.max_seq_len * kv_dim + pos * kv_dim;
            state.key_cache[cache_offset..cache_offset + kv_dim].copy_from_slice(&state.k);
            state.value_cache[cache_offset..cache_offset + kv_dim].copy_from_slice(&state.v);

            // 2e. Multi-head attention with grouped-query attention (GQA)
            // GQA: multiple query heads share the same KV head to reduce memory
            let kv_mul = self.n_heads / self.n_kv_heads;
            for h in 0..self.n_heads {
                let kv_h = h / kv_mul; // map query head -> shared KV head
                let q_offset = h * head_dim;
                let q_slice = &state.q[q_offset..q_offset + head_dim];

                // Compute attention scores for this head over all positions up to `pos`
                let att_offset = h * self.max_seq_len;
                for t in 0..=pos {
                    let k_offset =
                        l * self.max_seq_len * kv_dim + t * kv_dim + kv_h * head_dim;
                    let k_slice = &state.key_cache[k_offset..k_offset + head_dim];
                    let mut score = 0.0f32;
                    for i in 0..head_dim {
                        score += q_slice[i] * k_slice[i];
                    }
                    // Scale by 1/sqrt(d_k) to stabilize gradients (Vaswani et al.)
                    state.att[att_offset + t] = score / sqrt_f32(head_dim as f32);
                }

                // Softmax over attention scores [0..=pos]
                crate::tensor::softmax(&mut state.att[att_offset..att_offset + pos + 1]);

                // Weighted sum of value vectors
                let xb_offset = h * head_dim;
                for i in 0..head_dim {
                    state.xb[xb_offset + i] = 0.0;
                }
                for t in 0..=pos {
                    let v_offset =
                        l * self.max_seq_len * kv_dim + t * kv_dim + kv_h * head_dim;
                    let a = state.att[att_offset + t];
                    for i in 0..head_dim {
                        state.xb[xb_offset + i] += a * state.value_cache[v_offset + i];
                    }
                }
            }

            // 2f. Attention output projection
            crate::tensor::matvec(&mut state.xb2, &layer.wo, &state.xb, dim, dim);

            // 2g. Residual connection (attention)
            crate::tensor::add(&mut state.x, &state.xb2);

            // 2h. FFN RMSNorm
            crate::tensor::rmsnorm(
                &mut state.xb,
                &state.x,
                &layer.rms_ffn,
                dim,
                self.rms_norm_eps,
            );

            // 2i. FFN with SwiGLU activation
            //   gate = silu(x @ w_gate)
            //   up   = x @ w_up
            //   out  = (gate * up) @ w_down
            crate::tensor::matvec(&mut state.hb, &layer.w_gate, &state.xb, self.hidden_dim, dim);
            crate::tensor::matvec(&mut state.hb2, &layer.w_up, &state.xb, self.hidden_dim, dim);
            crate::tensor::silu(&mut state.hb);
            crate::tensor::elementwise_mul(&mut state.hb, &state.hb2);
            crate::tensor::matvec(&mut state.xb, &layer.w_down, &state.hb, dim, self.hidden_dim);

            // 2j. Residual connection (FFN)
            crate::tensor::add(&mut state.x, &state.xb);
        }

        // 3. Final RMSNorm
        crate::tensor::rmsnorm(
            &mut state.xb,
            &state.x,
            &self.weights.rms_final,
            dim,
            self.rms_norm_eps,
        );

        // 4. Output projection to logits
        crate::tensor::matvec(
            &mut state.logits,
            &self.weights.output,
            &state.xb,
            self.vocab_size,
            dim,
        );
    }

    /// Sample the next token from logits using greedy (argmax) decoding.
    pub fn sample_argmax(logits: &[f32]) -> u32 {
        let mut max_idx = 0u32;
        let mut max_val = f32::NEG_INFINITY;
        for (i, &v) in logits.iter().enumerate() {
            if v > max_val {
                max_val = v;
                max_idx = i as u32;
            }
        }
        max_idx
    }

    /// Sample the next token using temperature-scaled softmax.
    ///
    /// `temperature` controls randomness: 0.0 = greedy, 1.0 = standard,
    /// higher = more random. Uses a simple linear congruential RNG seeded
    /// by `rng_state` (mutated in place).
    pub fn sample_temperature(logits: &mut [f32], temperature: f32, rng_state: &mut u64) -> u32 {
        if temperature <= 0.0 {
            return Self::sample_argmax(logits);
        }

        // Apply temperature
        let inv_temp = 1.0 / temperature;
        for v in logits.iter_mut() {
            *v *= inv_temp;
        }

        // Softmax
        crate::tensor::softmax(logits);

        // Sample from the distribution using Knuth's LCG (constants from Numerical Recipes)
        *rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        // Use upper 31 bits for better randomness (LCG lower bits have short periods)
        let r = (*rng_state >> 33) as f32 / (1u64 << 31) as f32; // uniform [0, 1)

        let mut cumulative = 0.0f32;
        for (i, &p) in logits.iter().enumerate() {
            cumulative += p;
            if cumulative > r {
                return i as u32;
            }
        }

        // Fallback to last token (rounding errors)
        (logits.len() - 1) as u32
    }

    /// Sample with top-p (nucleus) filtering.
    ///
    /// Only considers the smallest set of tokens whose cumulative probability
    /// exceeds `top_p`. Reduces incoherent outputs while maintaining diversity.
    pub fn sample_top_p(logits: &mut [f32], top_p: f32, temperature: f32, rng_state: &mut u64) -> u32 {
        if temperature <= 0.0 {
            return Self::sample_argmax(logits);
        }

        // Apply temperature
        let inv_temp = 1.0 / temperature;
        for v in logits.iter_mut() {
            *v *= inv_temp;
        }

        // Softmax
        crate::tensor::softmax(logits);

        // Sort indices by probability (descending) -- simple selection sort for no_std
        // (no alloc-heavy sort available; O(n*k) where k = tokens until top_p reached)
        let n = logits.len();
        let mut indices: Vec<usize> = (0..n).collect();

        // Partial sort: we only need tokens until cumulative > top_p, so we
        // select-sort one element at a time and check the running sum early
        for i in 0..n {
            let mut max_j = i;
            for j in (i + 1)..n {
                if logits[indices[j]] > logits[indices[max_j]] {
                    max_j = j;
                }
            }
            indices.swap(i, max_j);

            // Check if we have enough probability mass
            let mut cumsum = 0.0f32;
            for k in 0..=i {
                cumsum += logits[indices[k]];
            }
            if cumsum >= top_p {
                // Renormalize and sample from top-(i+1) tokens
                let cutoff = i + 1;
                let inv_sum = 1.0 / cumsum;

                *rng_state = rng_state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let r = (*rng_state >> 33) as f32 / (1u64 << 31) as f32;

                let mut cum = 0.0f32;
                for k in 0..cutoff {
                    cum += logits[indices[k]] * inv_sum;
                    if cum > r {
                        return indices[k] as u32;
                    }
                }
                return indices[cutoff - 1] as u32;
            }
        }

        // Fallback: sample from full distribution
        *rng_state = rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r = (*rng_state >> 33) as f32 / (1u64 << 31) as f32;

        let mut cum = 0.0f32;
        for (i, &p) in logits.iter().enumerate() {
            cum += p;
            if cum > r {
                return i as u32;
            }
        }
        (n - 1) as u32
    }
}

/// Square root approximation using Newton's method.
/// Used for attention score scaling (1/sqrt(head_dim)).
fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    // Fast inverse sqrt trick: bit-shift gives a rough estimate in ~1 cycle
    let mut guess = f32::from_bits((x.to_bits() >> 1) + 0x1FC00000);
    // 4 Newton-Raphson iterations refine to ~24-bit precision
    for _ in 0..4 {
        guess = 0.5 * (guess + x / guess);
    }
    guess
}
