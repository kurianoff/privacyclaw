use chrono::Utc;
use uuid::Uuid;

pub fn new_uuid() -> String {
    Uuid::new_v4().to_string()
}

pub fn now_iso8601() -> String {
    Utc::now().to_rfc3339()
}

/// Format raw bytes as lowercase hex, truncated at `max` bytes.
/// Appends "...(N total bytes)" when the slice is longer than `max`.
pub fn fmt_chunk_hex(data: &[u8], max: usize) -> String {
    let take = data.len().min(max);
    let hex: String = data[..take]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(" ");
    if data.len() > max {
        format!("{}...({} total bytes)", hex, data.len())
    } else {
        hex
    }
}

/// Format an HTTP header block (raw text), redacting sensitive header values.
/// Replaces values of `authorization` and `x-api-key` with `[REDACTED]`.
/// Use this helper whenever logging raw HTTP headers to avoid credential leakage.
#[cfg_attr(not(test), allow(dead_code))]
pub fn fmt_headers(raw: &str) -> String {
    raw.lines()
        .map(|line| {
            if let Some((key, _val)) = line.split_once(':') {
                let lower = key.trim().to_lowercase();
                if lower == "authorization" || lower == "x-api-key" {
                    return format!("{}: [REDACTED]", key.trim());
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authorization_header_redacted() {
        let input = "Authorization: Bearer sk-abc123\nContent-Type: application/json";
        let output = fmt_headers(input);
        assert!(output.contains("Authorization"), "should still contain header name");
        assert!(!output.contains("sk-abc123"), "should not contain the token");
        assert!(output.contains("[REDACTED]"), "should contain [REDACTED]");
    }

    #[test]
    fn test_x_api_key_header_redacted() {
        let input = "X-Api-Key: my-secret-key\nContent-Type: text/plain";
        let output = fmt_headers(input);
        assert!(output.contains("X-Api-Key"), "should still contain header name");
        assert!(!output.contains("my-secret-key"), "should not contain the key value");
        assert!(output.contains("[REDACTED]"), "should contain [REDACTED]");
    }

    #[test]
    fn test_other_headers_not_redacted() {
        let input = "Content-Type: application/json\nAccept: text/plain";
        let output = fmt_headers(input);
        assert!(output.contains("Content-Type: application/json"), "Content-Type should be unchanged");
        assert!(output.contains("Accept: text/plain"), "Accept should be unchanged");
    }

    #[test]
    fn test_fmt_chunk_hex_truncates_at_256_bytes() {
        let data: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
        let output = fmt_chunk_hex(&data, 256);
        assert!(output.contains("..."), "output should contain '...' for truncated input");
        assert!(output.contains("1024 total bytes"), "output should contain total byte count");
        // Count hex tokens before "..."
        let before_ellipsis = output.split("...").next().unwrap_or("");
        let token_count = before_ellipsis.split_whitespace().count();
        assert!(token_count <= 256, "should have at most 256 hex tokens, got {}", token_count);
    }

    #[test]
    fn test_fmt_chunk_hex_short_input_not_truncated() {
        let data = &b"hello world"[..10];
        let output = fmt_chunk_hex(data, 256);
        assert!(!output.contains("..."), "output should not contain '...' for short input");
        let token_count = output.split_whitespace().count();
        assert_eq!(token_count, 10, "should have exactly 10 hex tokens, got {}", token_count);
    }

    #[test]
    fn test_fmt_chunk_hex_empty_input() {
        // Must not panic; any output (including empty string) is acceptable
        let output = fmt_chunk_hex(&[], 256);
        // no assertion on content — just verify no panic
        let _ = output;
    }
}
