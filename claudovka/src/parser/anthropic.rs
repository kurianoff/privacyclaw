use super::ParsedRequest;
use serde_json::Value;

pub fn parse_request(body: &[u8]) -> Option<ParsedRequest> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let model = v.get("model")?.as_str()?.to_string();

    let mut messages = Vec::new();

    // System prompt (top-level field in Anthropic API)
    if let Some(system) = v.get("system").and_then(|s| s.as_str()) {
        messages.push(super::Message {
            role: "system".to_string(),
            content: system.to_string(),
        });
    }

    if let Some(msgs) = v.get("messages").and_then(|m| m.as_array()) {
        for m in msgs {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user").to_string();
            let content = extract_content(m.get("content"));
            messages.push(super::Message { role, content });
        }
    }

    Some(ParsedRequest { model, messages })
}

fn extract_content(val: Option<&Value>) -> String {
    match val {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(extract_block)
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn extract_block(b: &Value) -> Option<String> {
    match b.get("type").and_then(|t| t.as_str()) {
        Some("text") => b
            .get("text")
            .and_then(|t| t.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        Some("thinking") => b
            .get("thinking")
            .and_then(|t| t.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| format!("[thinking] {}", s)),
        Some("tool_use") => {
            let name = b.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
            let input = b
                .get("input")
                .map(|i| serde_json::to_string(i).unwrap_or_default())
                .unwrap_or_default();
            Some(format!("[{}] {}", name, input))
        }
        Some("tool_result") => {
            let text = match b.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(blocks)) => blocks
                    .iter()
                    .filter_map(|b| {
                        if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                            b.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(""),
                _ => return None,
            };
            if text.is_empty() {
                return None;
            }
            Some(format!("[result] {}", text))
        }
        _ => None,
    }
}

/// Extract content delta from an Anthropic SSE event.
/// Captures text, tool-use input JSON, and thinking deltas.
/// Also emits a label on content_block_start for tool_use blocks.
pub fn extract_sse_delta(event: &crate::parser::sse::SseEvent) -> Option<String> {
    tracing::debug!(event_type = ?event.event_type, data_len = event.data.len(), "parser(anthropic): extract_sse_delta");
    let result = match event.event_type.as_deref() {
        Some("content_block_start") => {
            let v: Value = serde_json::from_str(&event.data).ok()?;
            let block_type = v.get("content_block")?.get("type")?.as_str()?;
            if block_type == "tool_use" {
                let name = v
                    .get("content_block")?
                    .get("name")?
                    .as_str()
                    .unwrap_or("unknown");
                return Some(format!("\n[{}] ", name));
            }
            None
        }
        Some("content_block_delta") => {
            let v: Value = serde_json::from_str(&event.data).ok()?;
            let delta = v.get("delta")?;
            match delta.get("type")?.as_str()? {
                "text_delta" => delta.get("text")?.as_str().map(|s| s.to_string()),
                "input_json_delta" => delta
                    .get("partial_json")?
                    .as_str()
                    .map(|s| s.to_string()),
                "thinking_delta" => delta
                    .get("thinking")?
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("[thinking] {}", s)),
                _ => None,
            }
        }
        _ => None,
    };
    tracing::debug!(delta_len = result.as_ref().map(|s| s.len()), "parser(anthropic): extract_sse_delta result");
    result
}

/// Extract token counts from a non-streaming Anthropic response.
#[allow(dead_code)]
pub fn extract_tokens(body: &[u8]) -> (Option<i64>, Option<i64>) {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return (None, None);
    };
    let tokens_in = v.get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(|t| t.as_i64());
    let tokens_out = v.get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(|t| t.as_i64());
    (tokens_in, tokens_out)
}

/// Extract full content from non-streaming Anthropic response.
pub fn extract_response_content(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let content = v.get("content")?.as_array()?;
    let text = content.iter()
        .filter_map(|b| {
            if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                b.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");
    Some(text)
}

pub fn extract_message_start_tokens(event: &crate::parser::sse::SseEvent) -> (Option<i64>, Option<i64>) {
    tracing::debug!(event_type = ?event.event_type, "parser(anthropic): extract_message_start_tokens");
    let result = if event.event_type.as_deref() == Some("message_start") {
        let Ok(v) = serde_json::from_str::<Value>(&event.data) else {
            return (None, None);
        };
        let usage = v.get("message").and_then(|m| m.get("usage"));
        let tokens_in = usage.and_then(|u| u.get("input_tokens")).and_then(|t| t.as_i64());
        let tokens_out = usage.and_then(|u| u.get("output_tokens")).and_then(|t| t.as_i64());
        (tokens_in, tokens_out)
    } else if event.event_type.as_deref() == Some("message_delta") {
        let Ok(v) = serde_json::from_str::<Value>(&event.data) else {
            return (None, None);
        };
        let tokens_out = v.get("usage")
            .and_then(|u| u.get("output_tokens"))
            .and_then(|t| t.as_i64());
        (None, tokens_out)
    } else {
        (None, None)
    };
    tracing::debug!(tokens_in = ?result.0, tokens_out = ?result.1, "parser(anthropic): extract_message_start_tokens result");
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::sse::SseEvent;

    fn event(event_type: Option<&str>, data: &str) -> SseEvent {
        SseEvent { event_type: event_type.map(|s| s.to_string()), data: data.to_string() }
    }

    #[test]
    fn test_parse_model_and_messages() {
        let body = br#"{"model":"claude-3-5-sonnet-20241022","max_tokens":1024,"messages":[{"role":"user","content":"Hello"}]}"#;
        let r = parse_request(body).unwrap();
        assert_eq!(r.model, "claude-3-5-sonnet-20241022");
        assert_eq!(r.messages.len(), 1);
        assert_eq!(r.messages[0].role, "user");
        assert_eq!(r.messages[0].content, "Hello");
    }

    #[test]
    fn test_parse_tool_use_content_array() {
        let body = serde_json::json!({
            "model": "claude-3",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "hello"},
                {"type": "text", "text": " world"}
            ]}]
        }).to_string();
        let r = parse_request(body.as_bytes()).unwrap();
        assert_eq!(r.messages[0].content, "hello world");
    }

    #[test]
    fn test_parse_image_block_elided() {
        let body = serde_json::json!({
            "model": "claude-3",
            "messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "aGVsbG8="}},
                {"type": "text", "text": "describe this"}
            ]}]
        }).to_string();
        let r = parse_request(body.as_bytes()).unwrap();
        assert!(r.messages[0].content.contains("describe this"));
        assert!(!r.messages[0].content.contains("aGVsbG8="));
    }

    #[test]
    fn test_parse_malformed_json_returns_none() {
        assert!(parse_request(b"not valid json {{{").is_none());
    }

    #[test]
    fn test_parse_missing_model_returns_none() {
        let body = br#"{"messages":[{"role":"user","content":"hi"}]}"#;
        assert!(parse_request(body).is_none());
    }

    #[test]
    fn test_extract_delta_from_content_block_delta() {
        let e = event(Some("content_block_delta"), r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}"#);
        let delta = extract_sse_delta(&e);
        assert_eq!(delta.as_deref(), Some("hello"));
    }

    #[test]
    fn test_extract_tokens_from_message_start() {
        let e = event(Some("message_start"), r#"{"type":"message_start","message":{"usage":{"input_tokens":100,"output_tokens":0}}}"#);
        let (ti, to) = extract_message_start_tokens(&e);
        assert_eq!(ti, Some(100));
        assert_eq!(to, Some(0));
    }

    #[test]
    fn test_extract_tokens_from_message_delta() {
        let e = event(Some("message_delta"), r#"{"type":"message_delta","usage":{"output_tokens":50}}"#);
        let (ti, to) = extract_message_start_tokens(&e);
        assert_eq!(ti, None);
        assert_eq!(to, Some(50));
    }

    #[test]
    fn test_empty_delta_returns_none() {
        let e = event(Some("content_block_delta"), r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":""}}"#);
        let _ = extract_sse_delta(&e);
    }

    #[test]
    fn test_non_delta_event_returns_none() {
        let e = event(Some("message_stop"), r#"{"type":"message_stop"}"#);
        assert!(extract_sse_delta(&e).is_none());
    }
}
