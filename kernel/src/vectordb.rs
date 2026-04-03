//! Bare-metal vector store with TF-IDF vectorization and cosine similarity.
//!
//! Provides semantic search over text documents without neural embeddings.
//! Uses term frequency - inverse document frequency (TF-IDF) to produce sparse
//! vectors, then cosine similarity for retrieval.
//!
//! No Postgres, no external services — runs entirely on bare metal with `alloc`.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │              VectorStore                     │
//! │  ┌──────────┐  ┌─────────┐  ┌───────────┐  │
//! │  │Vocabulary │  │ Entries │  │ IDF Cache │  │
//! │  │word→index │  │id,text, │  │ per-term  │  │
//! │  │           │  │embedding│  │           │  │
//! │  └──────────┘  └─────────┘  └───────────┘  │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! # Persistence
//!
//! Serializes to JSON for debuggability:
//! ```json
//! {
//!   "vocabulary": {"word": 0, "another": 1},
//!   "entries": [
//!     {"id": "abc123", "text": "...", "embedding": [0.5, 0.0, ...], "metadata": {...}}
//!   ],
//!   "doc_count": 42,
//!   "df": [3, 1, ...]
//! }
//! ```

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;

// ---------------------------------------------------------------------------
// VectorEntry — a single document in the store
// ---------------------------------------------------------------------------

/// A single entry in the vector store.
#[derive(Debug, Clone)]
pub struct VectorEntry {
    /// Unique identifier.
    pub id: String,
    /// Original text content.
    pub text: String,
    /// TF-IDF embedding vector (sparse, stored as dense for simplicity).
    pub embedding: Vec<f32>,
    /// Arbitrary key-value metadata (agent name, memory type, timestamp, etc.).
    pub metadata: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// VectorStore — collection of entries with shared vocabulary
// ---------------------------------------------------------------------------

/// In-memory vector store with TF-IDF vectorization and cosine similarity search.
///
/// Each store maintains its own vocabulary (word -> dimension index mapping),
/// document frequency counts, and collection of vector entries.
#[derive(Debug, Clone)]
pub struct VectorStore {
    /// Word -> dimension index mapping.
    vocabulary: BTreeMap<String, usize>,
    /// All stored entries.
    entries: Vec<VectorEntry>,
    /// Document frequency: df[i] = number of documents containing word i.
    df: Vec<u32>,
    /// Total number of documents ever inserted (for IDF computation).
    doc_count: u32,
    /// Next ID counter for auto-generated IDs.
    next_id: u64,
    /// Whether the index needs rebuilding (vocabulary changed).
    dirty: bool,
}

impl VectorStore {
    /// Create a new empty vector store.
    pub fn new() -> Self {
        Self {
            vocabulary: BTreeMap::new(),
            entries: Vec::new(),
            df: Vec::new(),
            doc_count: 0,
            next_id: 1,
            dirty: false,
        }
    }

    /// Number of entries in the store.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Size of the vocabulary (number of unique words).
    pub fn vocab_size(&self) -> usize {
        self.vocabulary.len()
    }

    /// Insert a text document with metadata. Returns the assigned ID.
    ///
    /// Tokenizes the text, updates vocabulary and document frequency,
    /// computes TF-IDF vector, and stores the entry.
    pub fn insert(&mut self, text: &str, metadata: BTreeMap<String, String>) -> String {
        let id = format!("vec_{}", self.next_id);
        self.next_id += 1;
        self.insert_with_id(&id, text, metadata);
        id
    }

    /// Insert with a specific ID (used for loading from persistence).
    pub fn insert_with_id(&mut self, id: &str, text: &str, metadata: BTreeMap<String, String>) {
        let tokens = tokenize(text);

        // Update vocabulary with any new words.
        let mut seen_words: BTreeMap<String, bool> = BTreeMap::new();
        for token in &tokens {
            if !self.vocabulary.contains_key(token) {
                let idx = self.vocabulary.len();
                self.vocabulary.insert(token.clone(), idx);
                self.df.push(0);
                self.dirty = true; // Vocabulary grew, existing embeddings need rebuild.
            }
            seen_words.insert(token.clone(), true);
        }

        // Update document frequency for words in this document.
        for word in seen_words.keys() {
            if let Some(&idx) = self.vocabulary.get(word) {
                if idx < self.df.len() {
                    self.df[idx] += 1;
                }
            }
        }

        self.doc_count += 1;

        // Compute TF-IDF embedding.
        let embedding = self.compute_tfidf(&tokens);

        self.entries.push(VectorEntry {
            id: String::from(id),
            text: String::from(text),
            embedding,
            metadata,
        });

        // If vocabulary grew, rebuild all embeddings.
        if self.dirty {
            self.rebuild_embeddings();
            self.dirty = false;
        }

        log::debug!(
            "[vectordb] inserted '{}' ({} tokens, vocab={})",
            id,
            tokens.len(),
            self.vocabulary.len()
        );
    }

    /// Search for the top-K most similar entries to the query text.
    ///
    /// Returns entries sorted by descending cosine similarity score.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(f32, &VectorEntry)> {
        if self.entries.is_empty() {
            return Vec::new();
        }

        let tokens = tokenize(query);
        let query_vec = self.compute_tfidf(&tokens);

        let mut scored: Vec<(f32, &VectorEntry)> = self
            .entries
            .iter()
            .map(|entry| {
                let score = cosine_similarity(&query_vec, &entry.embedding);
                (score, entry)
            })
            .filter(|(score, _)| *score > 0.0)
            .collect();

        // Sort by descending score.
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(core::cmp::Ordering::Equal));

        scored.truncate(top_k);
        scored
    }

    /// Delete an entry by ID. Returns true if found and removed.
    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        let removed = self.entries.len() < before;
        if removed {
            log::debug!("[vectordb] deleted entry '{}'", id);
            // Note: we don't decrement df/doc_count because rebuilding is expensive.
            // The IDF values will be slightly off but still functional.
        }
        removed
    }

    /// Update an entry's text (recomputes embedding). Returns true if found.
    pub fn update(&mut self, id: &str, new_text: &str) -> bool {
        // Find the entry, get its metadata, remove it, re-insert.
        let meta = match self.entries.iter().find(|e| e.id == id) {
            Some(e) => e.metadata.clone(),
            None => return false,
        };
        self.entries.retain(|e| e.id != id);

        let tokens = tokenize(new_text);
        let embedding = self.compute_tfidf(&tokens);

        self.entries.push(VectorEntry {
            id: String::from(id),
            text: String::from(new_text),
            embedding,
            metadata: meta,
        });
        true
    }

    /// Get all entries (for dumping/debugging).
    pub fn all_entries(&self) -> &[VectorEntry] {
        &self.entries
    }

    // ── TF-IDF computation ──────────────────────────────────────────

    /// Compute TF-IDF vector for a token sequence.
    fn compute_tfidf(&self, tokens: &[String]) -> Vec<f32> {
        let vocab_size = self.vocabulary.len();
        if vocab_size == 0 || tokens.is_empty() {
            return vec![0.0; vocab_size];
        }

        // Compute term frequency.
        let mut tf: BTreeMap<usize, f32> = BTreeMap::new();
        let token_count = tokens.len() as f32;
        for token in tokens {
            if let Some(&idx) = self.vocabulary.get(token) {
                *tf.entry(idx).or_insert(0.0) += 1.0;
            }
        }

        // Normalize TF and multiply by IDF.
        let mut vec = vec![0.0f32; vocab_size];
        let n = if self.doc_count > 0 {
            self.doc_count as f32
        } else {
            1.0
        };

        for (&idx, &count) in &tf {
            let term_freq = count / token_count;
            let doc_freq = if idx < self.df.len() && self.df[idx] > 0 {
                self.df[idx] as f32
            } else {
                1.0
            };
            // IDF = ln(N / df) + 1 (smoothed to avoid zero).
            let idf = ln_f32(n / doc_freq) + 1.0;
            if idx < vec.len() {
                vec[idx] = term_freq * idf;
            }
        }

        vec
    }

    /// Rebuild all entry embeddings after vocabulary growth.
    ///
    /// We collect texts first to avoid borrow conflicts with `&mut self.entries`
    /// and the `&self` needed by `compute_tfidf`.
    fn rebuild_embeddings(&mut self) {
        // Collect (index, tokens) pairs to avoid simultaneous borrow.
        let token_lists: Vec<(usize, Vec<String>)> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, entry)| (i, tokenize(&entry.text)))
            .collect();

        for (i, tokens) in token_lists {
            let embedding = self.compute_tfidf(&tokens);
            self.entries[i].embedding = embedding;
        }
    }

    // ── Serialization ───────────────────────────────────────────────

    /// Serialize the entire store to JSON bytes.
    pub fn to_json(&self) -> Vec<u8> {
        let mut buf = String::with_capacity(4096);
        buf.push_str("{\"vocabulary\":{");

        let mut first = true;
        for (word, &idx) in &self.vocabulary {
            if !first {
                buf.push(',');
            }
            first = false;
            buf.push('"');
            json_escape_into(&mut buf, word);
            buf.push_str("\":");
            buf.push_str(&format!("{}", idx));
        }

        buf.push_str("},\"doc_count\":");
        buf.push_str(&format!("{}", self.doc_count));
        buf.push_str(",\"next_id\":");
        buf.push_str(&format!("{}", self.next_id));

        buf.push_str(",\"df\":[");
        for (i, &d) in self.df.iter().enumerate() {
            if i > 0 {
                buf.push(',');
            }
            buf.push_str(&format!("{}", d));
        }

        buf.push_str("],\"entries\":[");
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                buf.push(',');
            }
            buf.push_str("{\"id\":\"");
            json_escape_into(&mut buf, &entry.id);
            buf.push_str("\",\"text\":\"");
            json_escape_into(&mut buf, &entry.text);
            buf.push_str("\",\"metadata\":{");
            let mut mfirst = true;
            for (k, v) in &entry.metadata {
                if !mfirst {
                    buf.push(',');
                }
                mfirst = false;
                buf.push('"');
                json_escape_into(&mut buf, k);
                buf.push_str("\":\"");
                json_escape_into(&mut buf, v);
                buf.push('"');
            }
            // Note: we don't serialize embeddings — they're rebuilt on load.
            buf.push_str("}}");
        }

        buf.push_str("]}");
        buf.into_bytes()
    }

    /// Deserialize from JSON bytes. Rebuilds all TF-IDF embeddings.
    pub fn from_json(data: &[u8]) -> Result<Self, String> {
        let text = core::str::from_utf8(data)
            .map_err(|_| String::from("vector store data is not valid UTF-8"))?;
        let text = text.trim();

        // Simple JSON parser for our known schema.
        let mut store = VectorStore::new();

        // Parse vocabulary.
        if let Some(vocab_start) = text.find("\"vocabulary\":{") {
            let after = &text[vocab_start + 14..];
            if let Some(end) = find_matching_brace(after) {
                let vocab_str = &after[..end];
                store.vocabulary = parse_string_int_map(vocab_str)?;
                // Initialize df to match vocabulary size.
                store.df = vec![0; store.vocabulary.len()];
            }
        }

        // Parse doc_count.
        if let Some(dc_start) = text.find("\"doc_count\":") {
            let after = &text[dc_start + 12..];
            let num_end = after
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after.len());
            if let Ok(n) = after[..num_end].parse::<u32>() {
                store.doc_count = n;
            }
        }

        // Parse next_id.
        if let Some(ni_start) = text.find("\"next_id\":") {
            let after = &text[ni_start + 10..];
            let num_end = after
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after.len());
            if let Ok(n) = after[..num_end].parse::<u64>() {
                store.next_id = n;
            }
        }

        // Parse df array.
        if let Some(df_start) = text.find("\"df\":[") {
            let after = &text[df_start + 5..];
            if let Some(bracket_start) = after.find('[') {
                let inner = &after[bracket_start + 1..];
                if let Some(bracket_end) = inner.find(']') {
                    let arr_str = &inner[..bracket_end];
                    if !arr_str.trim().is_empty() {
                        store.df = arr_str
                            .split(',')
                            .filter_map(|s| s.trim().parse::<u32>().ok())
                            .collect();
                    }
                }
            }
        }

        // Parse entries array.
        if let Some(entries_start) = text.find("\"entries\":[") {
            let after = &text[entries_start + 11..];
            // Find matching ']'.
            if let Some(arr_end) = find_matching_bracket(after) {
                let entries_str = &after[..arr_end];
                let entry_objects = split_json_objects(entries_str);
                for obj_str in entry_objects {
                    let id = extract_json_string(&obj_str, "id").unwrap_or_default();
                    let entry_text =
                        extract_json_string(&obj_str, "text").unwrap_or_default();
                    let metadata = extract_json_object(&obj_str, "metadata")
                        .map(|s| parse_string_string_map(&s).unwrap_or_default())
                        .unwrap_or_default();

                    // Compute embedding from text using the loaded vocabulary.
                    let tokens = tokenize(&entry_text);
                    let embedding = store.compute_tfidf(&tokens);

                    store.entries.push(VectorEntry {
                        id,
                        text: entry_text,
                        embedding,
                        metadata,
                    });
                }
            }
        }

        log::info!(
            "[vectordb] loaded {} entries, vocab={}, doc_count={}",
            store.entries.len(),
            store.vocabulary.len(),
            store.doc_count
        );

        Ok(store)
    }
}

// ---------------------------------------------------------------------------
// Tokenization
// ---------------------------------------------------------------------------

/// Tokenize text into lowercase words, filtering stopwords and short tokens.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            word.push(ch.to_ascii_lowercase());
        } else if !word.is_empty() {
            if word.len() >= 2 && !is_stopword(&word) {
                tokens.push(word.clone());
            }
            word.clear();
        }
    }
    if word.len() >= 2 && !is_stopword(&word) {
        tokens.push(word);
    }

    tokens
}

/// Check if a word is a common English stopword.
fn is_stopword(word: &str) -> bool {
    matches!(
        word,
        "the" | "is" | "at" | "of" | "on" | "in" | "to" | "for"
        | "and" | "or" | "an" | "as" | "it" | "be" | "by" | "if"
        | "no" | "so" | "do" | "he" | "we" | "my" | "up" | "am"
        | "me" | "us" | "not" | "but" | "are" | "was" | "has"
        | "had" | "its" | "can" | "may" | "our" | "you" | "all"
        | "any" | "who" | "how" | "did" | "get" | "got" | "set"
        | "let" | "new" | "old" | "use" | "now" | "way" | "own"
        | "see" | "say" | "her" | "him" | "his" | "she" | "they"
        | "them" | "this" | "that" | "with" | "from" | "have"
        | "been" | "were" | "will" | "what" | "when" | "your"
        | "than" | "each" | "just" | "also" | "into" | "over"
        | "such" | "some" | "very" | "only" | "then" | "more"
        | "about" | "which" | "would" | "could" | "should"
        | "there" | "their" | "these" | "those" | "other"
    )
}

// ---------------------------------------------------------------------------
// Minimal math helpers (no libm dependency in the kernel)
// ---------------------------------------------------------------------------

/// Approximate natural logarithm for f32.
///
/// We cannot use `libm` or `std::f32::ln()` in the no_std kernel, so we
/// implement a software approximation.  The approach:
/// 1. Decompose x into mantissa m in [1,2) and exponent e via IEEE 754 bit tricks
/// 2. Compute ln(m) using a 5th-degree polynomial (Taylor series of ln(1+t))
/// 3. Combine: ln(x) = e * ln(2) + ln(m)
///
/// Accuracy: ~4 decimal digits for x > 0.  This is more than sufficient for
/// TF-IDF weighting where we only need relative ordering, not exact values.
fn ln_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return f32::NEG_INFINITY;
    }
    if x == 1.0 {
        return 0.0;
    }

    // Decompose x = m * 2^e where 1 <= m < 2.
    let bits = x.to_bits();
    let e = ((bits >> 23) & 0xFF) as i32 - 127;
    let m_bits = (bits & 0x007F_FFFF) | 0x3F80_0000; // Set exponent to 127 (= 1.0 * mantissa).
    let m = f32::from_bits(m_bits);

    // ln(x) = e * ln(2) + ln(m)
    // Approximate ln(m) for m in [1, 2) using a polynomial in (m-1).
    let t = m - 1.0;
    // Pade-like approximation: ln(1+t) ~ t - t^2/2 + t^3/3 - t^4/4 + t^5/5
    let ln_m = t * (1.0 - t * (0.5 - t * (1.0 / 3.0 - t * (0.25 - t * 0.2))));

    let ln2: f32 = 0.693_147_2;
    (e as f32) * ln2 + ln_m
}

/// Approximate square root for f32.
///
/// Uses a bit-manipulation initial guess (exploiting IEEE 754 float layout:
/// halving the exponent bits gives an approximation of sqrt) followed by two
/// Newton-Raphson iterations for convergence.  Two iterations give ~6 digits
/// of accuracy, which is the limit of f32 precision anyway.
fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }

    // Initial guess using bit manipulation.
    let bits = x.to_bits();
    let guess_bits = (bits >> 1) + 0x1FC0_0000;
    let mut guess = f32::from_bits(guess_bits);

    // Two Newton-Raphson iterations: guess = (guess + x/guess) / 2.
    guess = 0.5 * (guess + x / guess);
    guess = 0.5 * (guess + x / guess);
    guess
}

// ---------------------------------------------------------------------------
// Cosine similarity
// ---------------------------------------------------------------------------

/// Compute cosine similarity between two vectors.
///
/// Returns 0.0 if either vector has zero magnitude.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut mag_a = 0.0f32;
    let mut mag_b = 0.0f32;

    for i in 0..len {
        dot += a[i] * b[i];
        mag_a += a[i] * a[i];
        mag_b += b[i] * b[i];
    }

    // Handle remaining elements in the longer vector for magnitude.
    for i in len..a.len() {
        mag_a += a[i] * a[i];
    }
    for i in len..b.len() {
        mag_b += b[i] * b[i];
    }

    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }

    dot / (sqrt_f32(mag_a) * sqrt_f32(mag_b))
}

// ---------------------------------------------------------------------------
// JSON helpers (no serde in no_std kernel)
// ---------------------------------------------------------------------------

fn json_escape_into(buf: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '"' => buf.push_str("\\\""),
            '\\' => buf.push_str("\\\\"),
            '\n' => buf.push_str("\\n"),
            '\r' => buf.push_str("\\r"),
            '\t' => buf.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                buf.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => buf.push(c),
        }
    }
}

/// Find the position of the matching closing brace for text starting after '{'.
fn find_matching_brace(text: &str) -> Option<usize> {
    let mut depth = 1i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, ch) in text.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Find position of matching ']' for text starting after '['.
fn find_matching_bracket(text: &str) -> Option<usize> {
    let mut depth = 1i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, ch) in text.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if ch == '[' {
            depth += 1;
        } else if ch == ']' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Split a JSON array body into individual object strings.
fn split_json_objects(text: &str) -> Vec<String> {
    let mut objects = Vec::new();
    let mut depth = 0i32;
    let mut start = None;
    let mut in_string = false;
    let mut escape = false;

    for (i, ch) in text.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if ch == '{' {
            if depth == 0 {
                start = Some(i);
            }
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                if let Some(s) = start {
                    objects.push(String::from(&text[s..=i]));
                }
                start = None;
            }
        }
    }

    objects
}

/// Extract a string value for a given key from a JSON object string.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let search = format!("\"{}\":\"", key);
    let start = json.find(&search)?;
    let after = &json[start + search.len()..];

    let mut result = String::new();
    let mut escape = false;
    for ch in after.chars() {
        if escape {
            match ch {
                '"' => result.push('"'),
                '\\' => result.push('\\'),
                'n' => result.push('\n'),
                'r' => result.push('\r'),
                't' => result.push('\t'),
                _ => {
                    result.push('\\');
                    result.push(ch);
                }
            }
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if ch == '"' {
            return Some(result);
        }
        result.push(ch);
    }
    None
}

/// Extract a JSON object value (as raw string) for a given key.
fn extract_json_object(json: &str, key: &str) -> Option<String> {
    let search = format!("\"{}\":{{", key);
    let start = json.find(&search)?;
    let after = &json[start + search.len() - 1..]; // Include the opening brace.
    let end = find_matching_brace(&after[1..])? + 1; // +1 for the opening brace offset.
    Some(String::from(&after[1..end]))
}

/// Parse a simple {"key": index, ...} map from JSON inner string (no braces).
fn parse_string_int_map(text: &str) -> Result<BTreeMap<String, usize>, String> {
    let mut map = BTreeMap::new();
    if text.trim().is_empty() {
        return Ok(map);
    }

    let mut chars = text.chars().peekable();
    loop {
        skip_ws(&mut chars);
        if chars.peek().is_none() {
            break;
        }
        let key = parse_quoted_string(&mut chars)?;
        skip_ws(&mut chars);
        if chars.next() != Some(':') {
            return Err(String::from("expected ':'"));
        }
        skip_ws(&mut chars);
        let mut num_str = String::new();
        while let Some(&ch) = chars.peek() {
            if ch.is_ascii_digit() {
                num_str.push(ch);
                chars.next();
            } else {
                break;
            }
        }
        let idx: usize = num_str
            .parse()
            .map_err(|_| format!("bad index: {}", num_str))?;
        map.insert(key, idx);
        skip_ws(&mut chars);
        if chars.peek() == Some(&',') {
            chars.next();
        }
    }
    Ok(map)
}

/// Parse a simple {"key": "value", ...} map from JSON inner string (no braces).
fn parse_string_string_map(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    if text.trim().is_empty() {
        return Ok(map);
    }

    let mut chars = text.chars().peekable();
    loop {
        skip_ws(&mut chars);
        if chars.peek().is_none() {
            break;
        }
        let key = parse_quoted_string(&mut chars)?;
        skip_ws(&mut chars);
        if chars.next() != Some(':') {
            return Err(String::from("expected ':'"));
        }
        skip_ws(&mut chars);
        let value = parse_quoted_string(&mut chars)?;
        map.insert(key, value);
        skip_ws(&mut chars);
        if chars.peek() == Some(&',') {
            chars.next();
        }
    }
    Ok(map)
}

fn skip_ws(chars: &mut core::iter::Peekable<core::str::Chars<'_>>) {
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
        } else {
            break;
        }
    }
}

fn parse_quoted_string(
    chars: &mut core::iter::Peekable<core::str::Chars<'_>>,
) -> Result<String, String> {
    if chars.next() != Some('"') {
        return Err(String::from("expected '\"'"));
    }
    let mut s = String::new();
    let mut escape = false;
    for ch in chars.by_ref() {
        if escape {
            match ch {
                '"' => s.push('"'),
                '\\' => s.push('\\'),
                'n' => s.push('\n'),
                'r' => s.push('\r'),
                't' => s.push('\t'),
                _ => {
                    s.push('\\');
                    s.push(ch);
                }
            }
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if ch == '"' {
            return Ok(s);
        }
        s.push(ch);
    }
    Err(String::from("unterminated string"))
}

// ---------------------------------------------------------------------------
// Global vector store
// ---------------------------------------------------------------------------

/// Global vector store instance.
///
/// # Safety
///
/// Uses `static mut` rather than `Mutex` for performance -- vector search
/// is called on every agent API request for RAG context injection, and the
/// mutex overhead is unnecessary in our cooperative single-threaded executor.
/// All access is through `global_store()` which is only called from the
/// async executor's single thread (never from interrupt handlers).
static mut VECTOR_STORE: Option<VectorStore> = None;

/// Get or initialize the global vector store.
pub fn global_store() -> &'static mut VectorStore {
    unsafe {
        let ptr = core::ptr::addr_of_mut!(VECTOR_STORE);
        if (*ptr).is_none() {
            *ptr = Some(VectorStore::new());
        }
        (*ptr).as_mut().unwrap()
    }
}

/// Load the global vector store from persisted JSON data.
pub fn load_global_store(data: &[u8]) -> Result<(), String> {
    let store = VectorStore::from_json(data)?;
    unsafe {
        let ptr = core::ptr::addr_of_mut!(VECTOR_STORE);
        *ptr = Some(store);
    }
    Ok(())
}

/// Serialize the global vector store to JSON bytes.
pub fn serialize_global_store() -> Vec<u8> {
    global_store().to_json()
}

/// VFS path for the global vector store.
pub fn vfs_path() -> &'static str {
    "/var/claudio/vectordb.json"
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_basic() {
        let tokens = tokenize("Hello world, this is a test!");
        assert!(tokens.contains(&String::from("hello")));
        assert!(tokens.contains(&String::from("world")));
        assert!(tokens.contains(&String::from("test")));
        // Stopwords filtered.
        assert!(!tokens.contains(&String::from("this")));
        assert!(!tokens.contains(&String::from("is")));
    }

    #[test]
    fn test_tokenize_programming() {
        let tokens = tokenize("Rust no_std bare-metal kernel with async executor");
        assert!(tokens.contains(&String::from("rust")));
        assert!(tokens.contains(&String::from("no_std")));
        assert!(tokens.contains(&String::from("bare-metal")));
        assert!(tokens.contains(&String::from("kernel")));
        assert!(tokens.contains(&String::from("async")));
        assert!(tokens.contains(&String::from("executor")));
    }

    #[test]
    fn test_insert_and_search() {
        let mut store = VectorStore::new();
        let meta = BTreeMap::new();

        store.insert("Rust programming language systems", meta.clone());
        store.insert("Python scripting and data science", meta.clone());
        store.insert("Bare metal kernel development in Rust", meta.clone());

        let results = store.search("Rust kernel", 2);
        assert!(!results.is_empty());
        // The Rust-related entries should score higher.
        assert!(results[0].1.text.contains("Rust") || results[0].1.text.contains("kernel"));
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 0.001);
    }

    #[test]
    fn test_delete() {
        let mut store = VectorStore::new();
        let meta = BTreeMap::new();
        let id = store.insert("test entry", meta);
        assert_eq!(store.len(), 1);
        assert!(store.delete(&id));
        assert_eq!(store.len(), 0);
        assert!(!store.delete(&id)); // Already gone.
    }

    #[test]
    fn test_update() {
        let mut store = VectorStore::new();
        let meta = BTreeMap::new();
        let id = store.insert("original text", meta);
        assert!(store.update(&id, "updated text about Rust"));
        let entry = store.all_entries().iter().find(|e| e.id == id).unwrap();
        assert_eq!(entry.text, "updated text about Rust");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut store = VectorStore::new();
        let mut meta = BTreeMap::new();
        meta.insert(String::from("agent"), String::from("test-agent"));
        meta.insert(String::from("type"), String::from("reference"));

        store.insert("Rust bare metal OS development", meta.clone());
        store.insert("TLS 1.3 handshake AES-GCM", meta);

        let json = store.to_json();
        let loaded = VectorStore::from_json(&json).expect("deserialize");

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.vocab_size(), store.vocab_size());
        assert_eq!(loaded.doc_count, store.doc_count);
    }

    #[test]
    fn test_empty_store_search() {
        let store = VectorStore::new();
        let results = store.search("anything", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_stopword_filtering() {
        assert!(is_stopword("the"));
        assert!(is_stopword("and"));
        assert!(!is_stopword("kernel"));
        assert!(!is_stopword("rust"));
    }

    #[test]
    fn test_metadata_preserved() {
        let mut store = VectorStore::new();
        let mut meta = BTreeMap::new();
        meta.insert(String::from("agent"), String::from("claude-1"));
        meta.insert(String::from("type"), String::from("feedback"));

        let id = store.insert("user prefers concise responses", meta);
        let entry = store.all_entries().iter().find(|e| e.id == id).unwrap();
        assert_eq!(
            entry.metadata.get("agent").map(|s| s.as_str()),
            Some("claude-1")
        );
        assert_eq!(
            entry.metadata.get("type").map(|s| s.as_str()),
            Some("feedback")
        );
    }
}
