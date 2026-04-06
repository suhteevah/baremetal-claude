//! Native Rust Anthropic Messages API client.
//!
//! No reqwest. No hyper. No tokio. Raw HTTP/1.1 over a generic async byte stream.
//! Handles: Messages API, streaming SSE, tool use protocol.
//!
//! Request profile mirrors the official Claude Code client so API requests
//! are indistinguishable from standard CLI usage.

#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub mod messages;
pub mod permissions;
pub mod streaming;
pub mod tools;

// ---------------------------------------------------------------------------
// Request profile — matches official Claude Code client exactly
// ---------------------------------------------------------------------------

/// Anthropic API version string.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Client name as sent in User-Agent.
const CLIENT_NAME: &str = "claude-code";

/// Client version — kept in sync with upstream releases.
const CLIENT_VERSION: &str = "1.0.24";

/// Beta feature flags sent with every request.
const BETAS: &[&str] = &[
    "claude-code-20250219",
    "prompt-caching-scope-2026-01-05",
];

/// HTTP status codes that should trigger a retry.
const RETRYABLE_STATUS_CODES: &[u16] = &[408, 409, 429, 500, 502, 503, 504];

/// Maximum number of retry attempts.
const MAX_RETRIES: u32 = 2;

/// Initial backoff delay in milliseconds (doubles each retry, capped at MAX_BACKOFF_MS).
const INITIAL_BACKOFF_MS: u64 = 200;

/// Maximum backoff delay in milliseconds.
const MAX_BACKOFF_MS: u64 = 2000;

// ---------------------------------------------------------------------------
// AnthropicClient
// ---------------------------------------------------------------------------

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
            Some(format!("x-api-key: {}", key))
        } else if let Some(ref token) = self.oauth_token {
            Some(format!("Authorization: Bearer {}", token))
        } else {
            None
        }
    }

    /// Build all HTTP headers for an API request, matching the official
    /// Claude Code client profile exactly.
    pub fn request_headers(&self) -> Vec<String> {
        let mut headers = Vec::new();

        // Auth
        if let Some(ref key) = self.api_key {
            headers.push(format!("x-api-key: {}", key));
        }
        if let Some(ref token) = self.oauth_token {
            headers.push(format!("Authorization: Bearer {}", token));
        }

        // Standard headers matching official client
        headers.push(format!("content-type: application/json"));
        headers.push(format!("anthropic-version: {}", ANTHROPIC_VERSION));
        headers.push(format!("user-agent: {}/{}", CLIENT_NAME, CLIENT_VERSION));

        // Beta feature flags
        if !BETAS.is_empty() {
            let beta_str = BETAS.join(",");
            headers.push(format!("anthropic-beta: {}", beta_str));
        }

        headers
    }

    /// Format the Host header for a given hostname.
    pub fn host_header(hostname: &str) -> String {
        format!("Host: {}", hostname)
    }

    /// Build a complete HTTP/1.1 POST request to /v1/messages.
    ///
    /// Returns the raw bytes to write to the TLS stream.
    pub fn build_request(&self, hostname: &str, body: &[u8]) -> Vec<u8> {
        let mut req = String::new();
        req.push_str("POST /v1/messages HTTP/1.1\r\n");
        req.push_str(&format!("Host: {}\r\n", hostname));

        for header in self.request_headers() {
            req.push_str(&header);
            req.push_str("\r\n");
        }

        // Content-length must come last before the blank line that ends headers
        req.push_str(&format!("content-length: {}\r\n", body.len()));
        // Blank line terminates the HTTP header section
        req.push_str("\r\n");

        // Append JSON body directly after headers — single contiguous write to TLS
        let mut out = req.into_bytes();
        out.extend_from_slice(body);
        out
    }
}

// ---------------------------------------------------------------------------
// Retry logic — exponential backoff for rate limits and server errors
// ---------------------------------------------------------------------------

/// Retry configuration for API requests.
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: MAX_RETRIES,
            initial_backoff_ms: INITIAL_BACKOFF_MS,
            max_backoff_ms: MAX_BACKOFF_MS,
        }
    }
}

impl RetryConfig {
    /// Calculate the backoff delay for a given attempt number (0-indexed).
    /// Uses exponential backoff: 200ms -> 400ms -> 800ms ... capped at max_backoff_ms.
    pub fn backoff_ms(&self, attempt: u32) -> u64 {
        // 2^attempt via bit shift; saturate to avoid overflow on large attempt values
        let multiplier = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
        let delay = self.initial_backoff_ms.saturating_mul(multiplier);
        delay.min(self.max_backoff_ms)
    }

    /// Check if a given HTTP status code should trigger a retry.
    pub fn is_retryable(status: u16) -> bool {
        RETRYABLE_STATUS_CODES.contains(&status)
    }

    /// Parse the Retry-After header value (seconds) from response headers.
    /// Returns the wait time in milliseconds.
    pub fn parse_retry_after(headers: &str) -> Option<u64> {
        for line in headers.lines() {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("retry-after:") {
                let val = line["retry-after:".len()..].trim();
                if let Ok(secs) = val.parse::<u64>() {
                    return Some(secs * 1000);
                }
            }
        }
        None
    }
}

/// Result of checking whether to retry a request.
pub enum RetryDecision {
    /// Retry after the specified delay in milliseconds.
    RetryAfter(u64),
    /// Do not retry — give up.
    GiveUp,
}

/// Decide whether to retry a failed request.
///
/// `attempt` is the current attempt number (0 = first try, 1 = first retry, etc.).
/// `status` is the HTTP status code from the response.
/// `response_headers` is the raw HTTP response headers (for Retry-After parsing).
pub fn should_retry(
    config: &RetryConfig,
    attempt: u32,
    status: u16,
    response_headers: &str,
) -> RetryDecision {
    if attempt >= config.max_retries {
        return RetryDecision::GiveUp;
    }
    if !RetryConfig::is_retryable(status) {
        return RetryDecision::GiveUp;
    }

    // 429 often includes a server-specified wait time; honor it over our own backoff
    if status == 429 {
        if let Some(wait_ms) = RetryConfig::parse_retry_after(response_headers) {
            log::info!(
                "[api] rate limited (429), Retry-After: {}ms (attempt {}/{})",
                wait_ms, attempt + 1, config.max_retries
            );
            return RetryDecision::RetryAfter(wait_ms);
        }
    }

    // Otherwise use exponential backoff.
    let delay = config.backoff_ms(attempt);
    log::info!(
        "[api] retryable error ({}), backing off {}ms (attempt {}/{})",
        status, delay, attempt + 1, config.max_retries
    );
    RetryDecision::RetryAfter(delay)
}

// ---------------------------------------------------------------------------
// HTTP response parsing helpers
// ---------------------------------------------------------------------------

/// Parse the HTTP status code from a raw HTTP response.
///
/// Expects the first line to be e.g. `HTTP/1.1 429 Too Many Requests`.
/// Returns 0 if the status line cannot be parsed.
pub fn parse_http_status(raw: &[u8]) -> u16 {
    // Find the end of the status line; cap at 128 bytes to avoid scanning huge bodies
    let line_end = raw.iter().position(|&b| b == b'\r' || b == b'\n')
        .unwrap_or(raw.len().min(128));
    let first_line = core::str::from_utf8(&raw[..line_end]).unwrap_or("");
    first_line
        .split(' ')
        .nth(1)
        .unwrap_or("0")
        .parse::<u16>()
        .unwrap_or(0)
}

/// Extract the HTTP headers portion (before `\r\n\r\n`) as a string.
pub fn extract_http_headers(raw: &[u8]) -> &str {
    // Scan for the double-CRLF that separates headers from body per HTTP/1.1
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(raw.len());
    core::str::from_utf8(&raw[..end]).unwrap_or("")
}
