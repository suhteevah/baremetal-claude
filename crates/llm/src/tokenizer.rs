//! BPE tokenizer for LLM inference.
//!
//! Loads vocabulary and merge priorities from GGUF model metadata, then
//! provides SentencePiece-style BPE encoding and decoding. Designed for
//! LLaMA-family models but works with any GGUF tokenizer that uses the
//! `tokenizer.ggml.*` metadata keys.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::gguf::{GgufFile, GgufValue};

/// Token type classification from GGUF metadata.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TokenType {
    /// Normal vocabulary token.
    Normal,
    /// Unknown/fallback token.
    Unknown,
    /// Control token (BOS, EOS, etc.).
    Control,
    /// User-defined token.
    UserDefined,
    /// Unused vocabulary slot.
    Unused,
    /// Byte fallback token (e.g. `<0x41>`).
    Byte,
}

impl TokenType {
    /// Convert from the GGUF integer token type.
    fn from_i32(v: i32) -> Self {
        match v {
            1 => TokenType::Normal,
            2 => TokenType::Unknown,
            3 => TokenType::Control,
            4 => TokenType::UserDefined,
            5 => TokenType::Unused,
            6 => TokenType::Byte,
            _ => TokenType::Normal,
        }
    }
}

/// A BPE tokenizer loaded from GGUF model metadata.
///
/// Supports SentencePiece-style encoding used by LLaMA and similar models.
/// The vocabulary and merge scores are read directly from the GGUF file's
/// metadata section.
pub struct Tokenizer {
    /// Token ID -> string piece.
    vocab: Vec<String>,
    /// Token scores for BPE merge priority (higher = merge first).
    scores: Vec<f32>,
    /// Token types (normal, control, byte, etc.).
    token_types: Vec<TokenType>,
    /// String piece -> Token ID (for encoding).
    piece_to_id: BTreeMap<String, u32>,
    /// Beginning-of-sequence token ID.
    pub bos_id: u32,
    /// End-of-sequence token ID.
    pub eos_id: u32,
}

impl Tokenizer {
    /// Build a tokenizer from GGUF metadata.
    ///
    /// Reads the following metadata keys:
    /// - `tokenizer.ggml.tokens` — vocabulary strings
    /// - `tokenizer.ggml.scores` — merge priority scores
    /// - `tokenizer.ggml.token_type` — token type classifications
    /// - `tokenizer.ggml.bos_token_id` — beginning-of-sequence ID
    /// - `tokenizer.ggml.eos_token_id` — end-of-sequence ID
    pub fn from_gguf(gguf: &GgufFile) -> Result<Self, String> {
        // Get vocab tokens array
        let tokens_val = gguf
            .metadata
            .get("tokenizer.ggml.tokens")
            .ok_or_else(|| String::from("missing tokenizer.ggml.tokens"))?;
        let token_strings: Vec<String> = match tokens_val {
            GgufValue::Array(arr) => arr
                .iter()
                .map(|v| match v {
                    GgufValue::String(s) => s.clone(),
                    _ => String::new(),
                })
                .collect(),
            _ => return Err(String::from("tokenizer.ggml.tokens not an array")),
        };

        let vocab_len = token_strings.len();
        log::info!("tokenizer: loaded {} vocab entries", vocab_len);

        // Get scores (default to 0.0 if missing)
        let scores: Vec<f32> = match gguf.metadata.get("tokenizer.ggml.scores") {
            Some(GgufValue::Array(arr)) => arr
                .iter()
                .map(|v| match v {
                    GgufValue::Float32(f) => *f,
                    _ => 0.0,
                })
                .collect(),
            _ => vec![0.0; vocab_len],
        };

        // Get token types (default to Normal if missing)
        let token_types: Vec<TokenType> = match gguf.metadata.get("tokenizer.ggml.token_type") {
            Some(GgufValue::Array(arr)) => arr
                .iter()
                .map(|v| match v {
                    GgufValue::Int32(i) => TokenType::from_i32(*i),
                    GgufValue::Uint32(u) => TokenType::from_i32(*u as i32),
                    _ => TokenType::Normal,
                })
                .collect(),
            _ => vec![TokenType::Normal; vocab_len],
        };

        // Build reverse lookup: string piece -> token ID
        let mut piece_to_id = BTreeMap::new();
        for (i, s) in token_strings.iter().enumerate() {
            piece_to_id.insert(s.clone(), i as u32);
        }

        // Get special token IDs
        let bos_id = gguf.get_u32("tokenizer.ggml.bos_token_id").unwrap_or(1);
        let eos_id = gguf.get_u32("tokenizer.ggml.eos_token_id").unwrap_or(2);

        log::info!(
            "tokenizer: bos_id={}, eos_id={}, vocab_size={}",
            bos_id,
            eos_id,
            vocab_len
        );

        Ok(Self {
            vocab: token_strings,
            scores,
            token_types,
            piece_to_id,
            bos_id,
            eos_id,
        })
    }

    /// Returns the vocabulary size.
    pub fn vocab_size(&self) -> usize {
        self.vocab.len()
    }

    /// Encode text to token IDs using SentencePiece-style BPE.
    ///
    /// The algorithm:
    /// 1. Greedily match the longest vocabulary entry at each position.
    ///    If no match, fall back to byte tokens (`<0xHH>`).
    /// 2. Iteratively merge the adjacent pair with the highest score
    ///    until no more merges are possible.
    pub fn encode(&self, text: &str, add_bos: bool) -> Vec<u32> {
        let mut tokens = Vec::new();
        if add_bos {
            tokens.push(self.bos_id);
        }

        if text.is_empty() {
            return tokens;
        }

        // Step 1: Initial tokenization — greedy longest match with byte fallback
        let mut pieces: Vec<u32> = Vec::new();
        let bytes = text.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            // Try longest match first (cap at 64 bytes to avoid quadratic blowup)
            let max_end = bytes.len().min(i + 64);
            let mut best_len = 0;
            let mut best_id = 0u32;

            for end in (i + 1..=max_end).rev() {
                if let Ok(s) = core::str::from_utf8(&bytes[i..end]) {
                    if let Some(&id) = self.piece_to_id.get(s) {
                        best_len = end - i;
                        best_id = id;
                        break;
                    }
                }
            }

            if best_len > 0 {
                pieces.push(best_id);
                i += best_len;
            } else {
                // Byte fallback: look for <0xHH> token
                let byte_token = format!("<0x{:02X}>", bytes[i]);
                if let Some(&id) = self.piece_to_id.get(&byte_token) {
                    pieces.push(id);
                } else {
                    log::warn!("tokenizer: no token for byte 0x{:02X}, skipping", bytes[i]);
                }
                i += 1;
            }
        }

        // Step 2: BPE merges — iteratively merge highest-scoring adjacent pair
        loop {
            if pieces.len() < 2 {
                break;
            }

            let mut best_score = f32::NEG_INFINITY;
            let mut best_idx = 0usize;
            let mut best_id = 0u32;

            for j in 0..pieces.len() - 1 {
                // Try merging pieces[j] + pieces[j+1]
                let left = &self.vocab[pieces[j] as usize];
                let right = &self.vocab[pieces[j + 1] as usize];
                let mut merged = String::with_capacity(left.len() + right.len());
                merged.push_str(left);
                merged.push_str(right);

                if let Some(&merge_id) = self.piece_to_id.get(&merged) {
                    let score = self.scores[merge_id as usize];
                    if score > best_score {
                        best_score = score;
                        best_idx = j;
                        best_id = merge_id;
                    }
                }
            }

            if best_score == f32::NEG_INFINITY {
                break;
            }

            // Apply the merge
            pieces[best_idx] = best_id;
            pieces.remove(best_idx + 1);
        }

        tokens.extend_from_slice(&pieces);
        tokens
    }

    /// Decode a sequence of token IDs back to text.
    ///
    /// Handles byte fallback tokens (`<0xHH>`) and SentencePiece space
    /// indicators (U+2581 `▁` -> ASCII space).
    pub fn decode(&self, tokens: &[u32]) -> String {
        let mut text = String::new();
        for &id in tokens {
            if (id as usize) >= self.vocab.len() {
                log::warn!("tokenizer: token id {} out of range, skipping", id);
                continue;
            }
            let piece = &self.vocab[id as usize];

            // Handle byte tokens: <0xHH> -> actual byte
            if piece.starts_with("<0x") && piece.ends_with('>') && piece.len() == 6 {
                if let Ok(byte_val) = u8::from_str_radix(&piece[3..5], 16) {
                    text.push(byte_val as char);
                    continue;
                }
            }

            // SentencePiece uses U+2581 (▁) as space indicator
            // Replace it with a normal ASCII space
            let replaced = piece.replace('\u{2581}', " ");
            text.push_str(&replaced);
        }
        text
    }

    /// Decode a single token ID to its string piece.
    ///
    /// Returns the raw vocabulary entry without any SentencePiece
    /// transformations. Returns `"<unk>"` for out-of-range IDs.
    pub fn decode_token(&self, id: u32) -> &str {
        if (id as usize) < self.vocab.len() {
            &self.vocab[id as usize]
        } else {
            "<unk>"
        }
    }

    /// Check if a token is the end-of-sequence token.
    pub fn is_eos(&self, id: u32) -> bool {
        id == self.eos_id
    }

    /// Get the token type for a given token ID.
    pub fn token_type(&self, id: u32) -> TokenType {
        if (id as usize) < self.token_types.len() {
            self.token_types[id as usize]
        } else {
            TokenType::Normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    /// Build a minimal test tokenizer with a small vocabulary.
    fn make_test_tokenizer() -> Tokenizer {
        let vocab = vec![
            String::from("<unk>"),    // 0
            String::from("<s>"),      // 1 (BOS)
            String::from("</s>"),     // 2 (EOS)
            String::from("h"),        // 3
            String::from("e"),        // 4
            String::from("l"),        // 5
            String::from("o"),        // 6
            String::from("he"),       // 7
            String::from("ll"),       // 8
            String::from("lo"),       // 9
            String::from("hel"),      // 10
            String::from("hello"),    // 11
            String::from("\u{2581}"), // 12 (space indicator)
            String::from("<0x41>"),   // 13 (byte fallback for 'A')
        ];

        // Higher score = merge earlier
        let scores = vec![
            0.0,  // <unk>
            0.0,  // <s>
            0.0,  // </s>
            -1.0, // h
            -1.0, // e
            -1.0, // l
            -1.0, // o
            5.0,  // he
            4.0,  // ll
            3.0,  // lo
            6.0,  // hel
            10.0, // hello
            0.0,  // space
            0.0,  // byte 0x41
        ];

        let token_types = vec![
            TokenType::Unknown, // <unk>
            TokenType::Control, // <s>
            TokenType::Control, // </s>
            TokenType::Normal,  // h
            TokenType::Normal,  // e
            TokenType::Normal,  // l
            TokenType::Normal,  // o
            TokenType::Normal,  // he
            TokenType::Normal,  // ll
            TokenType::Normal,  // lo
            TokenType::Normal,  // hel
            TokenType::Normal,  // hello
            TokenType::Normal,  // space
            TokenType::Byte,    // byte 0x41
        ];

        let mut piece_to_id = BTreeMap::new();
        for (i, s) in vocab.iter().enumerate() {
            piece_to_id.insert(s.clone(), i as u32);
        }

        Tokenizer {
            vocab,
            scores,
            token_types,
            piece_to_id,
            bos_id: 1,
            eos_id: 2,
        }
    }

    #[test]
    fn test_vocab_size() {
        let tok = make_test_tokenizer();
        assert_eq!(tok.vocab_size(), 14);
    }

    #[test]
    fn test_encode_simple() {
        let tok = make_test_tokenizer();
        // "hello" should greedily match token 11
        let tokens = tok.encode("hello", false);
        assert_eq!(tokens, vec![11]);
    }

    #[test]
    fn test_encode_with_bos() {
        let tok = make_test_tokenizer();
        let tokens = tok.encode("hello", true);
        assert_eq!(tokens[0], 1); // BOS
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn test_encode_empty() {
        let tok = make_test_tokenizer();
        let tokens = tok.encode("", false);
        assert!(tokens.is_empty());

        let tokens_bos = tok.encode("", true);
        assert_eq!(tokens_bos, vec![1]);
    }

    #[test]
    fn test_decode_simple() {
        let tok = make_test_tokenizer();
        let text = tok.decode(&[11]); // "hello"
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_decode_byte_fallback() {
        let tok = make_test_tokenizer();
        let text = tok.decode(&[13]); // <0x41> -> 'A'
        assert_eq!(text, "A");
    }

    #[test]
    fn test_decode_space_indicator() {
        let tok = make_test_tokenizer();
        // U+2581 should be replaced with space
        let text = tok.decode(&[12, 11]); // "▁" + "hello" -> " hello"
        assert_eq!(text, " hello");
    }

    #[test]
    fn test_decode_token() {
        let tok = make_test_tokenizer();
        assert_eq!(tok.decode_token(11), "hello");
        assert_eq!(tok.decode_token(0), "<unk>");
        assert_eq!(tok.decode_token(9999), "<unk>");
    }

    #[test]
    fn test_is_eos() {
        let tok = make_test_tokenizer();
        assert!(tok.is_eos(2));
        assert!(!tok.is_eos(0));
        assert!(!tok.is_eos(11));
    }

    #[test]
    fn test_token_type() {
        let tok = make_test_tokenizer();
        assert_eq!(tok.token_type(0), TokenType::Unknown);
        assert_eq!(tok.token_type(1), TokenType::Control);
        assert_eq!(tok.token_type(3), TokenType::Normal);
        assert_eq!(tok.token_type(13), TokenType::Byte);
    }

    #[test]
    fn test_roundtrip() {
        let tok = make_test_tokenizer();
        let original = "hello";
        let tokens = tok.encode(original, false);
        let decoded = tok.decode(&tokens);
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_from_gguf() {
        let mut metadata = BTreeMap::new();

        metadata.insert(
            String::from("tokenizer.ggml.tokens"),
            GgufValue::Array(vec![
                GgufValue::String(String::from("<unk>")),
                GgufValue::String(String::from("<s>")),
                GgufValue::String(String::from("</s>")),
                GgufValue::String(String::from("hello")),
            ]),
        );

        metadata.insert(
            String::from("tokenizer.ggml.scores"),
            GgufValue::Array(vec![
                GgufValue::Float32(0.0),
                GgufValue::Float32(0.0),
                GgufValue::Float32(0.0),
                GgufValue::Float32(1.0),
            ]),
        );

        metadata.insert(
            String::from("tokenizer.ggml.token_type"),
            GgufValue::Array(vec![
                GgufValue::Int32(2), // Unknown
                GgufValue::Int32(3), // Control
                GgufValue::Int32(3), // Control
                GgufValue::Int32(1), // Normal
            ]),
        );

        metadata.insert(
            String::from("tokenizer.ggml.bos_token_id"),
            GgufValue::Uint32(1),
        );

        metadata.insert(
            String::from("tokenizer.ggml.eos_token_id"),
            GgufValue::Uint32(2),
        );

        let gguf = GgufFile { metadata };
        let tok = Tokenizer::from_gguf(&gguf).unwrap();

        assert_eq!(tok.vocab_size(), 4);
        assert_eq!(tok.bos_id, 1);
        assert_eq!(tok.eos_id, 2);
        assert_eq!(tok.token_type(0), TokenType::Unknown);
        assert_eq!(tok.token_type(1), TokenType::Control);
        assert_eq!(tok.token_type(3), TokenType::Normal);
        assert_eq!(tok.decode_token(3), "hello");
    }
}
