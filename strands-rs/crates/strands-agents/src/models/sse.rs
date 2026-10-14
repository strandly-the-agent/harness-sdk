//! Minimal server-sent-events parser for HTTP streaming model providers.

/// One parsed SSE message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    /// Value of the `event:` field, if the server sent one.
    pub event: Option<String>,
    /// Concatenated `data:` lines (joined with `\n`).
    pub data: String,
}

/// Incremental SSE parser: feed byte chunks, get complete events back.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    /// A parser with an empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of the response body and return every event it completed.
    pub fn push(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buffer.push_str(&chunk.replace("\r\n", "\n").replace('\r', "\n"));
        let mut out = Vec::new();
        while let Some(idx) = self.buffer.find("\n\n") {
            let block: String = self.buffer.drain(..idx + 2).collect();
            if let Some(event) = parse_block(block.trim_end_matches('\n')) {
                out.push(event);
            }
        }
        out
    }

    /// Flush a trailing event that was not terminated by a blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        let block = std::mem::take(&mut self.buffer);
        parse_block(block.trim_end_matches('\n'))
    }
}

fn parse_block(block: &str) -> Option<SseEvent> {
    let mut event: Option<String> = None;
    let mut data: Vec<&str> = Vec::new();
    for line in block.split('\n') {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => event = Some(value.to_string()),
            "data" => data.push(value),
            _ => {}
        }
    }
    if event.is_none() && data.is_empty() {
        return None;
    }
    Some(SseEvent {
        event,
        data: data.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_event_and_data_fields() {
        let mut p = SseParser::new();
        let events = p.push("event: response.created\ndata: {\"a\":1}\n\n");
        assert_eq!(
            events,
            vec![SseEvent {
                event: Some("response.created".into()),
                data: "{\"a\":1}".into()
            }]
        );
    }

    #[test]
    fn joins_multiline_data_and_ignores_comments() {
        let mut p = SseParser::new();
        let events = p.push(": keep-alive\ndata: a\ndata: b\n\n");
        assert_eq!(events[0].data, "a\nb");
        assert_eq!(events[0].event, None);
    }

    #[test]
    fn handles_chunk_boundaries_and_crlf() {
        let mut p = SseParser::new();
        assert!(p.push("event: x\r\ndata: {\"he").is_empty());
        let events = p.push("llo\":1}\r\n\r\ndata: [DONE]\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "{\"hello\":1}");
        assert_eq!(events[1].data, "[DONE]");
    }

    #[test]
    fn finish_flushes_unterminated_event() {
        let mut p = SseParser::new();
        assert!(p.push("data: tail").is_empty());
        assert_eq!(p.finish().unwrap().data, "tail");
        assert!(p.finish().is_none());
    }
}
