//! Cloudflare challenge detection and solving.
//!
//! Detects Cloudflare "Under Attack Mode" (IUAM) challenge pages, extracts
//! the challenge JavaScript, evaluates it via `js-lite`, and returns the
//! resulting cookie value that should be set for the next request.
//!
//! # Typical Cloudflare challenge flow:
//!
//! 1. First request returns HTTP 503 with HTML containing "Just a moment..."
//! 2. The HTML has a `<script>` block that does math/string ops
//! 3. The script computes a value and sets `document.cookie` to a clearance token
//! 4. The script then submits a form or redirects with that cookie
//! 5. The server validates the cookie and lets you through
//!
//! We handle step 2-3 by extracting the JS and running it through js-lite.

use alloc::string::String;
use alloc::vec::Vec;

/// Result of analyzing a page for Cloudflare challenges.
#[derive(Debug)]
pub struct ChallengeResult {
    /// Whether a Cloudflare challenge was detected.
    pub is_challenge: bool,
    /// The computed cookie value (if challenge was solved).
    pub cookie: Option<String>,
    /// The form action URL to submit to (if any).
    pub submit_url: Option<String>,
    /// Additional form fields to include in the submission.
    pub form_fields: Vec<(String, String)>,
    /// Debug info about what was detected.
    pub debug_info: String,
}

/// Detect whether HTML content is a Cloudflare challenge page.
pub fn is_cloudflare_challenge(html: &str) -> bool {
    // Check for common Cloudflare challenge indicators
    let indicators = [
        "Just a moment...",
        "cf-browser-verification",
        "cf_chl_opt",
        "cf_chl_prog",
        "cf-challenge-running",
        "_cf_chl_opt",
        "Checking your browser",
        "jschl_vc",
        "jschl_answer",
        "cf_chl_2",
        "turnstile",
        "challenges.cloudflare.com",
        "__cf_chl_f_tk",
    ];

    for indicator in &indicators {
        if html.contains(indicator) {
            return true;
        }
    }

    false
}

/// Attempt to solve a Cloudflare challenge from the HTML content.
///
/// # Arguments
/// * `html` - The full HTML of the challenge page
/// * `hostname` - The hostname being accessed (e.g., "console.anthropic.com")
/// * `path` - The path being accessed (e.g., "/")
/// * `existing_cookies` - Any cookies already set for the domain
///
/// # Returns
/// A `ChallengeResult` with the solved cookie (if successful).
pub fn solve_challenge(
    html: &str,
    hostname: &str,
    path: &str,
    existing_cookies: &str,
) -> ChallengeResult {
    log::info!(
        "[cloudflare] attempting to solve challenge for {}{}",
        hostname, path
    );

    let mut result = ChallengeResult {
        is_challenge: false,
        cookie: None,
        submit_url: None,
        form_fields: Vec::new(),
        debug_info: String::new(),
    };

    if !is_cloudflare_challenge(html) {
        result.debug_info = String::from("not a Cloudflare challenge page");
        return result;
    }

    result.is_challenge = true;

    // Extract all <script> blocks
    let scripts = extract_scripts(html);
    result.debug_info.push_str(&alloc::format!(
        "found {} script blocks\n",
        scripts.len()
    ));

    // Extract form data (Cloudflare uses hidden forms for submission)
    extract_form_data(html, &mut result);

    // Try each script block looking for the challenge
    for (i, script) in scripts.iter().enumerate() {
        if is_challenge_script(script) {
            result.debug_info.push_str(&alloc::format!(
                "script {} looks like a challenge ({} bytes)\n",
                i,
                script.len()
            ));

            match js_lite::execute_with_context(script, existing_cookies, hostname, path) {
                Ok((output, cookie)) => {
                    if !cookie.is_empty() {
                        log::info!(
                            "[cloudflare] challenge solved! cookie: {}...",
                            &cookie[..cookie.len().min(40)]
                        );
                        result.cookie = Some(cookie);
                        result.debug_info.push_str("challenge solved via js-lite\n");
                        if !output.is_empty() {
                            result.debug_info.push_str(&alloc::format!(
                                "script output: {}\n",
                                &output[..output.len().min(200)]
                            ));
                        }
                        return result;
                    } else {
                        result.debug_info.push_str("script ran but no cookie was set\n");
                        if !output.is_empty() {
                            result.debug_info.push_str(&alloc::format!(
                                "output: {}\n",
                                &output[..output.len().min(200)]
                            ));
                        }
                    }
                }
                Err(e) => {
                    result.debug_info.push_str(&alloc::format!(
                        "script {} failed: {}\n",
                        i,
                        &e[..e.len().min(200)]
                    ));
                    log::warn!("[cloudflare] script {} eval error: {}", i, e);
                }
            }
        }
    }

    // If no individual script worked, try combining all challenge-related scripts
    let combined = combine_challenge_scripts(&scripts);
    if !combined.is_empty() {
        result.debug_info.push_str("trying combined script approach\n");
        match js_lite::execute_with_context(&combined, existing_cookies, hostname, path) {
            Ok((_output, cookie)) => {
                if !cookie.is_empty() {
                    log::info!("[cloudflare] challenge solved with combined scripts!");
                    result.cookie = Some(cookie);
                    result.debug_info.push_str("solved via combined scripts\n");
                    return result;
                }
            }
            Err(e) => {
                result.debug_info.push_str(&alloc::format!(
                    "combined script failed: {}\n",
                    &e[..e.len().min(200)]
                ));
            }
        }
    }

    // Try to extract the answer from inline JS patterns
    if let Some(cookie) = try_extract_inline_cookie(html) {
        result.cookie = Some(cookie);
        result.debug_info.push_str("extracted cookie from inline pattern\n");
        return result;
    }

    result.debug_info.push_str("could not solve challenge\n");
    result
}

/// Extract all `<script>` block contents from HTML.
fn extract_scripts(html: &str) -> Vec<String> {
    let mut scripts = Vec::new();
    let lower = html_to_lowercase(html);
    let mut pos = 0;

    while let Some(start) = find_ignoring_case(&lower, "<script", pos) {
        // Find the end of the opening tag
        let tag_end = match lower[start..].find('>') {
            Some(p) => start + p + 1,
            None => break,
        };

        // Skip scripts with src= (external scripts we can't evaluate)
        let tag = &html[start..tag_end];
        if tag.contains("src=") || tag.contains("src =") {
            pos = tag_end;
            continue;
        }

        // Find </script>
        let script_end = match find_ignoring_case(&lower, "</script", tag_end) {
            Some(p) => p,
            None => break,
        };

        let script_body = html[tag_end..script_end].trim();
        if !script_body.is_empty() {
            scripts.push(String::from(script_body));
        }

        pos = script_end + 9; // skip past </script>
    }

    scripts
}

/// Check if a script block looks like a Cloudflare challenge script.
fn is_challenge_script(script: &str) -> bool {
    // Cloudflare challenge scripts typically:
    // - Set document.cookie
    // - Reference cf_chl, jschl, or challenge-related variables
    // - Do math operations and string concatenation
    // - Use setTimeout or form submission

    let challenge_patterns = [
        "document.cookie",
        "cf_chl",
        "jschl",
        "challenge",
        "cf_clearance",
        "__cf_chl",
        "turnstile",
        "cpo.src",
        "cRq",
        "window._cf_chl_opt",
    ];

    let mut score = 0;
    for pattern in &challenge_patterns {
        if script.contains(pattern) {
            score += 1;
        }
    }

    // Also check for math-heavy scripts (common in old-style challenges)
    if script.contains("parseInt") && script.contains("charAt") {
        score += 1;
    }
    if script.contains("Math.") && (script.contains("+=") || script.contains("*=")) {
        score += 1;
    }

    score >= 1
}

/// Combine multiple challenge scripts into one.
fn combine_challenge_scripts(scripts: &[String]) -> String {
    let mut combined = String::new();
    for script in scripts {
        if is_challenge_script(script) {
            combined.push_str(script);
            combined.push_str(";\n");
        }
    }
    combined
}

/// Extract form data from a Cloudflare challenge page.
fn extract_form_data(html: &str, result: &mut ChallengeResult) {
    let lower = html_to_lowercase(html);

    // Look for the challenge form
    if let Some(form_start) = find_ignoring_case(&lower, "<form", 0) {
        let form_tag_end = match lower[form_start..].find('>') {
            Some(p) => form_start + p,
            None => return,
        };

        let form_tag = &html[form_start..form_tag_end + 1];

        // Extract action URL
        if let Some(action) = extract_attribute(form_tag, "action") {
            result.submit_url = Some(action);
        }

        // Find form end
        let form_end = find_ignoring_case(&lower, "</form", form_tag_end).unwrap_or(html.len());
        let form_body = &html[form_tag_end + 1..form_end];

        // Extract hidden input fields
        let form_lower = html_to_lowercase(form_body);
        let mut input_pos = 0;
        while let Some(input_start) = find_ignoring_case(&form_lower, "<input", input_pos) {
            let input_end = match form_lower[input_start..].find('>') {
                Some(p) => input_start + p + 1,
                None => break,
            };
            let input_tag = &form_body[input_start..input_end];
            if let (Some(name), Some(value)) = (
                extract_attribute(input_tag, "name"),
                extract_attribute(input_tag, "value"),
            ) {
                result.form_fields.push((name, value));
            }
            input_pos = input_end;
        }
    }
}

/// Try to extract a cookie directly from inline JavaScript patterns.
///
/// Some simpler Cloudflare challenges have patterns like:
/// `document.cookie="cf_clearance=..."`
fn try_extract_inline_cookie(html: &str) -> Option<String> {
    // Look for direct cookie assignment
    let patterns = [
        "document.cookie=\"",
        "document.cookie='",
        "document.cookie = \"",
        "document.cookie = '",
    ];

    for pattern in &patterns {
        if let Some(start) = html.find(pattern) {
            let value_start = start + pattern.len();
            let quote = if pattern.ends_with('"') { '"' } else { '\'' };
            if let Some(end) = html[value_start..].find(quote) {
                let cookie = &html[value_start..value_start + end];
                // Only return if it looks like a real cookie (has = sign)
                if cookie.contains('=') && cookie.len() > 5 {
                    // Extract just the key=value part (before any ;)
                    let cookie_val = cookie.split(';').next().unwrap_or(cookie);
                    return Some(String::from(cookie_val));
                }
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// HTML string helpers
// ---------------------------------------------------------------------------

fn html_to_lowercase(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        result.push(c.to_ascii_lowercase());
    }
    result
}

fn find_ignoring_case(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if from >= haystack.len() { return None; }
    haystack[from..].find(needle).map(|p| p + from)
}

fn extract_attribute(tag: &str, attr_name: &str) -> Option<String> {
    let lower = html_to_lowercase(tag);
    let needle = alloc::format!("{}=", attr_name);
    let pos = lower.find(&needle)?;
    let after = &tag[pos + needle.len()..];

    let (quote, rest) = if after.starts_with('"') {
        ('"', &after[1..])
    } else if after.starts_with('\'') {
        ('\'', &after[1..])
    } else {
        // Unquoted attribute value
        let end = after.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(after.len());
        return Some(String::from(&after[..end]));
    };

    let end = rest.find(quote)?;
    Some(String::from(&rest[..end]))
}

// ---------------------------------------------------------------------------
// High-level API for the transport layer
// ---------------------------------------------------------------------------

/// Cookie jar entry for Cloudflare clearance.
pub struct CloudflareCookie {
    /// The full cookie string (e.g., "cf_clearance=abc123")
    pub cookie: String,
    /// The domain it applies to.
    pub domain: String,
}

/// Check if a response indicates a Cloudflare challenge and attempt to solve it.
///
/// This is the function that `wraith-transport` should call after receiving
/// an HTTP response. If it returns `Some(cookie)`, the transport should:
/// 1. Add the cookie to the request headers
/// 2. Re-fetch the original URL
///
/// # Arguments
/// * `status` - HTTP status code of the response
/// * `body` - Response body (HTML)
/// * `hostname` - The hostname being accessed
/// * `path` - The path being accessed
/// * `existing_cookies` - Any cookies already set for the domain
pub fn handle_cloudflare_response(
    status: u16,
    body: &[u8],
    hostname: &str,
    path: &str,
    existing_cookies: &str,
) -> Option<CloudflareCookie> {
    // Cloudflare challenges typically return 403 or 503
    if status != 403 && status != 503 {
        return None;
    }

    let html = match core::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => return None,
    };

    if !is_cloudflare_challenge(html) {
        return None;
    }

    log::info!(
        "[cloudflare] detected challenge page for {} (status {})",
        hostname, status
    );

    let result = solve_challenge(html, hostname, path, existing_cookies);

    if let Some(cookie) = result.cookie {
        log::info!("[cloudflare] challenge solved, cookie: {}...", &cookie[..cookie.len().min(40)]);
        Some(CloudflareCookie {
            cookie,
            domain: String::from(hostname),
        })
    } else {
        log::warn!(
            "[cloudflare] could not solve challenge for {}. Debug: {}",
            hostname,
            result.debug_info
        );
        None
    }
}
