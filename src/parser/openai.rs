use super::ParsedRequest;
use crate::parser::sse::SseEvent;
use serde_json::Value;

pub fn parse_request(body: &[u8]) -> Option<ParsedRequest> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let model = v.get("model")?.as_str()?.to_string();

    let messages = v.get("messages")?.as_array()?.iter().map(|m| {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user").to_string();
        let content = extract_content(m.get("content"));
        super::Message { role, content }
    }).collect();

    Some(ParsedRequest { model, messages })
}

fn extract_content(val: Option<&Value>) -> String {
    match val {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                if p.get("type").and_then(|t| t.as_str()) == Some("text") {
                    p.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Extract text delta from an OpenAI SSE event.
pub fn extract_sse_delta(event: &SseEvent) -> Option<String> {
    tracing::debug!(event_type = ?event.event_type, data_len = event.data.len(), "parser(openai): extract_sse_delta");
    if SseEvent::data_is_done(&event.data) {
        tracing::debug!("parser(openai): extract_sse_delta: [DONE] sentinel");
        return None;
    }
    let v: Value = serde_json::from_str(&event.data).ok()?;
    let result = v.get("choices")?
        .as_array()?
        .first()?
        .get("delta")?
        .get("content")?
        .as_str()
        .map(|s| s.to_string());
    tracing::debug!(delta_len = result.as_ref().map(|s| s.len()), "parser(openai): extract_sse_delta result");
    result
}

impl SseEvent {
    pub fn data_is_done(data: &str) -> bool {
        data.trim() == "[DONE]"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::sse::SseEvent;

    #[test]
    fn test_openai_parse_model_and_messages() {
        let body = br#"{"model":"gpt-4o","messages":[{"role":"user","content":"Hello"}]}"#;
        let r = parse_request(body).unwrap();
        assert_eq!(r.model, "gpt-4o");
        assert_eq!(r.messages.len(), 1);
        assert_eq!(r.messages[0].content, "Hello");
    }

    #[test]
    fn test_openai_extract_delta_from_choices() {
        let e = SseEvent {
            event_type: None,
            data: r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#.to_string(),
        };
        assert_eq!(extract_sse_delta(&e).as_deref(), Some("hi"));
    }

    #[test]
    fn test_openai_done_sentinel_is_none() {
        let e = SseEvent { event_type: None, data: "[DONE]".to_string() };
        assert!(SseEvent::data_is_done(&e.data));
        assert!(extract_sse_delta(&e).is_none());
    }
}
