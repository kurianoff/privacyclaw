use super::ParsedRequest;
use crate::parser::sse::SseEvent;
use serde_json::Value;

pub fn parse_request(body: &[u8]) -> Option<ParsedRequest> {
    let v: Value = serde_json::from_slice(body).ok()?;

    // Gemini API: model is in the URL, not the body — we'll use "gemini" as fallback
    let model = v.get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("gemini")
        .to_string();

    let messages = v.get("contents")?.as_array()?.iter().map(|m| {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user").to_string();
        let content = m.get("parts")
            .and_then(|p| p.as_array())
            .map(|parts| {
                parts.iter()
                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        super::Message { role, content }
    }).collect();

    Some(ParsedRequest { model, messages })
}

/// Extract text delta from a Google Gemini SSE event.
pub fn extract_sse_delta(event: &SseEvent) -> Option<String> {
    tracing::debug!(event_type = ?event.event_type, data_len = event.data.len(), "parser(google): extract_sse_delta");
    let v: Value = serde_json::from_str(&event.data).ok()?;
    let result = v.get("candidates")?
        .as_array()?
        .first()?
        .get("content")?
        .get("parts")?
        .as_array()?
        .first()?
        .get("text")?
        .as_str()
        .map(|s| s.to_string());
    tracing::debug!(delta_len = result.as_ref().map(|s| s.len()), "parser(google): extract_sse_delta result");
    result
}

/// Extract token counts from a Google Gemini response.
#[allow(dead_code)]
pub fn extract_tokens(body: &[u8]) -> (Option<i64>, Option<i64>) {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return (None, None);
    };
    let meta = v.get("usageMetadata");
    let tokens_in = meta.and_then(|m| m.get("promptTokenCount")).and_then(|t| t.as_i64());
    let tokens_out = meta.and_then(|m| m.get("candidatesTokenCount")).and_then(|t| t.as_i64());
    (tokens_in, tokens_out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::sse::SseEvent;

    #[test]
    fn test_google_parse_contents_field() {
        let body = serde_json::json!({
            "contents": [{"role": "user", "parts": [{"text": "Hello Gemini"}]}]
        }).to_string();
        let r = parse_request(body.as_bytes()).unwrap();
        assert_eq!(r.messages.len(), 1);
        assert_eq!(r.messages[0].content, "Hello Gemini");
        assert_eq!(r.model, "gemini");
    }

    #[test]
    fn test_google_extract_delta_from_candidates() {
        let e = SseEvent {
            event_type: None,
            data: r#"{"candidates":[{"content":{"parts":[{"text":"hello"}]}}]}"#.to_string(),
        };
        assert_eq!(extract_sse_delta(&e).as_deref(), Some("hello"));
    }
}
