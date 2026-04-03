//! Agent memory system — markdown file persistence + vector search.
//!
//! A sophisticated memory system modeled after Claude Code's memory approach,
//! combined with a built-in TF-IDF vector store for semantic search.
//!
//! # Markdown persistence
//!
//! Each agent gets a directory tree:
//! ```text
//! /var/claudio/agents/{name}/
//!   memory/
//!     MEMORY.md      — index file linking to all memory files
//!     user.md        — what the agent knows about the user
//!     project.md     — what the agent knows about current work
//!     feedback.md    — corrections and preferences
//!     reference.md   — external resources and links
//! ```
//!
//! Each `.md` file has YAML frontmatter:
//! ```yaml
//! ---
//! name: user
//! description: User profile and preferences
//! type: user
//! created_at: 2026-04-03T00:00:00
//! updated_at: 2026-04-03T12:34:56
//! ---
//! ```
//!
//! # Vector store integration
//!
//! Every memory entry is also inserted into the global TF-IDF vector store
//! (see `crate::vectordb`). Before each API call, the user's input is searched
//! against the vector store and the top-K relevant memories are injected into
//! the system prompt.
//!
//! # Agent tools
//!
//! - `remember(type, name, content)` — save to .md + vector store
//! - `recall(query)` — semantic search via vector store
//! - `forget(name)` — remove from .md + vector store
//!
//! # Shell commands
//!
//! - `memory search <query>` — vector search across all agents
//! - `memory dump <agent>` — show all .md files
//! - `memory stats` — vector store size, vocab size, entry count
//! - `memory list [agent]` — list memory files for agent(s)
//! - `memory clear <agent>` — clear all agent memory

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::vectordb;

// ---------------------------------------------------------------------------
// Memory types
// ---------------------------------------------------------------------------

/// The type/category of a memory file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    /// What the agent knows about the user.
    User,
    /// What the agent knows about the current project/work.
    Project,
    /// Corrections, preferences, and behavioral guidance.
    Feedback,
    /// External resources, links, documentation.
    Reference,
    /// Custom/general memory.
    Custom,
}

impl MemoryType {
    /// Parse from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "user" => MemoryType::User,
            "project" => MemoryType::Project,
            "feedback" => MemoryType::Feedback,
            "reference" => MemoryType::Reference,
            _ => MemoryType::Custom,
        }
    }

    /// Convert to string.
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::User => "user",
            MemoryType::Project => "project",
            MemoryType::Feedback => "feedback",
            MemoryType::Reference => "reference",
            MemoryType::Custom => "custom",
        }
    }

    /// Human-readable description for YAML frontmatter.
    pub fn description(&self) -> &'static str {
        match self {
            MemoryType::User => "User profile and preferences",
            MemoryType::Project => "Current project context and state",
            MemoryType::Feedback => "Corrections, preferences, and behavioral guidance",
            MemoryType::Reference => "External resources, links, and documentation",
            MemoryType::Custom => "General memory",
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryFile — a single .md memory file
// ---------------------------------------------------------------------------

/// A single markdown memory file with YAML frontmatter.
#[derive(Debug, Clone)]
pub struct MemoryFile {
    /// Filename without extension (e.g., "user", "project", "my-notes").
    pub name: String,
    /// Description of this memory file.
    pub description: String,
    /// Memory type/category.
    pub memory_type: MemoryType,
    /// ISO 8601 timestamp when first created.
    pub created_at: String,
    /// ISO 8601 timestamp of last update.
    pub updated_at: String,
    /// Markdown content (body after frontmatter).
    pub content: String,
    /// Vector store entry ID (for cross-referencing).
    pub vector_id: Option<String>,
}

impl MemoryFile {
    /// Create a new memory file.
    pub fn new(name: &str, memory_type: MemoryType, content: &str) -> Self {
        let now = current_timestamp();
        Self {
            name: String::from(name),
            description: String::from(memory_type.description()),
            memory_type,
            created_at: now.clone(),
            updated_at: now,
            content: String::from(content),
            vector_id: None,
        }
    }

    /// Update the content and timestamp.
    pub fn update_content(&mut self, content: &str) {
        self.content = String::from(content);
        self.updated_at = current_timestamp();
    }

    /// Append content to the file.
    pub fn append_content(&mut self, content: &str) {
        if !self.content.is_empty() && !self.content.ends_with('\n') {
            self.content.push('\n');
        }
        self.content.push_str(content);
        self.updated_at = current_timestamp();
    }

    /// Serialize to markdown with YAML frontmatter.
    pub fn to_markdown(&self) -> String {
        let mut md = String::with_capacity(self.content.len() + 256);
        md.push_str("---\n");
        md.push_str(&format!("name: {}\n", self.name));
        md.push_str(&format!("description: {}\n", self.description));
        md.push_str(&format!("type: {}\n", self.memory_type.as_str()));
        md.push_str(&format!("created_at: {}\n", self.created_at));
        md.push_str(&format!("updated_at: {}\n", self.updated_at));
        md.push_str("---\n\n");
        md.push_str(&self.content);
        if !self.content.ends_with('\n') {
            md.push('\n');
        }
        md
    }

    /// Parse from markdown with YAML frontmatter.
    pub fn from_markdown(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if !text.starts_with("---") {
            return Err(String::from("missing YAML frontmatter"));
        }

        // Find end of frontmatter.
        let after_first = &text[3..];
        let fm_end = after_first
            .find("---")
            .ok_or_else(|| String::from("unterminated frontmatter"))?;
        let frontmatter = &after_first[..fm_end].trim();
        let body_start = 3 + fm_end + 3; // skip both "---" markers
        let body = if body_start < text.len() {
            text[body_start..].trim()
        } else {
            ""
        };

        // Parse YAML-like frontmatter (simple key: value pairs).
        let mut name = String::new();
        let mut description = String::new();
        let mut type_str = String::from("custom");
        let mut created_at = String::new();
        let mut updated_at = String::new();

        for line in frontmatter.lines() {
            let line = line.trim();
            if let Some(idx) = line.find(':') {
                let key = line[..idx].trim();
                let value = line[idx + 1..].trim();
                match key {
                    "name" => name = String::from(value),
                    "description" => description = String::from(value),
                    "type" => type_str = String::from(value),
                    "created_at" => created_at = String::from(value),
                    "updated_at" => updated_at = String::from(value),
                    _ => {} // Ignore unknown fields.
                }
            }
        }

        let memory_type = MemoryType::from_str(&type_str);
        if name.is_empty() {
            name = String::from(memory_type.as_str());
        }
        if description.is_empty() {
            description = String::from(memory_type.description());
        }
        if created_at.is_empty() {
            created_at = current_timestamp();
        }
        if updated_at.is_empty() {
            updated_at = created_at.clone();
        }

        Ok(Self {
            name,
            description,
            memory_type,
            created_at,
            updated_at,
            content: String::from(body),
            vector_id: None,
        })
    }
}

// ---------------------------------------------------------------------------
// AgentMemory — the full memory system for one agent
// ---------------------------------------------------------------------------

/// Complete memory system for a single agent.
///
/// Manages markdown memory files and integrates with the global vector store.
#[derive(Debug, Clone)]
pub struct AgentMemory {
    /// Agent name (used as namespace).
    pub agent_name: String,
    /// Memory files indexed by name.
    files: BTreeMap<String, MemoryFile>,
    /// Legacy key-value entries (backward compatible).
    legacy_entries: BTreeMap<String, String>,
    /// Whether there are unsaved changes.
    dirty: bool,
}

impl AgentMemory {
    /// Create a new empty memory store for the given agent.
    pub fn new(agent_name: &str) -> Self {
        Self {
            agent_name: String::from(agent_name),
            files: BTreeMap::new(),
            legacy_entries: BTreeMap::new(),
            dirty: false,
        }
    }

    // ── Markdown file operations ────────────────────────────────────

    /// Save a memory file. Creates or updates the file and its vector store entry.
    pub fn save_memory(&mut self, memory_type: MemoryType, name: &str, content: &str) {
        let existing = self.files.get_mut(name);
        if let Some(file) = existing {
            file.update_content(content);
            // Update vector store entry.
            if let Some(ref vid) = file.vector_id {
                vectordb::global_store().update(vid, content);
            }
            log::info!(
                "[agent_memory] {}: updated memory file '{}'",
                self.agent_name,
                name
            );
        } else {
            let mut file = MemoryFile::new(name, memory_type, content);
            // Insert into vector store.
            let mut meta = BTreeMap::new();
            meta.insert(String::from("agent"), self.agent_name.clone());
            meta.insert(String::from("type"), String::from(memory_type.as_str()));
            meta.insert(String::from("name"), String::from(name));
            let vid = vectordb::global_store().insert(content, meta);
            file.vector_id = Some(vid);
            self.files.insert(String::from(name), file);
            log::info!(
                "[agent_memory] {}: created memory file '{}'",
                self.agent_name,
                name
            );
        }
        self.dirty = true;
    }

    /// Append content to an existing memory file, or create a new one.
    pub fn append_memory(&mut self, memory_type: MemoryType, name: &str, content: &str) {
        if self.files.contains_key(name) {
            let file = self.files.get_mut(name).unwrap();
            file.append_content(content);
            // Update vector store with full content.
            if let Some(ref vid) = file.vector_id {
                vectordb::global_store().update(vid, &file.content);
            }
        } else {
            self.save_memory(memory_type, name, content);
        }
        self.dirty = true;
    }

    /// Load a specific memory file by name.
    pub fn load_memory(&self, name: &str) -> Option<&MemoryFile> {
        self.files.get(name)
    }

    /// Load all memory files.
    pub fn load_all_memories(&self) -> Vec<&MemoryFile> {
        self.files.values().collect()
    }

    /// Remove a memory file. Also removes from vector store.
    pub fn forget(&mut self, name: &str) -> bool {
        if let Some(file) = self.files.remove(name) {
            if let Some(ref vid) = file.vector_id {
                vectordb::global_store().delete(vid);
            }
            self.dirty = true;
            log::info!(
                "[agent_memory] {}: removed memory file '{}'",
                self.agent_name,
                name
            );
            true
        } else {
            false
        }
    }

    /// Search this agent's memories using the vector store.
    pub fn recall(&self, query: &str, top_k: usize) -> Vec<(f32, String, String)> {
        let store = vectordb::global_store();
        let results = store.search(query, top_k * 3); // Over-fetch, then filter by agent.

        let mut agent_results = Vec::new();
        for (score, entry) in results {
            if entry.metadata.get("agent").map(|s| s.as_str()) == Some(&self.agent_name) {
                let name = entry
                    .metadata
                    .get("name")
                    .cloned()
                    .unwrap_or_else(|| entry.id.clone());
                agent_results.push((score, name, entry.text.clone()));
                if agent_results.len() >= top_k {
                    break;
                }
            }
        }
        agent_results
    }

    // ── Legacy key-value compatibility ──────────────────────────────

    /// Store a key-value pair (legacy API, backward compatible).
    pub fn save(&mut self, key: &str, value: &str) {
        self.legacy_entries
            .insert(String::from(key), String::from(value));
        self.dirty = true;
        log::debug!(
            "[agent_memory] {}: saved key '{}' ({} bytes)",
            self.agent_name,
            key,
            value.len()
        );
    }

    /// Retrieve a value by key (legacy API).
    pub fn load(&self, key: &str) -> Option<&str> {
        self.legacy_entries.get(key).map(|s| s.as_str())
    }

    /// List all stored keys (legacy API).
    pub fn list_keys(&self) -> Vec<&str> {
        self.legacy_entries.keys().map(|s| s.as_str()).collect()
    }

    /// Remove a legacy key.
    pub fn remove(&mut self, key: &str) -> bool {
        let existed = self.legacy_entries.remove(key).is_some();
        if existed {
            self.dirty = true;
        }
        existed
    }

    /// Clear all entries (both memory files and legacy entries).
    pub fn clear(&mut self) {
        // Remove vector store entries.
        for file in self.files.values() {
            if let Some(ref vid) = file.vector_id {
                vectordb::global_store().delete(vid);
            }
        }
        self.files.clear();
        self.legacy_entries.clear();
        self.dirty = true;
        log::info!("[agent_memory] {}: cleared all memory", self.agent_name);
    }

    /// Total number of items (memory files + legacy entries).
    pub fn len(&self) -> usize {
        self.files.len() + self.legacy_entries.len()
    }

    /// Whether all storage is empty.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.legacy_entries.is_empty()
    }

    /// Whether there are unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark as saved.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    // ── MEMORY.md index generation ──────────────────────────────────

    /// Generate the MEMORY.md index file content.
    pub fn generate_index(&self) -> String {
        let mut md = String::with_capacity(512);
        md.push_str("# Memory Index\n\n");

        if self.files.is_empty() && self.legacy_entries.is_empty() {
            md.push_str("No memories stored yet.\n");
            return md;
        }

        // Memory files section.
        if !self.files.is_empty() {
            for file in self.files.values() {
                md.push_str(&format!(
                    "- [{}](memory/{}.md) -- {}\n",
                    file.name, file.name, file.description
                ));
            }
            md.push('\n');
        }

        // Legacy key-value section.
        if !self.legacy_entries.is_empty() {
            md.push_str("## Legacy key-value entries\n\n");
            for key in self.legacy_entries.keys() {
                md.push_str(&format!("- `{}`\n", key));
            }
            md.push('\n');
        }

        md
    }

    // ── System prompt injection ─────────────────────────────────────

    /// Generate a system prompt fragment from all stored memories.
    ///
    /// Includes both markdown memories and legacy key-value pairs.
    pub fn to_system_prompt(&self) -> String {
        if self.is_empty() {
            return String::new();
        }

        let mut prompt = String::with_capacity(2048);
        prompt.push_str("<agent_memory>\n");

        // Markdown memory files.
        if !self.files.is_empty() {
            prompt.push_str("# Persistent Memory Files\n\n");
            prompt.push_str("The following memories are persisted across sessions. ");
            prompt.push_str("Use remember/recall/forget tools to manage them.\n\n");

            for file in self.files.values() {
                prompt.push_str(&format!(
                    "## {} ({})\n",
                    file.name,
                    file.memory_type.as_str()
                ));
                // Truncate very long content.
                let display = if file.content.len() > 800 {
                    format!(
                        "{}...\n[truncated, {} bytes total]",
                        &file.content[..800],
                        file.content.len()
                    )
                } else {
                    file.content.clone()
                };
                prompt.push_str(&display);
                if !display.ends_with('\n') {
                    prompt.push('\n');
                }
                prompt.push('\n');
            }
        }

        // Legacy entries.
        if !self.legacy_entries.is_empty() {
            prompt.push_str("# Legacy Key-Value Memory\n\n");
            for (key, value) in &self.legacy_entries {
                let display_value = if value.len() > 500 {
                    format!("{}... ({} bytes total)", &value[..500], value.len())
                } else {
                    value.clone()
                };
                prompt.push_str(&format!("- {}: {}\n", key, display_value));
            }
            prompt.push('\n');
        }

        prompt.push_str("</agent_memory>\n\n");
        prompt
    }

    /// Inject relevant memories for a specific query into the system prompt.
    ///
    /// Searches the vector store and includes the top matches as context.
    pub fn inject_relevant_context(&self, query: &str, top_k: usize) -> String {
        let results = self.recall(query, top_k);
        if results.is_empty() {
            return String::new();
        }

        let mut prompt = String::with_capacity(1024);
        prompt.push_str("<relevant_memories>\n");
        prompt.push_str("The following memories were retrieved by semantic search ");
        prompt.push_str("as relevant to the current conversation:\n\n");

        for (score, name, text) in &results {
            let display = if text.len() > 600 {
                format!("{}...", &text[..600])
            } else {
                text.clone()
            };
            prompt.push_str(&format!(
                "- **{}** (relevance: {:.2}): {}\n",
                name, score, display
            ));
        }

        prompt.push_str("</relevant_memories>\n\n");
        prompt
    }

    // ── Serialization ───────────────────────────────────────────────

    /// Serialize the entire memory to JSON bytes (for VFS persistence).
    ///
    /// Format includes both memory files and legacy entries.
    pub fn to_json(&self) -> Vec<u8> {
        let mut buf = String::with_capacity(4096);
        buf.push_str("{\"version\":2,\"agent\":\"");
        json_escape_into(&mut buf, &self.agent_name);
        buf.push_str("\",\"files\":{");

        let mut first = true;
        for (name, file) in &self.files {
            if !first {
                buf.push(',');
            }
            first = false;
            buf.push('"');
            json_escape_into(&mut buf, name);
            buf.push_str("\":{\"name\":\"");
            json_escape_into(&mut buf, &file.name);
            buf.push_str("\",\"description\":\"");
            json_escape_into(&mut buf, &file.description);
            buf.push_str("\",\"type\":\"");
            buf.push_str(file.memory_type.as_str());
            buf.push_str("\",\"created_at\":\"");
            json_escape_into(&mut buf, &file.created_at);
            buf.push_str("\",\"updated_at\":\"");
            json_escape_into(&mut buf, &file.updated_at);
            buf.push_str("\",\"content\":\"");
            json_escape_into(&mut buf, &file.content);
            buf.push_str("\",\"vector_id\":");
            match &file.vector_id {
                Some(vid) => {
                    buf.push('"');
                    json_escape_into(&mut buf, vid);
                    buf.push('"');
                }
                None => buf.push_str("null"),
            }
            buf.push('}');
        }

        buf.push_str("},\"legacy\":{");

        first = true;
        for (key, value) in &self.legacy_entries {
            if !first {
                buf.push(',');
            }
            first = false;
            buf.push('"');
            json_escape_into(&mut buf, key);
            buf.push_str("\":\"");
            json_escape_into(&mut buf, value);
            buf.push('"');
        }

        buf.push_str("}}");
        buf.into_bytes()
    }

    /// Deserialize from JSON bytes. Supports both v1 (flat KV) and v2 (files + legacy).
    pub fn from_json(agent_name: &str, data: &[u8]) -> Result<Self, String> {
        let text = core::str::from_utf8(data)
            .map_err(|_| String::from("memory file is not valid UTF-8"))?;
        let text = text.trim();

        if !text.starts_with('{') || !text.ends_with('}') {
            return Err(String::from("memory file is not a JSON object"));
        }

        // Check for v2 format.
        if text.contains("\"version\":2") || text.contains("\"version\": 2") {
            return Self::from_json_v2(agent_name, text);
        }

        // Legacy v1 format: flat {"key":"value",...}
        Self::from_json_v1(agent_name, text)
    }

    /// Parse v1 (legacy flat key-value format).
    fn from_json_v1(agent_name: &str, text: &str) -> Result<Self, String> {
        let inner = &text[1..text.len() - 1];
        let mut mem = AgentMemory::new(agent_name);

        if inner.trim().is_empty() {
            return Ok(mem);
        }

        let mut chars = inner.chars().peekable();

        loop {
            skip_whitespace(&mut chars);
            if chars.peek().is_none() {
                break;
            }

            let key = parse_json_string(&mut chars)
                .map_err(|e| format!("bad key: {}", e))?;

            skip_whitespace(&mut chars);
            match chars.next() {
                Some(':') => {}
                other => return Err(format!("expected ':', got {:?}", other)),
            }

            skip_whitespace(&mut chars);

            let value = if chars.peek() == Some(&'"') {
                parse_json_string(&mut chars)
                    .map_err(|e| format!("bad value for '{}': {}", key, e))?
            } else {
                let mut val = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch == ',' || ch == '}' {
                        break;
                    }
                    val.push(chars.next().unwrap());
                }
                val.trim().to_string()
            };

            mem.legacy_entries.insert(key, value);

            skip_whitespace(&mut chars);
            match chars.peek() {
                Some(&',') => {
                    chars.next();
                }
                _ => break,
            }
        }

        log::info!(
            "[agent_memory] loaded {} legacy entries for agent '{}'",
            mem.legacy_entries.len(),
            agent_name
        );

        Ok(mem)
    }

    /// Parse v2 format with memory files and legacy entries.
    fn from_json_v2(agent_name: &str, text: &str) -> Result<Self, String> {
        let mut mem = AgentMemory::new(agent_name);

        // Parse files section.
        if let Some(files_start) = text.find("\"files\":{") {
            let after = &text[files_start + 9..];
            if let Some(end) = find_matching_brace(after) {
                let files_str = &after[..end];
                // Parse each file object.
                let file_entries = parse_file_objects(files_str)?;
                for (name, file) in file_entries {
                    // Re-insert into vector store.
                    let mut file = file;
                    let mut meta = BTreeMap::new();
                    meta.insert(String::from("agent"), String::from(agent_name));
                    meta.insert(
                        String::from("type"),
                        String::from(file.memory_type.as_str()),
                    );
                    meta.insert(String::from("name"), name.clone());
                    let vid = vectordb::global_store().insert(&file.content, meta);
                    file.vector_id = Some(vid);
                    mem.files.insert(name, file);
                }
            }
        }

        // Parse legacy section.
        if let Some(legacy_start) = text.find("\"legacy\":{") {
            let after = &text[legacy_start + 10..];
            if let Some(end) = find_matching_brace(after) {
                let legacy_str = &after[..end];
                if !legacy_str.trim().is_empty() {
                    let mut chars = legacy_str.chars().peekable();
                    loop {
                        skip_whitespace(&mut chars);
                        if chars.peek().is_none() {
                            break;
                        }
                        let key = match parse_json_string(&mut chars) {
                            Ok(k) => k,
                            Err(_) => break,
                        };
                        skip_whitespace(&mut chars);
                        if chars.next() != Some(':') {
                            break;
                        }
                        skip_whitespace(&mut chars);
                        let value = match parse_json_string(&mut chars) {
                            Ok(v) => v,
                            Err(_) => break,
                        };
                        mem.legacy_entries.insert(key, value);
                        skip_whitespace(&mut chars);
                        if chars.peek() == Some(&',') {
                            chars.next();
                        }
                    }
                }
            }
        }

        log::info!(
            "[agent_memory] loaded {} files + {} legacy entries for agent '{}'",
            mem.files.len(),
            mem.legacy_entries.len(),
            agent_name
        );

        Ok(mem)
    }

    // ── VFS paths ───────────────────────────────────────────────────

    /// Return the VFS directory for this agent's memory.
    pub fn vfs_dir(&self) -> String {
        format!("/var/claudio/agents/{}/memory", self.agent_name)
    }

    /// Return the VFS path for the memory JSON file (primary persistence).
    pub fn vfs_path(&self) -> String {
        format!("/var/claudio/agents/{}/memory.json", self.agent_name)
    }

    /// Return VFS paths for all individual .md files that should be written.
    pub fn vfs_md_paths(&self) -> Vec<(String, String)> {
        let dir = self.vfs_dir();
        let mut paths = Vec::new();

        // MEMORY.md index.
        paths.push((format!("{}/MEMORY.md", dir), self.generate_index()));

        // Individual memory files.
        for file in self.files.values() {
            paths.push((
                format!("{}/{}.md", dir, file.name),
                file.to_markdown(),
            ));
        }

        paths
    }
}

// ---------------------------------------------------------------------------
// Auto-memory extraction
// ---------------------------------------------------------------------------

/// Patterns that indicate a memory should be auto-extracted from conversation.
struct ExtractionPattern {
    /// Text pattern to look for in Claude's response.
    trigger: &'static str,
    /// Memory type to assign.
    memory_type: MemoryType,
    /// Name prefix for the memory file.
    name_prefix: &'static str,
}

/// List of extraction patterns for auto-memory.
const EXTRACTION_PATTERNS: &[ExtractionPattern] = &[
    ExtractionPattern {
        trigger: "I'll remember",
        memory_type: MemoryType::Custom,
        name_prefix: "noted",
    },
    ExtractionPattern {
        trigger: "I will remember",
        memory_type: MemoryType::Custom,
        name_prefix: "noted",
    },
    ExtractionPattern {
        trigger: "I've noted",
        memory_type: MemoryType::Custom,
        name_prefix: "noted",
    },
    ExtractionPattern {
        trigger: "your preference",
        memory_type: MemoryType::Feedback,
        name_prefix: "pref",
    },
    ExtractionPattern {
        trigger: "you prefer",
        memory_type: MemoryType::Feedback,
        name_prefix: "pref",
    },
    ExtractionPattern {
        trigger: "correction noted",
        memory_type: MemoryType::Feedback,
        name_prefix: "correction",
    },
    ExtractionPattern {
        trigger: "you mentioned",
        memory_type: MemoryType::User,
        name_prefix: "context",
    },
    ExtractionPattern {
        trigger: "your name is",
        memory_type: MemoryType::User,
        name_prefix: "user",
    },
    ExtractionPattern {
        trigger: "you work on",
        memory_type: MemoryType::Project,
        name_prefix: "project",
    },
    ExtractionPattern {
        trigger: "the project",
        memory_type: MemoryType::Project,
        name_prefix: "project",
    },
];

/// Scan a Claude response for auto-extractable memories.
///
/// Returns a list of (memory_type, name, extracted_content) tuples.
pub fn auto_extract_memories(response: &str) -> Vec<(MemoryType, String, String)> {
    let mut extracted = Vec::new();
    let response_lower = response.to_lowercase();

    for pattern in EXTRACTION_PATTERNS {
        if let Some(pos) = response_lower.find(pattern.trigger) {
            // Extract the sentence containing the trigger.
            let sentence = extract_sentence(response, pos);
            if sentence.len() > 10 {
                // Minimum useful length.
                let name = format!(
                    "{}-{}",
                    pattern.name_prefix,
                    simple_hash(&sentence) % 10000
                );
                extracted.push((pattern.memory_type, name, sentence));
            }
        }
    }

    // Deduplicate by name.
    extracted.sort_by(|a, b| a.1.cmp(&b.1));
    extracted.dedup_by(|a, b| a.1 == b.1);
    extracted
}

/// Extract the sentence containing the given position.
fn extract_sentence(text: &str, pos: usize) -> String {
    // Find sentence start (walk back to period, newline, or start).
    let before = &text[..pos];
    let sent_start = before
        .rfind(|c: char| c == '.' || c == '!' || c == '?' || c == '\n')
        .map(|i| i + 1)
        .unwrap_or(0);

    // Find sentence end (walk forward to period, newline, or end).
    let after = &text[pos..];
    let sent_end = after
        .find(|c: char| c == '.' || c == '!' || c == '?' || c == '\n')
        .map(|i| pos + i + 1)
        .unwrap_or(text.len());

    let sentence = &text[sent_start..sent_end];
    sentence.trim().to_string()
}

/// Simple hash for generating unique-ish names.
fn simple_hash(s: &str) -> u32 {
    let mut h: u32 = 5381;
    for b in s.bytes() {
        h = h.wrapping_mul(33).wrapping_add(b as u32);
    }
    h
}

// ---------------------------------------------------------------------------
// Timestamp helper
// ---------------------------------------------------------------------------

/// Get a simple timestamp string. Uses the kernel's RTC if available,
/// otherwise returns a placeholder.
fn current_timestamp() -> String {
    // Try to get time from RTC module.
    // If not available, use a counter-based placeholder.
    #[cfg(not(test))]
    {
        // In the kernel, we can try the RTC. For now, use a simple counter.
        static COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        format!("2026-04-03T{:02}:{:02}:{:02}", n / 3600 % 24, n / 60 % 60, n % 60)
    }
    #[cfg(test)]
    {
        String::from("2026-04-03T00:00:00")
    }
}

// ---------------------------------------------------------------------------
// JSON helpers
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

fn skip_whitespace(chars: &mut core::iter::Peekable<core::str::Chars<'_>>) {
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
        } else {
            break;
        }
    }
}

fn parse_json_string(
    chars: &mut core::iter::Peekable<core::str::Chars<'_>>,
) -> Result<String, String> {
    match chars.next() {
        Some('"') => {}
        other => return Err(format!("expected '\"', got {:?}", other)),
    }

    let mut s = String::new();
    loop {
        match chars.next() {
            None => return Err(String::from("unterminated string")),
            Some('"') => return Ok(s),
            Some('\\') => match chars.next() {
                Some('"') => s.push('"'),
                Some('\\') => s.push('\\'),
                Some('/') => s.push('/'),
                Some('n') => s.push('\n'),
                Some('r') => s.push('\r'),
                Some('t') => s.push('\t'),
                Some('u') => {
                    let mut hex = String::with_capacity(4);
                    for _ in 0..4 {
                        match chars.next() {
                            Some(c) => hex.push(c),
                            None => return Err(String::from("truncated \\u escape")),
                        }
                    }
                    let code = u32::from_str_radix(&hex, 16)
                        .map_err(|_| format!("bad \\u escape: {}", hex))?;
                    if let Some(c) = char::from_u32(code) {
                        s.push(c);
                    }
                }
                other => {
                    s.push('\\');
                    if let Some(c) = other {
                        s.push(c);
                    }
                }
            },
            Some(c) => s.push(c),
        }
    }
}

/// Find the matching closing brace for text starting after '{'.
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

/// Parse file objects from the "files" section of v2 JSON.
fn parse_file_objects(text: &str) -> Result<Vec<(String, MemoryFile)>, String> {
    let mut results = Vec::new();
    let mut chars = text.chars().peekable();

    loop {
        skip_whitespace(&mut chars);
        if chars.peek().is_none() {
            break;
        }

        // Parse file name key.
        let name = match parse_json_string(&mut chars) {
            Ok(n) => n,
            Err(_) => break,
        };

        skip_whitespace(&mut chars);
        if chars.next() != Some(':') {
            break;
        }
        skip_whitespace(&mut chars);
        if chars.next() != Some('{') {
            break;
        }

        // Read the object body until matching '}'.
        let mut depth = 1i32;
        let mut obj_str = String::from("{");
        let mut in_str = false;
        let mut esc = false;
        while let Some(ch) = chars.next() {
            obj_str.push(ch);
            if esc {
                esc = false;
                continue;
            }
            if ch == '\\' && in_str {
                esc = true;
                continue;
            }
            if ch == '"' {
                in_str = !in_str;
                continue;
            }
            if in_str {
                continue;
            }
            if ch == '{' {
                depth += 1;
            } else if ch == '}' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
        }

        // Parse the file object fields.
        let file_name = extract_json_field(&obj_str, "name").unwrap_or_else(|| name.clone());
        let description = extract_json_field(&obj_str, "description")
            .unwrap_or_else(|| String::from("Memory file"));
        let type_str = extract_json_field(&obj_str, "type").unwrap_or_else(|| String::from("custom"));
        let created_at =
            extract_json_field(&obj_str, "created_at").unwrap_or_else(current_timestamp);
        let updated_at =
            extract_json_field(&obj_str, "updated_at").unwrap_or_else(current_timestamp);
        let content = extract_json_field(&obj_str, "content").unwrap_or_default();
        let vector_id = extract_json_field(&obj_str, "vector_id");

        let file = MemoryFile {
            name: file_name,
            description,
            memory_type: MemoryType::from_str(&type_str),
            created_at,
            updated_at,
            content,
            vector_id,
        };

        results.push((name, file));

        skip_whitespace(&mut chars);
        if chars.peek() == Some(&',') {
            chars.next();
        }
    }

    Ok(results)
}

/// Extract a JSON string field value from an object string.
fn extract_json_field(json: &str, key: &str) -> Option<String> {
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

// ---------------------------------------------------------------------------
// Global memory store
// ---------------------------------------------------------------------------

/// Global store of all agent memories.
///
/// SAFETY: Single-threaded kernel -- no concurrent access.
static mut MEMORY_STORE: Option<BTreeMap<String, AgentMemory>> = None;

fn store() -> &'static mut BTreeMap<String, AgentMemory> {
    unsafe {
        let ptr = core::ptr::addr_of_mut!(MEMORY_STORE);
        if (*ptr).is_none() {
            *ptr = Some(BTreeMap::new());
        }
        (*ptr).as_mut().unwrap()
    }
}

/// Get or create an agent's memory.
pub fn get_memory(agent_name: &str) -> &'static mut AgentMemory {
    let store = store();
    if !store.contains_key(agent_name) {
        store.insert(String::from(agent_name), AgentMemory::new(agent_name));
    }
    store.get_mut(agent_name).unwrap()
}

/// Load an agent's memory from VFS data.
pub fn load_from_data(agent_name: &str, data: &[u8]) -> Result<(), String> {
    let mem = AgentMemory::from_json(agent_name, data)?;
    store().insert(String::from(agent_name), mem);
    Ok(())
}

/// Get the serialized JSON for an agent's memory.
pub fn serialize_memory(agent_name: &str) -> Option<Vec<u8>> {
    store().get(agent_name).map(|m| m.to_json())
}

/// Check if an agent has unsaved changes.
pub fn is_dirty(agent_name: &str) -> bool {
    store()
        .get(agent_name)
        .map(|m| m.is_dirty())
        .unwrap_or(false)
}

/// Mark an agent's memory as saved.
pub fn mark_clean(agent_name: &str) {
    if let Some(m) = store().get_mut(agent_name) {
        m.mark_clean();
    }
}

/// List all agents that have memory stored.
pub fn list_agents() -> Vec<String> {
    store().keys().cloned().collect()
}

// ---------------------------------------------------------------------------
// Agent tool handlers
// ---------------------------------------------------------------------------

/// Execute the `remember` tool: save content to both .md and vector store.
///
/// Input: type (user/project/feedback/reference/custom), name, content
/// Returns confirmation string.
pub fn tool_remember(agent_name: &str, memory_type: &str, name: &str, content: &str) -> String {
    let mt = MemoryType::from_str(memory_type);
    let mem = get_memory(agent_name);
    mem.save_memory(mt, name, content);
    format!(
        "Saved memory '{}' (type: {}, {} bytes) to persistent storage and vector index.",
        name,
        mt.as_str(),
        content.len()
    )
}

/// Execute the `recall` tool: semantic search via vector store.
///
/// Input: query text, optional top_k (default 5)
/// Returns matching memories ranked by relevance.
pub fn tool_recall(agent_name: &str, query: &str, top_k: usize) -> String {
    let mem = get_memory(agent_name);
    let results = mem.recall(query, if top_k == 0 { 5 } else { top_k });

    if results.is_empty() {
        return String::from("No relevant memories found for that query.");
    }

    let mut output = format!("{} relevant memories found:\n\n", results.len());
    for (i, (score, name, text)) in results.iter().enumerate() {
        let preview = if text.len() > 200 {
            format!("{}...", &text[..200])
        } else {
            text.clone()
        };
        output.push_str(&format!(
            "{}. [{}] (relevance: {:.2})\n   {}\n\n",
            i + 1,
            name,
            score,
            preview
        ));
    }
    output
}

/// Execute the `forget` tool: remove from .md and vector store.
///
/// Input: name of the memory to remove
/// Returns confirmation or error.
pub fn tool_forget(agent_name: &str, name: &str) -> String {
    let mem = get_memory(agent_name);
    if mem.forget(name) {
        format!("Removed memory '{}' from persistent storage and vector index.", name)
    } else {
        format!("Memory '{}' not found.", name)
    }
}

// ── Legacy tool handlers (backward compatible) ──────────────────────

/// Execute the `save_memory` tool: store a key-value pair (legacy).
pub fn tool_save_memory(agent_name: &str, key: &str, value: &str) -> String {
    let mem = get_memory(agent_name);
    mem.save(key, value);
    format!("Saved '{}' ({} bytes) to agent memory.", key, value.len())
}

/// Execute the `load_memory` tool: retrieve a value (legacy).
pub fn tool_load_memory(agent_name: &str, key: &str) -> String {
    let mem = get_memory(agent_name);
    match mem.load(key) {
        Some(value) => value.to_string(),
        None => format!("Key '{}' not found in agent memory.", key),
    }
}

/// Execute the `list_memories` tool: list all memories.
pub fn tool_list_memories(agent_name: &str) -> String {
    let mem = get_memory(agent_name);

    let mut output = String::new();

    // Memory files.
    let files = mem.load_all_memories();
    if !files.is_empty() {
        output.push_str(&format!("{} memory files:\n", files.len()));
        for file in &files {
            let preview = if file.content.len() > 80 {
                format!("{}...", &file.content[..80])
            } else {
                file.content.clone()
            };
            output.push_str(&format!(
                "  {} ({}) = {}\n",
                file.name,
                file.memory_type.as_str(),
                preview
            ));
        }
    }

    // Legacy entries.
    let keys = mem.list_keys();
    if !keys.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&format!("{} legacy keys:\n", keys.len()));
        for key in keys {
            let value_preview = mem
                .load(key)
                .map(|v| {
                    if v.len() > 80 {
                        format!("{}...", &v[..80])
                    } else {
                        v.to_string()
                    }
                })
                .unwrap_or_default();
            output.push_str(&format!("  {} = {}\n", key, value_preview));
        }
    }

    if output.is_empty() {
        String::from("Agent memory is empty.")
    } else {
        output
    }
}

// ---------------------------------------------------------------------------
// Shell command handler
// ---------------------------------------------------------------------------

/// Execute a `memory` shell command.
///
/// Commands:
/// - `memory list [agent]` — list memories
/// - `memory clear <agent>` — clear all memory
/// - `memory get <agent> <key>` — get a legacy value
/// - `memory set <agent> <key> <value>` — set a legacy value
/// - `memory search <query>` — vector search across all agents
/// - `memory dump <agent>` — show all .md files
/// - `memory stats` — vector store statistics
/// - `memory remember <agent> <type> <name> <content>` — save memory
/// - `memory recall <agent> <query>` — semantic search for agent
/// - `memory forget <agent> <name>` — remove memory
pub fn execute_memory_command(args: &[&str]) -> String {
    if args.is_empty() {
        return String::from(
            "usage: memory <command> [args...]\n\
             \n\
             Commands:\n\
             \x20 memory list                          — list all agents with memory\n\
             \x20 memory list <agent>                   — list memories for an agent\n\
             \x20 memory clear <agent>                  — clear all agent memory\n\
             \x20 memory get <agent> <key>              — get a legacy value\n\
             \x20 memory set <agent> <key> <value>      — set a legacy value\n\
             \x20 memory search <query>                 — vector search across all agents\n\
             \x20 memory dump <agent>                   — show all .md memory files\n\
             \x20 memory stats                          — vector store statistics\n\
             \x20 memory remember <agent> <type> <name> <content>\n\
             \x20                                       — save structured memory\n\
             \x20 memory recall <agent> <query>         — semantic search for agent\n\
             \x20 memory forget <agent> <name>          — remove a memory\n",
        );
    }

    match args[0] {
        "list" => {
            if args.len() > 1 {
                let agent = args[1];
                tool_list_memories(agent)
            } else {
                let agents = list_agents();
                if agents.is_empty() {
                    String::from("No agent memories stored.\n")
                } else {
                    let mut output = format!("{} agent(s) with memory:\n", agents.len());
                    for name in &agents {
                        let mem = get_memory(name);
                        output.push_str(&format!(
                            "  {} -- {} files, {} legacy keys\n",
                            name,
                            mem.files.len(),
                            mem.legacy_entries.len()
                        ));
                    }
                    output
                }
            }
        }

        "clear" => {
            if args.len() < 2 {
                return String::from("usage: memory clear <agent>\n");
            }
            let agent = args[1];
            let mem = get_memory(agent);
            let count = mem.len();
            mem.clear();
            format!("Cleared {} entries from agent '{}' memory.\n", count, agent)
        }

        "get" => {
            if args.len() < 3 {
                return String::from("usage: memory get <agent> <key>\n");
            }
            let agent = args[1];
            let key = args[2];
            match get_memory(agent).load(key) {
                Some(val) => format!("{}\n", val),
                None => format!("Key '{}' not found for agent '{}'.\n", key, agent),
            }
        }

        "set" => {
            if args.len() < 4 {
                return String::from("usage: memory set <agent> <key> <value>\n");
            }
            let agent = args[1];
            let key = args[2];
            let value = args[3..].join(" ");
            get_memory(agent).save(key, &value);
            format!("Saved '{}' for agent '{}'.\n", key, agent)
        }

        "search" => {
            if args.len() < 2 {
                return String::from("usage: memory search <query>\n");
            }
            let query = args[1..].join(" ");
            let store = vectordb::global_store();
            let results = store.search(&query, 10);

            if results.is_empty() {
                return format!("No results for '{}'.\n", query);
            }

            let mut output = format!(
                "Vector search: {} results for '{}':\n\n",
                results.len(),
                query
            );
            for (i, (score, entry)) in results.iter().enumerate() {
                let agent = entry
                    .metadata
                    .get("agent")
                    .map(|s| s.as_str())
                    .unwrap_or("unknown");
                let name = entry
                    .metadata
                    .get("name")
                    .map(|s| s.as_str())
                    .unwrap_or(&entry.id);
                let preview = if entry.text.len() > 120 {
                    format!("{}...", &entry.text[..120])
                } else {
                    entry.text.clone()
                };
                output.push_str(&format!(
                    "  {}. [{}/{}] (score: {:.3})\n     {}\n\n",
                    i + 1,
                    agent,
                    name,
                    score,
                    preview
                ));
            }
            output
        }

        "dump" => {
            if args.len() < 2 {
                return String::from("usage: memory dump <agent>\n");
            }
            let agent = args[1];
            let mem = get_memory(agent);
            let files = mem.load_all_memories();

            if files.is_empty() {
                return format!("Agent '{}' has no memory files.\n", agent);
            }

            let mut output = format!(
                "Agent '{}' -- {} memory files:\n\n",
                agent,
                files.len()
            );

            // MEMORY.md index.
            output.push_str("=== MEMORY.md ===\n");
            output.push_str(&mem.generate_index());
            output.push('\n');

            // Individual files.
            for file in files {
                output.push_str(&format!("=== {}.md ===\n", file.name));
                output.push_str(&file.to_markdown());
                output.push('\n');
            }

            output
        }

        "stats" => {
            let store = vectordb::global_store();
            let agents = list_agents();
            let mut total_files = 0usize;
            let mut total_legacy = 0usize;
            for name in &agents {
                let mem = get_memory(name);
                total_files += mem.files.len();
                total_legacy += mem.legacy_entries.len();
            }

            format!(
                "Memory System Statistics:\n\
                 \n\
                 Agents:            {}\n\
                 Memory files:      {}\n\
                 Legacy KV entries: {}\n\
                 \n\
                 Vector Store:\n\
                 \x20 Entries:     {}\n\
                 \x20 Vocabulary:  {} words\n\
                 \x20 Index size:  ~{} bytes\n",
                agents.len(),
                total_files,
                total_legacy,
                store.len(),
                store.vocab_size(),
                store.len() * store.vocab_size() * 4, // rough estimate: entries * dims * f32
            )
        }

        "remember" => {
            if args.len() < 5 {
                return String::from(
                    "usage: memory remember <agent> <type> <name> <content...>\n\
                     types: user, project, feedback, reference, custom\n",
                );
            }
            let agent = args[1];
            let mem_type = args[2];
            let name = args[3];
            let content = args[4..].join(" ");
            tool_remember(agent, mem_type, name, &content)
        }

        "recall" => {
            if args.len() < 3 {
                return String::from("usage: memory recall <agent> <query...>\n");
            }
            let agent = args[1];
            let query = args[2..].join(" ");
            tool_recall(agent, &query, 5)
        }

        "forget" => {
            if args.len() < 3 {
                return String::from("usage: memory forget <agent> <name>\n");
            }
            let agent = args[1];
            let name = args[2];
            tool_forget(agent, name)
        }

        _ => format!("memory: unknown subcommand '{}'\n", args[0]),
    }
}

// ---------------------------------------------------------------------------
// Auto-save / auto-load helpers
// ---------------------------------------------------------------------------

/// Generate the system prompt fragment for an agent's memory.
///
/// Called when building the system prompt for an API request.
pub fn system_prompt_for_agent(agent_name: &str) -> String {
    let mem = get_memory(agent_name);
    mem.to_system_prompt()
}

/// Generate context-aware system prompt injection based on user query.
///
/// Searches vector store and returns relevant memories for the query.
pub fn context_for_query(agent_name: &str, query: &str) -> String {
    let mem = get_memory(agent_name);
    mem.inject_relevant_context(query, 5)
}

/// Process a Claude response for auto-memory extraction.
///
/// Call this after each API response to automatically store relevant facts.
pub fn process_response_for_memories(agent_name: &str, response: &str) {
    let extracted = auto_extract_memories(response);
    if extracted.is_empty() {
        return;
    }

    let mem = get_memory(agent_name);
    for (memory_type, name, content) in extracted {
        // Only auto-save if we don't already have this memory.
        if mem.load_memory(&name).is_none() {
            mem.save_memory(memory_type, &name, &content);
            log::debug!(
                "[agent_memory] auto-extracted '{}' for agent '{}'",
                name,
                agent_name
            );
        }
    }
}

/// Get all agent names and their memory file paths, for VFS persistence.
pub fn persistence_list() -> Vec<(String, String)> {
    store()
        .iter()
        .filter(|(_, mem)| !mem.is_empty())
        .map(|(name, mem)| (name.clone(), mem.vfs_path()))
        .collect()
}

/// Get all .md file paths that should be written for persistence.
pub fn persistence_md_list() -> Vec<(String, String)> {
    let mut all_paths = Vec::new();
    for mem in store().values() {
        if !mem.is_empty() {
            all_paths.extend(mem.vfs_md_paths());
        }
    }
    all_paths
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_file_markdown_roundtrip() {
        let file = MemoryFile::new("user", MemoryType::User, "Matt Gates, systems Rust dev");
        let md = file.to_markdown();
        assert!(md.contains("---"));
        assert!(md.contains("name: user"));
        assert!(md.contains("type: user"));
        assert!(md.contains("Matt Gates"));

        let parsed = MemoryFile::from_markdown(&md).unwrap();
        assert_eq!(parsed.name, "user");
        assert_eq!(parsed.memory_type, MemoryType::User);
        assert!(parsed.content.contains("Matt Gates"));
    }

    #[test]
    fn test_memory_type_roundtrip() {
        for mt in &[
            MemoryType::User,
            MemoryType::Project,
            MemoryType::Feedback,
            MemoryType::Reference,
            MemoryType::Custom,
        ] {
            assert_eq!(MemoryType::from_str(mt.as_str()), *mt);
        }
    }

    #[test]
    fn test_agent_memory_save_load() {
        let mut mem = AgentMemory::new("test-agent");
        mem.save_memory(MemoryType::User, "user", "prefers concise output");
        mem.save_memory(MemoryType::Project, "project", "building ClaudioOS");

        assert_eq!(mem.files.len(), 2);
        let user = mem.load_memory("user").unwrap();
        assert!(user.content.contains("concise"));

        let project = mem.load_memory("project").unwrap();
        assert!(project.content.contains("ClaudioOS"));
    }

    #[test]
    fn test_agent_memory_forget() {
        let mut mem = AgentMemory::new("test-agent");
        mem.save_memory(MemoryType::User, "user", "some info");
        assert!(mem.forget("user"));
        assert!(!mem.forget("user")); // Already gone.
        assert!(mem.load_memory("user").is_none());
    }

    #[test]
    fn test_legacy_compatibility() {
        let mut mem = AgentMemory::new("test");
        mem.save("old_key", "old_value");
        assert_eq!(mem.load("old_key"), Some("old_value"));
        assert_eq!(mem.list_keys(), vec!["old_key"]);
    }

    #[test]
    fn test_v1_json_roundtrip() {
        // v1 format: flat key-value.
        let json = b"{\"key1\":\"value1\",\"key2\":\"value2\"}";
        let mem = AgentMemory::from_json("test", json).unwrap();
        assert_eq!(mem.load("key1"), Some("value1"));
        assert_eq!(mem.load("key2"), Some("value2"));
    }

    #[test]
    fn test_index_generation() {
        let mut mem = AgentMemory::new("test");
        mem.save_memory(MemoryType::User, "user", "Matt");
        mem.save_memory(MemoryType::Project, "project", "ClaudioOS");

        let index = mem.generate_index();
        assert!(index.contains("Memory Index"));
        assert!(index.contains("user.md"));
        assert!(index.contains("project.md"));
    }

    #[test]
    fn test_system_prompt_generation() {
        let mut mem = AgentMemory::new("test");
        mem.save_memory(MemoryType::Feedback, "feedback", "be concise");
        mem.save("legacy_key", "legacy_val");

        let prompt = mem.to_system_prompt();
        assert!(prompt.contains("<agent_memory>"));
        assert!(prompt.contains("be concise"));
        assert!(prompt.contains("legacy_key"));
        assert!(prompt.contains("</agent_memory>"));
    }

    #[test]
    fn test_auto_extract_memories() {
        let response = "I'll remember that you prefer Rust over Python.";
        let extracted = auto_extract_memories(response);
        assert!(!extracted.is_empty());
        assert!(extracted[0].2.contains("prefer Rust"));
    }

    #[test]
    fn test_auto_extract_no_match() {
        let response = "Here is the code you requested.";
        let extracted = auto_extract_memories(response);
        assert!(extracted.is_empty());
    }

    #[test]
    fn test_append_memory() {
        let mut mem = AgentMemory::new("test");
        mem.save_memory(MemoryType::Feedback, "notes", "First note.");
        mem.append_memory(MemoryType::Feedback, "notes", "Second note.");

        let file = mem.load_memory("notes").unwrap();
        assert!(file.content.contains("First note."));
        assert!(file.content.contains("Second note."));
    }

    #[test]
    fn test_empty_system_prompt() {
        let mem = AgentMemory::new("empty");
        assert_eq!(mem.to_system_prompt(), "");
    }

    #[test]
    fn test_extract_sentence() {
        let text = "Hello world. I'll remember that fact. Thanks!";
        let sentence = extract_sentence(text, text.find("remember").unwrap());
        assert!(sentence.contains("remember"));
        assert!(!sentence.contains("Hello"));
    }
}
