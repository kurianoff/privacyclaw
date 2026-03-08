/// Generic SSE parser for a stream of bytes.
///
/// Buffers partial chunks and emits complete events.
pub struct SseParser {
    buffer: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct SseEvent {
    pub event_type: Option<String>,
    pub data: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Feed raw bytes; returns any complete SSE events parsed.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        tracing::debug!(input_bytes = bytes.len(), buf_len = self.buffer.len(), "sse: push called");
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();

        // find_double_newline returns (block_end, drain_end) where:
        //   block_end: index of start of separator (block content is buf[..block_end])
        //   drain_end: index after separator (drain buf[..drain_end])
        while let Some((block_end, drain_end)) = find_double_newline(&self.buffer) {
            let block = self.buffer[..block_end].to_vec();
            self.buffer.drain(..drain_end);
            if let Some(event) = parse_event_block(&block) {
                tracing::debug!(
                    event_type = ?event.event_type,
                    data_len = event.data.len(),
                    "sse: event parsed"
                );
                events.push(event);
            }
        }

        events
    }

    pub fn is_done_sentinel(event: &SseEvent) -> bool {
        let done = event.data.trim() == "[DONE]";
        if done {
            tracing::debug!("sse: [DONE] sentinel detected");
        }
        done
    }
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `(block_end, drain_end)` where `block_end` is the index of the start
/// of the separator and `drain_end` is the index just past the separator.
fn find_double_newline(buf: &[u8]) -> Option<(usize, usize)> {
    for i in 0..buf.len().saturating_sub(1) {
        // Check \r\n\r\n first to avoid partial match on \n\n within it
        if i + 3 < buf.len()
            && buf[i] == b'\r'
            && buf[i + 1] == b'\n'
            && buf[i + 2] == b'\r'
            && buf[i + 3] == b'\n'
        {
            return Some((i, i + 4));
        }
        if buf[i] == b'\n' && buf[i + 1] == b'\n' {
            return Some((i, i + 2));
        }
    }
    None
}

fn parse_event_block(block: &[u8]) -> Option<SseEvent> {
    let text = std::str::from_utf8(block).ok()?;
    let mut event_type = None;
    let mut data_parts = Vec::new();

    for line in text.lines() {
        if let Some(val) = line.strip_prefix("event:") {
            event_type = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("data:") {
            data_parts.push(val.trim().to_string());
        }
        // Ignore id:, retry:, comments (:)
    }

    if data_parts.is_empty() {
        return None;
    }

    Some(SseEvent {
        event_type,
        data: data_parts.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_event() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: {\"text\":\"hello\"}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, r#"{"text":"hello"}"#);
    }

    #[test]
    fn test_partial_chunks() {
        let mut parser = SseParser::new();
        let e1 = parser.push(b"data: hel");
        assert!(e1.is_empty());
        let e2 = parser.push(b"lo\n\n");
        assert_eq!(e2.len(), 1);
        assert_eq!(e2[0].data, "hello");
    }

    #[test]
    fn test_event_type() {
        let mut parser = SseParser::new();
        let events = parser.push(b"event: content_block_delta\ndata: {}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type.as_deref(), Some("content_block_delta"));
    }

    #[test]
    fn test_done_sentinel() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: [DONE]\n\n");
        assert_eq!(events.len(), 1);
        assert!(SseParser::is_done_sentinel(&events[0]));
    }

    #[test]
    fn test_multiple_events() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: a\n\ndata: b\n\ndata: c\n\n");
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn test_empty_event_skipped() {
        let mut parser = SseParser::new();
        let events = parser.push(b"\n\ndata: hello\n\n");
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_lf_only_line_endings() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: hello\n\n");
        assert_eq!(events.len(), 1, "should emit exactly 1 event");
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn test_comment_lines_ignored() {
        let mut parser = SseParser::new();
        let events = parser.push(b": this is a comment\ndata: content\n\n");
        assert_eq!(events.len(), 1, "comment should not produce an extra event");
        assert_eq!(events[0].data, "content");
    }

    #[test]
    fn test_multi_line_data_concatenated() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: hello\ndata: world\n\n");
        assert_eq!(events.len(), 1, "multi-line data should produce exactly 1 event");
        assert_eq!(events[0].data, "hello\nworld");
    }

    #[test]
    fn test_event_type_without_data_skipped() {
        let mut parser = SseParser::new();
        let events = parser.push(b"event: ping\n\n");
        assert_eq!(events.len(), 0, "event with no data field should be skipped");
    }

    #[test]
    fn test_1000_events_in_one_push() {
        let mut stream = Vec::new();
        for i in 0..1000 {
            stream.extend_from_slice(format!("data: event_{}\n\n", i).as_bytes());
        }
        let mut parser = SseParser::new();
        let events = parser.push(&stream);
        assert_eq!(events.len(), 1000, "should parse all 1000 events");
    }

    #[test]
    fn test_byte_by_byte_same_as_full_push() {
        let stream: Vec<u8> = b"data: a\n\ndata: b\n\ndata: c\n\ndata: d\n\ndata: e\n\n".to_vec();

        let mut parser_a = SseParser::new();
        let events_a = parser_a.push(&stream);

        let mut parser_b = SseParser::new();
        let mut events_b = Vec::new();
        for byte in &stream {
            events_b.extend(parser_b.push(&[*byte]));
        }

        let data_a: Vec<&str> = events_a.iter().map(|e| e.data.as_str()).collect();
        let data_b: Vec<&str> = events_b.iter().map(|e| e.data.as_str()).collect();
        assert_eq!(data_a, data_b, "full push and byte-by-byte push should produce same events");
    }

    #[test]
    fn test_message_stop_event_parsed_correctly() {
        let mut parser = SseParser::new();
        let events = parser.push(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
        assert_eq!(events.len(), 1, "should emit exactly 1 event");
        assert_eq!(events[0].event_type.as_deref(), Some("message_stop"));
        assert!(events[0].data.contains("message_stop"), "data should contain 'message_stop'");
    }

    #[test]
    fn test_large_stream_throughput() {
        let mut stream = Vec::new();
        for i in 0..5000 {
            stream.extend_from_slice(format!("data: {{\"index\":{},\"payload\":\"xxxxxxxxxxxxxxxxxxxxxxxxxx\"}}\n\n", i).as_bytes());
        }
        let start = std::time::Instant::now();
        let mut parser = SseParser::new();
        let events = parser.push(&stream);
        let elapsed_ms = start.elapsed().as_millis();
        assert_eq!(events.len(), 5000, "should parse all 5000 events");
        assert!(elapsed_ms < 100, "parsing 5000 events took {}ms, expected < 100ms", elapsed_ms);
    }
}
