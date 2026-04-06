//! Token-aware conversation compaction for long-running agent sessions.
//!
//! When a conversation grows beyond a configured token threshold, this module
//! compacts the history by replacing old messages with a structured summary
//! while preserving the N most recent messages for continuity.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as FmtWrite;

use crate::{ContentBlock, ConversationMessage, ConversationRole};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Controls when and how conversation compaction occurs.
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Maximum estimated tokens before triggering compaction.
    pub max_estimated_tokens: usize,
    /// Number of recent messages to always preserve.
    pub preserve_recent: usize,
    /// Maximum characters in the generated summary.
    pub max_summary_chars: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            max_estimated_tokens: 80_000,
            preserve_recent: 10,
            max_summary_chars: 1200,
        }
    }
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

/// Returned after a compaction pass with before/after metrics.
#[derive(Debug)]
pub struct CompactionResult {
    /// The generated summary text inserted as a system-context user message.
    pub summary: String,
    /// How many messages were removed and replaced by the summary.
    pub removed_count: usize,
    /// Estimated token count of the conversation before compaction.
    pub estimated_tokens_before: usize,
    /// Estimated token count of the conversation after compaction.
    pub estimated_tokens_after: usize,
}

// ---------------------------------------------------------------------------
// Token estimation
// ---------------------------------------------------------------------------

/// Estimate the token count for a plain text string.
///
/// Uses the simple heuristic of `chars / 4 + 1` which is a reasonable
/// approximation for English text with the Claude tokenizer.
pub fn estimate_tokens(text: &str) -> usize {
    text.len() / 4 + 1
}

/// Estimate the total token count for a single content block.
fn estimate_block_tokens(block: &ContentBlock) -> usize {
    match block {
        ContentBlock::Text { text } => estimate_tokens(text),
        ContentBlock::ToolUse { name, input, .. } => (name.len() + input.len()) / 4 + 1,
        ContentBlock::ToolResult { content, .. } => estimate_tokens(content),
    }
}

/// Estimate the total token count for a slice of messages.
fn estimate_message_tokens(messages: &[ConversationMessage]) -> usize {
    messages
        .iter()
        .map(|m| m.content.iter().map(estimate_block_tokens).sum::<usize>())
        .sum()
}

// ---------------------------------------------------------------------------
// Compaction trigger check
// ---------------------------------------------------------------------------

/// Check whether the conversation has grown large enough to warrant compaction.
pub fn should_compact(messages: &[ConversationMessage], config: &CompactionConfig) -> bool {
    let total = estimate_message_tokens(messages);
    total > config.max_estimated_tokens && messages.len() > config.preserve_recent
}

// ---------------------------------------------------------------------------
// Summary generation
// ---------------------------------------------------------------------------

/// Common source file extensions used to detect file path references.
const FILE_EXTENSIONS: &[&str] = &[
    ".rs", ".py", ".js", ".ts", ".tsx", ".jsx", ".toml", ".json", ".yaml", ".yml", ".md", ".html",
    ".css", ".c", ".h", ".cpp", ".go", ".sh",
];

/// Extract file paths from a text string by looking for tokens ending in
/// known extensions.
fn extract_file_paths(text: &str) -> Vec<&str> {
    let mut paths = Vec::new();
    for word in text.split_whitespace() {
        // Strip surrounding punctuation (quotes, backticks, parens, etc.)
        let trimmed = word.trim_matches(|c: char| {
            c == '"' || c == '\'' || c == '`' || c == '(' || c == ')' || c == ','
        });
        for ext in FILE_EXTENSIONS {
            if trimmed.ends_with(ext) && trimmed.len() > ext.len() {
                paths.push(trimmed);
                break;
            }
        }
    }
    paths
}

/// Generate a structured summary of the removed messages.
///
/// The summary includes:
/// - Message counts by role
/// - Tool names used
/// - Recent user requests (up to 3, truncated to 160 chars)
/// - Key file paths referenced
/// - Current work context from recent assistant messages
fn summarize_messages(messages: &[ConversationMessage], max_chars: usize) -> String {
    let mut summary = String::with_capacity(max_chars);

    // --- Counts by role ---
    let mut user_count = 0usize;
    let mut assistant_count = 0usize;
    let mut tool_use_count = 0usize;
    let mut tool_result_count = 0usize;

    for msg in messages {
        match msg.role {
            ConversationRole::User => user_count += 1,
            ConversationRole::Assistant => assistant_count += 1,
        }
        for block in &msg.content {
            match block {
                ContentBlock::ToolUse { .. } => tool_use_count += 1,
                ContentBlock::ToolResult { .. } => tool_result_count += 1,
                _ => {}
            }
        }
    }

    let _ = write!(
        summary,
        "[Compacted conversation history]\n\
         Messages removed: {} total ({} user, {} assistant, {} tool calls, {} tool results)\n",
        messages.len(),
        user_count,
        assistant_count,
        tool_use_count,
        tool_result_count,
    );

    // --- Tool names used ---
    let mut tool_names: Vec<&str> = Vec::new();
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { name, .. } = block {
                if !tool_names.contains(&name.as_str()) {
                    tool_names.push(name.as_str());
                }
            }
        }
    }
    if !tool_names.is_empty() {
        let _ = write!(summary, "Tools used: ");
        for (i, name) in tool_names.iter().enumerate() {
            if i > 0 {
                let _ = write!(summary, ", ");
            }
            let _ = write!(summary, "{}", name);
        }
        let _ = writeln!(summary);
    }

    // --- File paths referenced ---
    let mut all_paths: Vec<&str> = Vec::new();
    for msg in messages {
        for block in &msg.content {
            let text = match block {
                ContentBlock::Text { text } => text.as_str(),
                ContentBlock::ToolUse { input, .. } => input.as_str(),
                ContentBlock::ToolResult { content, .. } => content.as_str(),
            };
            for path in extract_file_paths(text) {
                if !all_paths.contains(&path) {
                    all_paths.push(path);
                    if all_paths.len() >= 15 {
                        break;
                    }
                }
            }
        }
        if all_paths.len() >= 15 {
            break;
        }
    }
    if !all_paths.is_empty() {
        let _ = write!(summary, "Key files: ");
        for (i, path) in all_paths.iter().enumerate() {
            if i > 0 {
                let _ = write!(summary, ", ");
            }
            let _ = write!(summary, "{}", path);
        }
        let _ = writeln!(summary);
    }

    // --- Recent user requests (up to 3, from the end of the removed slice) ---
    let user_messages: Vec<&ConversationMessage> = messages
        .iter()
        .filter(|m| m.role == ConversationRole::User)
        .collect();
    let recent_user: Vec<&ConversationMessage> = if user_messages.len() > 3 {
        user_messages[user_messages.len() - 3..].to_vec()
    } else {
        user_messages
    };

    if !recent_user.is_empty() {
        let _ = writeln!(summary, "Recent user requests:");
        for msg in recent_user {
            for block in &msg.content {
                if let ContentBlock::Text { text } = block {
                    let truncated = if text.len() > 160 {
                        // Find a safe truncation point (don't split mid-char)
                        let mut end = 160;
                        while end < text.len() && !text.is_char_boundary(end) {
                            end += 1;
                        }
                        let slice = &text[..end.min(text.len())];
                        let mut t = String::from(slice);
                        t.push_str("...");
                        t
                    } else {
                        text.clone()
                    };
                    let _ = writeln!(summary, "  - {}", truncated);
                }
            }
        }
    }

    // --- Work context from recent assistant messages ---
    let assistant_messages: Vec<&ConversationMessage> = messages
        .iter()
        .filter(|m| m.role == ConversationRole::Assistant)
        .collect();
    if let Some(last_assistant) = assistant_messages.last() {
        for block in &last_assistant.content {
            if let ContentBlock::Text { text } = block {
                let context_len = 300.min(text.len());
                let mut end = context_len;
                while end < text.len() && !text.is_char_boundary(end) {
                    end += 1;
                }
                let snippet = &text[..end.min(text.len())];
                let _ = writeln!(summary, "Last assistant context: {}...", snippet);
                break;
            }
        }
    }

    // Truncate to max_chars if we overshot
    if summary.len() > max_chars {
        summary.truncate(max_chars);
        // Ensure we don't truncate mid-char
        while !summary.is_char_boundary(summary.len()) {
            summary.pop();
        }
        summary.push_str("...");
    }

    summary
}

// ---------------------------------------------------------------------------
// Compaction
// ---------------------------------------------------------------------------

/// Compact the conversation history.
///
/// Splits messages into a "remove" prefix and a "preserve" suffix, generates
/// a structured summary of the removed messages, and returns the new message
/// list along with compaction metrics.
///
/// The new message list is: `[summary_message] + [preserved recent messages]`.
///
/// The summary is injected as a `User` message with a single `Text` block so
/// it is always valid in the Anthropic Messages API (user messages can appear
/// at position 0).
pub fn compact(
    messages: &[ConversationMessage],
    config: &CompactionConfig,
) -> (Vec<ConversationMessage>, CompactionResult) {
    let estimated_tokens_before = estimate_message_tokens(messages);

    // If there aren't enough messages to split, return as-is.
    if messages.len() <= config.preserve_recent {
        return (
            messages.to_vec(),
            CompactionResult {
                summary: String::new(),
                removed_count: 0,
                estimated_tokens_before,
                estimated_tokens_after: estimated_tokens_before,
            },
        );
    }

    let split_point = messages.len() - config.preserve_recent;
    let to_remove = &messages[..split_point];
    let to_preserve = &messages[split_point..];

    let summary = summarize_messages(to_remove, config.max_summary_chars);

    // Build the summary message as a user message (safe first position for the API).
    let summary_message = ConversationMessage {
        role: ConversationRole::User,
        content: alloc::vec![ContentBlock::Text {
            text: summary.clone(),
        }],
        // Use the timestamp of the last removed message, or 0.
        timestamp: to_remove.last().map(|m| m.timestamp).unwrap_or(0),
    };

    let mut new_messages = Vec::with_capacity(1 + to_preserve.len());
    new_messages.push(summary_message);

    // Ensure the first preserved message after the summary doesn't create an
    // invalid user-user sequence. If the first preserved message is a user
    // message, that's fine — the API allows consecutive user messages. But we
    // check whether the preserved window starts with an assistant message and
    // if so, we're good (summary is user, then assistant).
    for msg in to_preserve {
        new_messages.push(msg.clone());
    }

    let estimated_tokens_after = estimate_message_tokens(&new_messages);

    log::info!(
        "[compaction] removed {} messages, tokens {} -> {} (saved ~{})",
        split_point,
        estimated_tokens_before,
        estimated_tokens_after,
        estimated_tokens_before.saturating_sub(estimated_tokens_after),
    );

    (
        new_messages,
        CompactionResult {
            summary,
            removed_count: split_point,
            estimated_tokens_before,
            estimated_tokens_after,
        },
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;

    fn text_msg(role: ConversationRole, text: &str) -> ConversationMessage {
        ConversationMessage {
            role,
            content: vec![ContentBlock::Text {
                text: String::from(text),
            }],
            timestamp: 0,
        }
    }

    fn tool_msg(name: &str, input: &str) -> ConversationMessage {
        ConversationMessage {
            role: ConversationRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: String::from("id-1"),
                name: String::from(name),
                input: String::from(input),
            }],
            timestamp: 0,
        }
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 1);
        assert_eq!(estimate_tokens("hello world"), 3); // 11/4 + 1
        assert_eq!(estimate_tokens("a"), 1); // 1/4 + 1
    }

    #[test]
    fn test_should_compact_below_threshold() {
        let config = CompactionConfig {
            max_estimated_tokens: 100,
            preserve_recent: 2,
            ..Default::default()
        };
        let messages = vec![
            text_msg(ConversationRole::User, "hi"),
            text_msg(ConversationRole::Assistant, "hello"),
        ];
        assert!(!should_compact(&messages, &config));
    }

    #[test]
    fn test_should_compact_above_threshold() {
        let config = CompactionConfig {
            max_estimated_tokens: 5,
            preserve_recent: 2,
            ..Default::default()
        };
        let long_text = "a]".repeat(100);
        let messages = vec![
            text_msg(ConversationRole::User, &long_text),
            text_msg(ConversationRole::Assistant, &long_text),
            text_msg(ConversationRole::User, "recent"),
            text_msg(ConversationRole::Assistant, "recent reply"),
        ];
        assert!(should_compact(&messages, &config));
    }

    #[test]
    fn test_compact_preserves_recent() {
        let config = CompactionConfig {
            max_estimated_tokens: 5,
            preserve_recent: 2,
            max_summary_chars: 500,
        };
        let messages = vec![
            text_msg(ConversationRole::User, "old message 1"),
            text_msg(ConversationRole::Assistant, "old reply 1"),
            text_msg(ConversationRole::User, "recent question"),
            text_msg(ConversationRole::Assistant, "recent answer"),
        ];

        let (new_msgs, result) = compact(&messages, &config);

        // summary + 2 preserved = 3
        assert_eq!(new_msgs.len(), 3);
        assert_eq!(result.removed_count, 2);

        // Last two messages should be the recent ones
        if let ContentBlock::Text { text } = &new_msgs[1].content[0] {
            assert_eq!(text, "recent question");
        } else {
            panic!("expected text block");
        }
        if let ContentBlock::Text { text } = &new_msgs[2].content[0] {
            assert_eq!(text, "recent answer");
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn test_compact_no_op_when_few_messages() {
        let config = CompactionConfig {
            max_estimated_tokens: 5,
            preserve_recent: 10,
            max_summary_chars: 500,
        };
        let messages = vec![
            text_msg(ConversationRole::User, "hi"),
            text_msg(ConversationRole::Assistant, "hello"),
        ];

        let (new_msgs, result) = compact(&messages, &config);
        assert_eq!(new_msgs.len(), 2);
        assert_eq!(result.removed_count, 0);
    }

    #[test]
    fn test_summary_contains_tool_names() {
        let messages = vec![
            tool_msg("file_read", r#"{"path": "src/main.rs"}"#),
            text_msg(ConversationRole::User, "thanks"),
        ];
        let summary = summarize_messages(&messages, 2000);
        assert!(summary.contains("file_read"));
    }

    #[test]
    fn test_extract_file_paths_basic() {
        let text = "I edited src/main.rs and tests/foo.py today";
        let paths = extract_file_paths(text);
        assert!(paths.contains(&"src/main.rs"));
        assert!(paths.contains(&"tests/foo.py"));
    }

    #[test]
    fn test_summary_truncation() {
        let long_text = "x".repeat(5000);
        let messages = vec![text_msg(ConversationRole::User, &long_text)];
        let summary = summarize_messages(&messages, 200);
        assert!(summary.len() <= 210); // 200 + "..."
    }
}
