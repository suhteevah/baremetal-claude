//! Native Rust Anthropic Messages API client.
//!
//! No reqwest. No hyper. No tokio. Raw HTTP/1.1 over a generic async byte stream.
//! Handles: Messages API, streaming SSE, tool use protocol.

#![no_std]
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub mod messages;
pub mod streaming;
pub mod tools;

/// Anthropic API client. Stateless per-call — caller provides the TLS stream.
pub struct AnthropicClient {
    pub api_key: Option<String>,
    pub oauth_token: Option<String>,
    pub model: String,
    pub max_tokens: u32,
}

impl AnthropicClient {
    pub fn new() -> Self {
        Self {
            api_key: None,
            oauth_token: None,
            model: String::from("claude-sonnet-4-20250514"),
            max_tokens: 8192,
        }
    }

    pub fn with_api_key(mut self, key: String) -> Self {
        self.api_key = Some(key);
        self
    }

    pub fn with_oauth_token(mut self, token: String) -> Self {
        self.oauth_token = Some(token);
        self
    }

    pub fn auth_header(&self) -> Option<String> {
        if let Some(ref key) = self.api_key {
            Some(alloc::format!("x-api-key: {}", key))
        } else if let Some(ref token) = self.oauth_token {
            Some(alloc::format!("Authorization: Bearer {}", token))
        } else {
            None
        }
    }
}
