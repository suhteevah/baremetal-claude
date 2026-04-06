//! Text generation loop for LLM inference.
//!
//! Drives the full pipeline: tokenize prompt, run transformer forward passes,
//! sample output tokens, decode back to text.

use alloc::string::String;
use alloc::vec::Vec;

use crate::sampler::{sample, Rng, SamplerConfig};
use crate::tokenizer::Tokenizer;
use crate::transformer::{RunState, TransformerModel};
use crate::ModelConfig;

/// Generate text from a prompt using the given model.
///
/// Tokenizes the prompt, processes it through the transformer, then
/// autoregressively samples new tokens until EOS or `max_tokens` is reached.
pub fn generate(
    model: &TransformerModel,
    tokenizer: &Tokenizer,
    prompt: &str,
    max_tokens: usize,
    config: &ModelConfig,
) -> Result<String, String> {
    // 1. Tokenize prompt
    let prompt_tokens = tokenizer.encode(prompt, true); // add BOS
    if prompt_tokens.is_empty() {
        return Err("empty prompt after tokenization".into());
    }

    log::info!("[llm] prompt: {} tokens", prompt_tokens.len());

    // 2. Initialize state
    let mut state = model.new_run_state();
    let mut rng = Rng::new(42);
    let sampler_config = SamplerConfig {
        temperature: config.temperature,
        top_k: config.top_k,
        top_p: config.top_p,
        ..Default::default()
    };

    // 3. Generation loop
    let mut output_tokens: Vec<u32> = Vec::new();
    let mut all_tokens: Vec<u32> = Vec::new();
    let mut pos = 0usize;
    let mut token = prompt_tokens[0];

    let total_len = prompt_tokens.len() + max_tokens;

    for step in 0..total_len {
        // Forward pass: compute logits for current token at current position
        model.forward(&mut state, token, pos);
        pos += 1;

        if step < prompt_tokens.len() - 1 {
            // Still processing prompt -- next token is known
            token = prompt_tokens[step + 1];
        } else {
            // Generating -- sample from logits
            token = sample(&mut state.logits, &sampler_config, &mut rng, &all_tokens);

            // Check for EOS
            if tokenizer.is_eos(token) {
                log::info!("[llm] EOS at position {}", pos);
                break;
            }

            output_tokens.push(token);

            // Log progress every 10 tokens
            if output_tokens.len() % 10 == 0 {
                log::info!("[llm] generated {} tokens...", output_tokens.len());
            }
        }

        all_tokens.push(token);

        // Check max sequence length
        if pos >= model.max_seq_len {
            log::warn!("[llm] reached max sequence length {}", model.max_seq_len);
            break;
        }
    }

    // 4. Decode output tokens to text
    let text = tokenizer.decode(&output_tokens);
    log::info!(
        "[llm] generated {} tokens -> {} chars",
        output_tokens.len(),
        text.len()
    );

    Ok(text)
}
