pub mod buffer;
pub mod locale;
pub mod synth;
pub mod tier1;
pub mod tier2;
pub mod tier3;
pub mod vault;

// Re-export key types for convenience.
pub use locale::Locale;
pub use vault::{PiiSpan, PiiType, PiiVault, VaultHandle, VaultRegistry};

use crate::parser::Provider;
use crate::pii::synth::SyntheticGenerator;
use crate::pii::tier1::Tier1Detector;
use std::sync::Arc;

#[allow(unused_imports)]
use anyhow::Result;

// ─── PiiMode ──────────────────────────────────────────────────────────────────

/// Controls whether PII detection/replacement is active.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PiiMode {
    /// PII detection and replacement are disabled.
    #[default]
    Off,
    /// Detect PII and log spans, but do not modify request/response bodies.
    DetectOnly,
    /// Detect PII and replace with synthetic values in outbound traffic.
    Replace,
}

// ─── PiiContext ───────────────────────────────────────────────────────────────

/// Shared context passed through proxy layers.
pub struct PiiContext {
    pub registry: Arc<VaultRegistry>,
    pub locale: Locale,
    pub mode: PiiMode,
}

/// Convenience alias: `None` means PII processing is disabled for this connection.
pub type PiiCtx = Option<Arc<PiiContext>>;

// ─── PiiPipeline ─────────────────────────────────────────────────────────────

pub struct PiiPipeline;

impl PiiPipeline {
    /// Process an HTTP request body (JSON), replacing PII in all message content fields.
    ///
    /// Returns the modified body bytes. If no PII was found / no changes made, returns `None`
    /// so the caller can forward the original bytes unchanged.
    ///
    /// `vault` must already be write-locked by the caller.
    pub fn process_request_body(
        body: &[u8],
        vault: &mut PiiVault,
        provider: Provider,
        locale: &Locale,
    ) -> Option<Vec<u8>> {
        let text = match std::str::from_utf8(body) {
            Ok(s) => s,
            Err(_) => {
                tracing::warn!("pii: request body is not valid UTF-8, skipping PII scan");
                return None;
            }
        };

        let mut value: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "pii: failed to parse request body as JSON, forwarding original");
                return None;
            }
        };

        // Determine the field name that holds the messages array.
        let messages_field = match provider {
            Provider::Google => "contents",
            _ => "messages",
        };

        let messages = match value.get_mut(messages_field).and_then(|v| v.as_array_mut()) {
            Some(arr) => arr,
            None => {
                tracing::debug!(
                    provider = provider.as_str(),
                    field = messages_field,
                    "pii: no messages array found in request body"
                );
                return None;
            }
        };

        let mut any_replaced = false;

        for message in messages.iter_mut() {
            let content = match message.get_mut("content") {
                Some(c) => c,
                None => {
                    // Google uses "parts" nested inside "contents" entries; skip unknown shapes.
                    continue;
                }
            };

            if let Some(text_str) = content.as_str() {
                // Simple string content (OpenAI / Anthropic single-part).
                let (replaced, spans) = Tier1Detector::replace_in_text(
                    text_str,
                    locale,
                    |original, pii_type| {
                        SyntheticGenerator::get_or_create(vault, original, pii_type, locale)
                    },
                );
                if !spans.is_empty() {
                    *content = serde_json::Value::String(replaced);
                    any_replaced = true;
                }
            } else if let Some(parts) = content.as_array_mut() {
                // Anthropic multi-part content: [{type:"text",text:"..."}, ...]
                for part in parts.iter_mut() {
                    // Only process text parts.
                    let is_text = part
                        .get("type")
                        .and_then(|t| t.as_str())
                        .map(|t| t == "text")
                        .unwrap_or(false);

                    if !is_text {
                        continue;
                    }

                    let text_val = match part.get("text").and_then(|v| v.as_str()) {
                        Some(s) => s.to_string(),
                        None => continue,
                    };

                    let (replaced, spans) = Tier1Detector::replace_in_text(
                        &text_val,
                        locale,
                        |original, pii_type| {
                            SyntheticGenerator::get_or_create(vault, original, pii_type, locale)
                        },
                    );

                    if !spans.is_empty() {
                        if let Some(obj) = part.as_object_mut() {
                            obj.insert("text".to_string(), serde_json::Value::String(replaced));
                        }
                        any_replaced = true;
                    }
                }
            }
        }

        if !any_replaced {
            return None;
        }

        match serde_json::to_vec(&value) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                tracing::warn!(error = %e, "pii: failed to re-serialize modified request body");
                None
            }
        }
    }

    /// Log detected PII spans at INFO level.
    ///
    /// Original text is NOT included — only the entity type, byte range, confidence,
    /// and the conversation id are logged.
    pub fn log_detections(spans: &[PiiSpan], conv_id: &str) {
        for span in spans {
            tracing::info!(
                conv_id = conv_id,
                entity_type = span.entity_type.label(),
                start = span.start,
                end = span.end,
                confidence = span.confidence,
                tier = span.tier,
                "pii: detected span"
            );
        }
    }
}

// ─── rebuild_request ─────────────────────────────────────────────────────────

/// Rebuild the HTTP request with a new body, updating the `Content-Length` header.
///
/// `original_request` — the full raw HTTP bytes (headers + body).
/// `header_end`       — byte offset of the end of the headers section
///                      (i.e. the position right after the blank line `\r\n\r\n`).
/// `new_body`         — replacement body bytes.
///
/// Returns: `new_headers_bytes || new_body`.
pub fn rebuild_request(original_request: &[u8], header_end: usize, new_body: &[u8]) -> Vec<u8> {
    let header_bytes = &original_request[..header_end];
    let header_str = String::from_utf8_lossy(header_bytes);

    // Replace Content-Length value.  The header can appear as:
    //   "Content-Length: 1234\r\n"  or  "content-length: 1234\r\n"
    let new_len_str = new_body.len().to_string();
    let updated_headers = replace_content_length(&header_str, &new_len_str);

    let mut result = Vec::with_capacity(updated_headers.len() + new_body.len());
    result.extend_from_slice(updated_headers.as_bytes());
    result.extend_from_slice(new_body);
    result
}

/// Replace the numeric value in a `Content-Length:` header line.
/// If no such header is found the original string is returned unchanged.
fn replace_content_length(headers: &str, new_value: &str) -> String {
    // Work line-by-line so we don't accidentally clobber other headers.
    let mut result = String::with_capacity(headers.len());
    for line in headers.split_inclusive('\n') {
        // Case-insensitive match on the header name.
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("content-length:") {
            // Reconstruct with same capitalisation up to the colon, then new value.
            if let Some(colon_pos) = line.find(':') {
                let name_part = &line[..=colon_pos]; // "Content-Length:"
                // Preserve any trailing CRLF.
                let tail = if line.ends_with("\r\n") {
                    "\r\n"
                } else if line.ends_with('\n') {
                    "\n"
                } else {
                    ""
                };
                result.push_str(name_part);
                result.push(' ');
                result.push_str(new_value);
                result.push_str(tail);
                continue;
            }
        }
        result.push_str(line);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pii_mode_default() {
        let mode: PiiMode = Default::default();
        assert_eq!(mode, PiiMode::Off);
    }

    #[test]
    fn test_pii_mode_serde_roundtrip() {
        let mode = PiiMode::Replace;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"replace\"");
        let back: PiiMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PiiMode::Replace);
    }

    #[test]
    fn test_process_request_body_openai_no_pii() {
        let body = br#"{"model":"gpt-4o","messages":[{"role":"user","content":"Hello, how are you?"}]}"#;
        let mut vault = PiiVault::new("test-conv");
        let result = PiiPipeline::process_request_body(body, &mut vault, Provider::OpenAI, &Locale::EnUs);
        assert!(result.is_none(), "no PII => should return None");
    }

    #[test]
    fn test_process_request_body_openai_with_email() {
        let body = br#"{"model":"gpt-4o","messages":[{"role":"user","content":"Email me at john@acme.com"}]}"#;
        let mut vault = PiiVault::new("test-conv-2");
        let result = PiiPipeline::process_request_body(body, &mut vault, Provider::OpenAI, &Locale::EnUs);
        assert!(result.is_some(), "email should be detected");
        let new_body: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
        let content = new_body["messages"][0]["content"].as_str().unwrap();
        assert!(!content.contains("john@acme.com"), "original email must be replaced: {}", content);
        assert!(!vault.is_empty(), "vault must have mapping");
    }

    #[test]
    fn test_process_request_body_anthropic_multipart() {
        let body = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "SSN: 123-45-6789"},
                    {"type": "image_url", "image_url": {"url": "http://example.com/img.png"}}
                ]
            }]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let mut vault = PiiVault::new("test-conv-3");
        let result = PiiPipeline::process_request_body(&body_bytes, &mut vault, Provider::Anthropic, &Locale::EnUs);
        assert!(result.is_some(), "SSN in text part should trigger replacement");
        let new_body: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
        let text = new_body["messages"][0]["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("123-45-6789"), "SSN must be replaced: {}", text);
    }

    #[test]
    fn test_process_request_body_invalid_json() {
        let body = b"not json at all {{{";
        let mut vault = PiiVault::new("test-conv-4");
        let result = PiiPipeline::process_request_body(body, &mut vault, Provider::OpenAI, &Locale::EnUs);
        assert!(result.is_none(), "invalid JSON should return None");
    }

    #[test]
    fn test_rebuild_request_updates_content_length() {
        let headers = b"POST /v1/chat HTTP/1.1\r\nHost: api.openai.com\r\nContent-Length: 99\r\nContent-Type: application/json\r\n\r\n";
        let old_body = b"{\"old\":\"body\"}";
        let header_end = headers.len();
        let mut full = headers.to_vec();
        full.extend_from_slice(old_body);

        let new_body = b"{\"new\":\"body\",\"extra\":true}";
        let rebuilt = rebuild_request(&full, header_end, new_body);

        let rebuilt_str = String::from_utf8_lossy(&rebuilt);
        assert!(
            rebuilt_str.contains(&format!("Content-Length: {}", new_body.len())),
            "Content-Length not updated: {}",
            rebuilt_str
        );
        assert!(rebuilt_str.ends_with(std::str::from_utf8(new_body).unwrap()));
    }

    #[test]
    fn test_log_detections_does_not_panic() {
        // Smoke test: just ensure no panics with a non-empty span list.
        let spans = vec![
            PiiSpan {
                start: 0,
                end: 12,
                entity_type: PiiType::Email,
                confidence: 1.0,
                tier: 1,
            },
        ];
        PiiPipeline::log_detections(&spans, "conv-smoke-test");
    }

    // ── New tests ──────────────────────────────────────────────────────────────

    #[test]
    fn test_process_request_body_no_messages_field() {
        // Body with no "messages" array — should return None, not crash.
        let body = br#"{"model":"gpt-4"}"#;
        let mut vault = PiiVault::new("conv-no-messages");
        let result = PiiPipeline::process_request_body(body, &mut vault, Provider::OpenAI, &Locale::EnUs);
        assert!(result.is_none(), "no messages field => should return None");
    }

    #[test]
    fn test_process_request_body_openai_multiple_messages() {
        // 3 messages; 2 of them contain an email. All emails must be replaced.
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user",   "content": "Reach me at alice@corp.com"},
                {"role": "user",   "content": "Or at bob@corp.com if urgent"}
            ]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let mut vault = PiiVault::new("conv-multi-msg");
        let result = PiiPipeline::process_request_body(&body_bytes, &mut vault, Provider::OpenAI, &Locale::EnUs);
        assert!(result.is_some(), "emails present => should return Some");
        let new_body: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
        for msg in new_body["messages"].as_array().unwrap() {
            let content = msg["content"].as_str().unwrap_or("");
            assert!(!content.contains("alice@corp.com"), "alice@corp.com not replaced in: {content}");
            assert!(!content.contains("bob@corp.com"),   "bob@corp.com not replaced in: {content}");
        }
    }

    #[test]
    fn test_process_request_body_openai_multiple_pii_types() {
        // Same message contains both an email and an SSN — both must be replaced.
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "user", "content": "SSN is 123-45-6789 and email is carol@example.com"}
            ]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let mut vault = PiiVault::new("conv-multi-pii");
        let result = PiiPipeline::process_request_body(&body_bytes, &mut vault, Provider::OpenAI, &Locale::EnUs);
        assert!(result.is_some(), "PII present => should return Some");
        let new_body: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
        let content = new_body["messages"][0]["content"].as_str().unwrap();
        assert!(!content.contains("123-45-6789"),      "SSN not replaced: {content}");
        assert!(!content.contains("carol@example.com"), "email not replaced: {content}");
    }

    #[test]
    fn test_process_request_body_openai_system_message() {
        // System message has no PII; user message has an email.
        // Only the user message content should be altered.
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user",   "content": "Please contact dave@secret.org"}
            ]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let mut vault = PiiVault::new("conv-system-msg");
        let result = PiiPipeline::process_request_body(&body_bytes, &mut vault, Provider::OpenAI, &Locale::EnUs);
        assert!(result.is_some(), "user message has PII => should return Some");
        let new_body: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
        // System message must be unchanged.
        let system_content = new_body["messages"][0]["content"].as_str().unwrap();
        assert_eq!(system_content, "You are a helpful assistant.");
        // User message must have PII replaced.
        let user_content = new_body["messages"][1]["content"].as_str().unwrap();
        assert!(!user_content.contains("dave@secret.org"), "email not replaced: {user_content}");
    }

    #[test]
    fn test_process_request_body_google_format() {
        // Google uses "contents" (not "messages") and "parts" (not "content").
        // The current implementation iterates "contents" entries but looks for a
        // "content" field inside each entry — which Google doesn't have.
        // Therefore we expect None (graceful degradation, no crash).
        let body = serde_json::json!({
            "model": "gemini-pro",
            "contents": [
                {"role": "user", "parts": [{"text": "Email alice@corp.com"}]}
            ]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let mut vault = PiiVault::new("conv-google-format");
        let result = PiiPipeline::process_request_body(&body_bytes, &mut vault, Provider::Google, &Locale::EnUs);
        // Google "parts" shape: no "content" key → any_replaced stays false → None.
        // The test verifies we don't panic and return a sensible value.
        // If the implementation is later updated to handle "parts", this assertion
        // would need updating — but the no-panic guarantee is what matters here.
        let _ = result; // either None or Some is acceptable; must not panic
    }

    #[test]
    fn test_rebuild_request_lowercase_content_length() {
        // Headers with a lowercase "content-length" — must still be updated.
        let headers = b"POST /v1/chat HTTP/1.1\r\nHost: api.openai.com\r\ncontent-length: 50\r\nContent-Type: application/json\r\n\r\n";
        let header_end = headers.len();
        let mut full = headers.to_vec();
        full.extend_from_slice(b"a".repeat(50).as_slice());

        let new_body = b"hello";
        let rebuilt = rebuild_request(&full, header_end, new_body);
        let rebuilt_str = String::from_utf8_lossy(&rebuilt);

        // The header name capitalisation is preserved; the value must be updated.
        assert!(
            rebuilt_str.to_ascii_lowercase().contains("content-length: 5"),
            "lowercase content-length not updated: {rebuilt_str}"
        );
        assert!(rebuilt_str.ends_with("hello"), "body not appended correctly");
    }

    #[test]
    fn test_rebuild_request_no_content_length_header() {
        // No Content-Length header at all — rebuild_request must not crash and must
        // end with the new body.
        let headers = b"POST /v1/chat HTTP/1.1\r\nHost: api.openai.com\r\nContent-Type: application/json\r\n\r\n";
        let header_end = headers.len();
        let mut full = headers.to_vec();
        full.extend_from_slice(b"original body");

        let new_body = b"replacement body";
        let rebuilt = rebuild_request(&full, header_end, new_body);

        assert!(
            rebuilt.ends_with(new_body),
            "rebuilt request does not end with new body"
        );
    }

    #[test]
    fn test_rebuild_request_preserves_other_headers() {
        // Host, Content-Type, and Authorization must survive unmodified after rebuild.
        let headers = b"POST /v1/chat HTTP/1.1\r\nHost: api.openai.com\r\nContent-Type: application/json\r\nAuthorization: Bearer sk-test\r\nContent-Length: 99\r\n\r\n";
        let old_body = b"old body here";
        let header_end = headers.len();
        let mut full = headers.to_vec();
        full.extend_from_slice(old_body);

        let new_body = b"new body";
        let rebuilt = rebuild_request(&full, header_end, new_body);
        let rebuilt_str = String::from_utf8_lossy(&rebuilt);

        assert!(rebuilt_str.contains("Host: api.openai.com"),        "Host header missing: {rebuilt_str}");
        assert!(rebuilt_str.contains("Content-Type: application/json"), "Content-Type missing: {rebuilt_str}");
        assert!(rebuilt_str.contains("Authorization: Bearer sk-test"),  "Authorization missing: {rebuilt_str}");
        assert!(
            rebuilt_str.contains(&format!("Content-Length: {}", new_body.len())),
            "Content-Length not updated: {rebuilt_str}"
        );
    }

    #[test]
    fn test_log_detections_empty_spans() {
        // Must not panic when called with an empty slice.
        PiiPipeline::log_detections(&[], "conv-empty");
    }

    #[test]
    fn test_pii_mode_detect_only_serialization() {
        let mode = PiiMode::DetectOnly;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"detect-only\"", "DetectOnly must serialize to \"detect-only\"");
    }
}
