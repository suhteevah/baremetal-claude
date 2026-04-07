//! ClaudioOS LLM inference engine — run language models on bare metal.
//!
//! Provides a complete pipeline for loading and running transformer-based
//! language models directly on x86_64 hardware, no OS required:
//!
//! - **GGUF model loading**: Parse the GGUF container format, extract tensors
//! - **BPE tokenizer**: Encode text to tokens, decode tokens to text
//! - **Tensor math**: CPU-based matmul, RMSNorm, RoPE, softmax, SiLU
//! - **Transformer inference**: Attention, FFN, KV cache
//! - **Text generation**: Top-k, top-p sampling, temperature
//!
//! ## Supported Models
//!
//! Any GGUF-format model using the LLaMA architecture:
//! - LLaMA 2/3, TinyLlama, Mistral, Phi-2/3, Gemma, Qwen
//! - Quantization: Q4_0, Q4_1, Q5_0, Q5_1, Q8_0, F16, F32

#![no_std]
extern crate alloc;

use alloc::string::String;

pub mod gguf;
pub mod tensor;
pub mod tokenizer;
pub mod transformer;
pub mod sampler;
pub mod generate;

/// A fully-loaded model: parsed weights + tokenizer, ready for inference.
///
/// `from_bytes` parses a GGUF buffer once and copies all weights/vocab into
/// owned storage. The input buffer can be dropped after construction.
pub struct LoadedModel {
    pub model: transformer::TransformerModel,
    pub tokenizer: tokenizer::Tokenizer,
}

impl LoadedModel {
    /// Parse a GGUF buffer and build a runnable model + tokenizer.
    pub fn from_bytes(data: &[u8], config: &ModelConfig) -> Result<Self, String> {
        let gguf = gguf::GgufFile::parse(data)
            .map_err(|e| alloc::format!("gguf parse failed: {:?}", e))?;
        log::info!("[llm] loaded GGUF: {} tensors, arch={}", gguf.tensors.len(), gguf.architecture());
        let model = transformer::TransformerModel::from_gguf(&gguf, config)?;
        let tokenizer = tokenizer::Tokenizer::from_gguf(&gguf)?;
        Ok(Self { model, tokenizer })
    }

    /// Generate text from a prompt. Convenience wrapper over `generate::generate`.
    pub fn generate(&self, prompt: &str, max_tokens: usize, config: &ModelConfig) -> Result<String, String> {
        generate::generate(&self.model, &self.tokenizer, prompt, max_tokens, config)
    }
}

/// Configuration for model loading and inference.
pub struct ModelConfig {
    pub max_seq_len: usize,
    pub n_threads: usize,
    pub temperature: f32,
    pub top_k: usize,
    pub top_p: f32,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            max_seq_len: 2048,
            n_threads: 1,
            temperature: 0.7,
            top_k: 40,
            top_p: 0.9,
        }
    }
}
