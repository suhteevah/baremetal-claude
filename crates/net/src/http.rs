//! Minimal HTTP/1.1 client.
//!
//! Formats raw HTTP/1.1 request bytes and parses response status lines,
//! headers, and bodies.  This is a zero-dependency (beyond `alloc`) HTTP
//! implementation designed for `#![no_std]` environments.
//!
//! No chunked transfer encoding, no redirects, no keep-alive management —
//! just enough to POST JSON to the Anthropic API and parse the response.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// An HTTP/1.1 request ready to be serialized to bytes.
pub struct HttpRequest {
    /// HTTP method (`"GET"`, `"POST"`, etc.).
    pub method: &'static str,
    /// Request path (e.g. `"/v1/messages"`).
    pub path: String,
    /// Host header value (e.g. `"api.anthropic.com"`).
    pub host: String,
    /// Additional headers beyond Host and Content-Length.
    pub headers: Vec<(String, String)>,
    /// Optional request body.
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    /// Create a GET request.
    pub fn get(host: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            method: "GET",
            path: path.into(),
            host: host.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// Create a POST request with a body.
    pub fn post(
        host: impl Into<String>,
        path: impl Into<String>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            method: "POST",
            path: path.into(),
            host: host.into(),
            headers: Vec::new(),
            body: Some(body),
        }
    }

    /// Add a header to the request.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Serialize the request to raw HTTP/1.1 bytes.
    ///
    /// Produces a complete request including the request line, Host header,
    /// Content-Length (if body is present), any additional headers, the blank
    /// line separator, and the body.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(512);

        // Request line: METHOD /path HTTP/1.1\r\n
        buf.extend_from_slice(self.method.as_bytes());
        buf.extend_from_slice(b" ");
        buf.extend_from_slice(self.path.as_bytes());
        buf.extend_from_slice(b" HTTP/1.1\r\n");

        // Host header (mandatory in HTTP/1.1)
        buf.extend_from_slice(b"Host: ");
        buf.extend_from_slice(self.host.as_bytes());
        buf.extend_from_slice(b"\r\n");

        // Content-Length if we have a body
        if let Some(ref body) = self.body {
            let len_str = format!("{}", body.len());
            buf.extend_from_slice(b"Content-Length: ");
            buf.extend_from_slice(len_str.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }

        // Additional headers
        for (name, value) in &self.headers {
            buf.extend_from_slice(name.as_bytes());
            buf.extend_from_slice(b": ");
            buf.extend_from_slice(value.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }

        // Blank line separating headers from body
        buf.extend_from_slice(b"\r\n");

        // Body
        if let Some(ref body) = self.body {
            buf.extend_from_slice(body);
        }

        buf
    }
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

/// A parsed HTTP/1.1 response.
#[derive(Debug)]
pub struct HttpResponse {
    /// HTTP status code (e.g. 200, 404, 500).
    pub status: u16,
    /// Reason phrase (e.g. "OK", "Not Found").
    pub reason: String,
    /// Response headers.
    pub headers: Vec<(String, String)>,
    /// Response body bytes.
    pub body: Vec<u8>,
}

/// Errors from HTTP response parsing.
#[derive(Debug)]
pub enum HttpError {
    /// The response data is incomplete (need more bytes).
    Incomplete,
    /// The status line is malformed.
    InvalidStatusLine,
    /// A header line is malformed.
    InvalidHeader,
    /// The Content-Length header has a non-numeric value.
    InvalidContentLength,
    /// The response is too large (exceeds our buffer limits).
    TooLarge,
}

impl HttpResponse {
    /// Parse a complete HTTP/1.1 response from raw bytes.
    ///
    /// This expects the entire response (headers + body) to be present in
    /// `data`.  For streaming, use [`HttpResponse::parse_headers`] first to
    /// determine how many body bytes to expect, then call
    /// [`HttpResponse::from_parts`] once enough data has arrived.
    pub fn parse(data: &[u8]) -> Result<Self, HttpError> {
        let (headers_end, status, reason, headers) = Self::parse_header_block(data)?;

        // Determine body length from Content-Length header.
        let content_length = Self::content_length_from_headers(&headers);

        let body_start = headers_end;
        let body = match content_length {
            Some(len) => {
                let available = data.len().saturating_sub(body_start);
                if available < len {
                    return Err(HttpError::Incomplete);
                }
                data[body_start..body_start + len].to_vec()
            }
            None => {
                // No Content-Length — take everything after headers.
                // This handles Connection: close style responses.
                data[body_start..].to_vec()
            }
        };

        Ok(Self {
            status,
            reason,
            headers,
            body,
        })
    }

    /// Parse only the HTTP headers from `data`.
    ///
    /// Returns `(bytes_consumed, status, reason, headers)` on success.
    /// `bytes_consumed` is the offset where the body begins (after the
    /// `\r\n\r\n` separator).
    ///
    /// Returns `Err(Incomplete)` if the header block is not yet complete
    /// (no `\r\n\r\n` found).
    pub fn parse_headers(
        data: &[u8],
    ) -> Result<(usize, u16, String, Vec<(String, String)>), HttpError> {
        Self::parse_header_block(data)
    }

    /// Construct a response from pre-parsed headers and a body.
    pub fn from_parts(
        status: u16,
        reason: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            status,
            reason,
            headers,
            body,
        }
    }

    /// Get the value of a header by name (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        let name_lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(n, _)| n.to_ascii_lowercase() == name_lower)
            .map(|(_, v)| v.as_str())
    }

    /// Extract Content-Length from parsed headers.
    pub fn content_length_from_headers(headers: &[(String, String)]) -> Option<usize> {
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("content-length") {
                return value.trim().parse().ok();
            }
        }
        None
    }

    /// Check if the response uses chunked transfer encoding.
    pub fn is_chunked(&self) -> bool {
        self.header("transfer-encoding")
            .map(|v| v.to_ascii_lowercase().contains("chunked"))
            .unwrap_or(false)
    }

    /// Body as a UTF-8 string (lossy conversion).
    pub fn body_as_str(&self) -> &str {
        core::str::from_utf8(&self.body).unwrap_or("<non-utf8 body>")
    }

    // ── Internal parsing ──────────────────────────────────────────────

    fn parse_header_block(
        data: &[u8],
    ) -> Result<(usize, u16, String, Vec<(String, String)>), HttpError> {
        // Find the end of the header block: \r\n\r\n
        let header_end = find_header_end(data).ok_or(HttpError::Incomplete)?;
        let header_bytes = &data[..header_end];

        // Split into lines.
        let mut lines = split_lines(header_bytes);

        // Parse status line: "HTTP/1.1 200 OK"
        let status_line = lines.next().ok_or(HttpError::InvalidStatusLine)?;
        let (status, reason) = parse_status_line(status_line)?;

        // Parse headers.
        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let (name, value) = parse_header_line(line)?;
            headers.push((name, value));
        }

        // Body starts after \r\n\r\n (4 bytes past the header block).
        let body_start = header_end + 4;

        Ok((body_start, status, reason, headers))
    }
}

// ---------------------------------------------------------------------------
// Chunked transfer encoding decoder
// ---------------------------------------------------------------------------

/// Decode a chunked transfer-encoded body.
///
/// Takes the raw bytes after the headers and decodes chunk-by-chunk until the
/// terminating `0\r\n\r\n` is found.
///
/// Returns the decoded body bytes on success, or `Err(Incomplete)` if the
/// chunked stream is not yet complete.
pub fn decode_chunked(data: &[u8]) -> Result<Vec<u8>, HttpError> {
    let mut result = Vec::new();
    let mut pos = 0;

    loop {
        // Find the end of the chunk size line.
        let line_end = find_crlf(&data[pos..]).ok_or(HttpError::Incomplete)?;
        let size_str = core::str::from_utf8(&data[pos..pos + line_end])
            .map_err(|_| HttpError::InvalidHeader)?;

        // Chunk size is hex, possibly followed by extensions (which we ignore).
        let size_hex = size_str.split(';').next().unwrap_or("").trim();
        let chunk_size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| HttpError::InvalidContentLength)?;

        pos += line_end + 2; // skip past \r\n

        if chunk_size == 0 {
            // Terminal chunk — we're done.
            break;
        }

        // Read chunk_size bytes of data.
        if pos + chunk_size > data.len() {
            return Err(HttpError::Incomplete);
        }
        result.extend_from_slice(&data[pos..pos + chunk_size]);
        pos += chunk_size;

        // Each chunk is followed by \r\n.
        if pos + 2 > data.len() || data[pos] != b'\r' || data[pos + 1] != b'\n' {
            return Err(HttpError::Incomplete);
        }
        pos += 2;
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// SSE (Server-Sent Events) line parser
// ---------------------------------------------------------------------------

/// An SSE event parsed from stream data.
#[derive(Debug, Clone)]
pub struct SseEvent {
    /// Event type (from `event:` field), or empty for default message events.
    pub event: String,
    /// Event data (from `data:` field(s), concatenated with newlines).
    pub data: String,
}

/// Parse SSE events from a chunk of stream data.
///
/// SSE events are delimited by blank lines.  Each event consists of
/// `field: value` lines where field is one of `event`, `data`, `id`, or
/// `retry`.  We only care about `event` and `data`.
///
/// Returns a vector of parsed events and the number of bytes consumed.
pub fn parse_sse_events(data: &[u8]) -> (Vec<SseEvent>, usize) {
    let text = match core::str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => return (Vec::new(), 0),
    };

    let mut events = Vec::new();
    let mut consumed = 0;
    let mut current_event = String::new();
    let mut current_data = String::new();

    for line in text.split('\n') {
        let line_bytes = line.len() + 1; // +1 for the \n

        let line = line.trim_end_matches('\r');

        if line.is_empty() {
            // Blank line = end of event.
            if !current_data.is_empty() {
                events.push(SseEvent {
                    event: core::mem::take(&mut current_event),
                    data: core::mem::take(&mut current_data),
                });
            }
            current_event.clear();
            current_data.clear();
            consumed += line_bytes;
            continue;
        }

        if let Some(value) = line.strip_prefix("event:") {
            current_event = String::from(value.trim_start());
        } else if let Some(value) = line.strip_prefix("data:") {
            if !current_data.is_empty() {
                current_data.push('\n');
            }
            current_data.push_str(value.trim_start());
        }
        // Ignore `id:`, `retry:`, and comments (lines starting with `:`)

        consumed += line_bytes;
    }

    // Don't consume the last incomplete event — leave it for the next call.
    // Only count bytes up to the last blank-line-terminated event.
    // We already consumed line by line above; the caller can use `consumed`
    // to advance their buffer.

    (events, consumed)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find the offset of `\r\n\r\n` in `data`, returning the position of the
/// first `\r` in the sequence.
fn find_header_end(data: &[u8]) -> Option<usize> {
    if data.len() < 4 {
        return None;
    }
    for i in 0..data.len() - 3 {
        if &data[i..i + 4] == b"\r\n\r\n" {
            return Some(i);
        }
    }
    None
}

/// Find the offset of the first `\r\n` in `data`.
fn find_crlf(data: &[u8]) -> Option<usize> {
    if data.len() < 2 {
        return None;
    }
    for i in 0..data.len() - 1 {
        if data[i] == b'\r' && data[i + 1] == b'\n' {
            return Some(i);
        }
    }
    None
}

/// Split header bytes into lines on `\r\n`.
fn split_lines(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    data.split(|&b| b == b'\n')
        .map(|line| {
            if line.last() == Some(&b'\r') {
                &line[..line.len() - 1]
            } else {
                line
            }
        })
}

/// Parse "HTTP/1.1 200 OK" into (200, "OK").
fn parse_status_line(line: &[u8]) -> Result<(u16, String), HttpError> {
    let s = core::str::from_utf8(line).map_err(|_| HttpError::InvalidStatusLine)?;

    // Split: "HTTP/1.1 200 OK" -> ["HTTP/1.1", "200", "OK"]
    let mut parts = s.splitn(3, ' ');

    let _version = parts.next().ok_or(HttpError::InvalidStatusLine)?;
    let status_str = parts.next().ok_or(HttpError::InvalidStatusLine)?;
    let reason = parts.next().unwrap_or("");

    let status: u16 = status_str
        .parse()
        .map_err(|_| HttpError::InvalidStatusLine)?;

    Ok((status, String::from(reason)))
}

/// Parse "Content-Type: application/json" into ("Content-Type", "application/json").
fn parse_header_line(line: &[u8]) -> Result<(String, String), HttpError> {
    let s = core::str::from_utf8(line).map_err(|_| HttpError::InvalidHeader)?;

    let colon = s.find(':').ok_or(HttpError::InvalidHeader)?;
    let name = &s[..colon];
    let value = s[colon + 1..].trim_start();

    Ok((String::from(name), String::from(value)))
}

// ---------------------------------------------------------------------------
// Convenience helpers for API usage
// ---------------------------------------------------------------------------

/// Helper to create a JSON POST request for the Anthropic Messages API.
pub fn anthropic_messages_request(
    api_key: &str,
    body_json: &[u8],
) -> HttpRequest {
    HttpRequest::post("api.anthropic.com", "/v1/messages", body_json.to_vec())
        .header("Content-Type", "application/json")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("Accept", "text/event-stream")
        .header("Connection", "close")
}

/// Helper to create a JSON POST request for OAuth device code flow.
pub fn oauth_device_code_request(client_id: &str) -> HttpRequest {
    let body = format!(
        r#"{{"client_id":"{}","scope":"org:read user:read"}}"#,
        client_id
    );
    HttpRequest::post(
        "auth.anthropic.com",
        "/oauth/device/code",
        body.into_bytes(),
    )
    .header("Content-Type", "application/json")
    .header("Connection", "close")
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate alloc;
    use alloc::string::String;
    use alloc::vec;

    #[test]
    fn test_request_serialization() {
        let req = HttpRequest::post("example.com", "/api", b"hello".to_vec())
            .header("Content-Type", "text/plain");
        let bytes = req.to_bytes();
        let text = core::str::from_utf8(&bytes).unwrap();

        assert!(text.starts_with("POST /api HTTP/1.1\r\n"));
        assert!(text.contains("Host: example.com\r\n"));
        assert!(text.contains("Content-Length: 5\r\n"));
        assert!(text.contains("Content-Type: text/plain\r\n"));
        assert!(text.ends_with("\r\n\r\nhello"));
    }

    #[test]
    fn test_response_parsing() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi";
        let resp = HttpResponse::parse(raw).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.reason, "OK");
        assert_eq!(resp.body, b"hi");
        assert_eq!(resp.header("content-length"), Some("2"));
    }

    #[test]
    fn test_incomplete_response() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nhi";
        let result = HttpResponse::parse(raw);
        assert!(matches!(result, Err(HttpError::Incomplete)));
    }

    #[test]
    fn test_chunked_decode() {
        let data = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let body = decode_chunked(data).unwrap();
        assert_eq!(&body, b"hello world");
    }

    #[test]
    fn test_sse_parsing() {
        let data = b"event: content_block_delta\ndata: {\"type\":\"text\"}\n\n";
        let (events, consumed) = parse_sse_events(data);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "content_block_delta");
        assert_eq!(events[0].data, "{\"type\":\"text\"}");
        assert_eq!(consumed, data.len());
    }

    // `to_ascii_lowercase` used on str slices — verify no-std compat
    #[test]
    fn test_case_insensitive_header() {
        let raw = b"HTTP/1.1 200 OK\r\nCONTENT-TYPE: text/html\r\nContent-Length: 0\r\n\r\n";
        let resp = HttpResponse::parse(raw).unwrap();
        assert_eq!(resp.header("content-type"), Some("text/html"));
    }
}
