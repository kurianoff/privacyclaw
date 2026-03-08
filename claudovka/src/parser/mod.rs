pub mod anthropic;
pub mod google;
pub mod openai;
pub mod sse;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedRequest {
    pub model: String,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAI,
    Google,
    Unknown,
}

impl Provider {
    pub fn from_host(host: &str) -> Self {
        if host.contains("anthropic.com") {
            Provider::Anthropic
        } else if host.contains("openai.com") {
            Provider::OpenAI
        } else if host.contains("googleapis.com") {
            Provider::Google
        } else {
            Provider::Unknown
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAI => "openai",
            Provider::Google => "google",
            Provider::Unknown => "unknown",
        }
    }
}

pub fn parse_request(provider: Provider, body: &[u8]) -> Option<ParsedRequest> {
    let result = match provider {
        Provider::Anthropic => anthropic::parse_request(body),
        Provider::OpenAI => openai::parse_request(body),
        Provider::Google => google::parse_request(body),
        Provider::Unknown => {
            // Best-effort: try OpenAI format
            openai::parse_request(body)
        }
    };
    match &result {
        Some(parsed) => tracing::info!(
            provider = provider.as_str(),
            body_bytes = body.len(),
            messages = parsed.messages.len(),
            model = %parsed.model,
            "parser: parse_request ok"
        ),
        None => tracing::debug!(
            provider = provider.as_str(),
            body_bytes = body.len(),
            "parser: parse_request failed"
        ),
    }
    result
}

pub fn extract_sse_delta(provider: Provider, event: &sse::SseEvent) -> Option<String> {
    match provider {
        Provider::Anthropic => anthropic::extract_sse_delta(event),
        Provider::OpenAI => openai::extract_sse_delta(event),
        Provider::Google => google::extract_sse_delta(event),
        Provider::Unknown => openai::extract_sse_delta(event),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unknown_provider_falls_back_to_openai_format() {
        let body = br#"{"model":"some-model","messages":[{"role":"user","content":"hi"}]}"#;
        let result = parse_request(Provider::Unknown, body);
        assert!(result.is_some(), "Unknown provider should try OpenAI format");
        assert_eq!(result.unwrap().model, "some-model");
    }

    #[test]
    fn test_parse_request_scaling_182_turns_under_50ms() {
        let mut messages = Vec::new();
        for i in 0..182 {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            let content = format!("Message {} with some content to make it realistic. This is a longer message that contains multiple sentences to simulate real-world usage patterns.", i);
            messages.push(serde_json::json!({"role": role, "content": content}));
        }
        let body = serde_json::json!({
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 1024,
            "messages": messages
        }).to_string();
        let body_bytes = body.as_bytes();
        let start = std::time::Instant::now();
        let result = parse_request(Provider::Anthropic, body_bytes);
        let elapsed = start.elapsed();
        assert!(result.is_some());
        assert_eq!(result.unwrap().messages.len(), 182);
        assert!(elapsed.as_millis() < 50, "Parse took {}ms, expected < 50ms", elapsed.as_millis());
    }
}
