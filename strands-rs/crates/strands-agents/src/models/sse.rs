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
///
/// Chunk boundaries are arbitrary bytes, so an incomplete UTF-8 sequence or a `\r` that may still
/// be followed by `\n` is held back until the next chunk arrives.
#[derive(Debug, Default)]
pub struct SseParser {
    /// Decoded, newline-normalised text awaiting an event terminator.
    buffer: String,
    /// Trailing bytes that do not yet form a complete UTF-8 sequence.
    partial: Vec<u8>,
}

impl SseParser {
    /// A parser with an empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of the response body and return every event it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let decoded = self.decode(chunk);
        self.buffer.push_str(&decoded);
        self.normalize(false);
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
        if !self.partial.is_empty() {
            let tail = String::from_utf8_lossy(&std::mem::take(&mut self.partial)).into_owned();
            self.buffer.push_str(&tail);
        }
        self.normalize(true);
        let block = std::mem::take(&mut self.buffer);
        parse_block(block.trim_end_matches('\n'))
    }

    /// Decode `chunk` after any held-back bytes, keeping an incomplete trailing sequence.
    fn decode(&mut self, chunk: &[u8]) -> String {
        self.partial.extend_from_slice(chunk);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.partial) {
                Ok(text) => {
                    out.push_str(text);
                    self.partial.clear();
                    return out;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.partial[..valid]).unwrap_or_default());
                    match e.error_len() {
                        // Truly invalid bytes: replace them and keep going.
                        Some(len) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            self.partial.drain(..valid + len);
                        }
                        // Incomplete tail: hold it back for the next chunk.
                        None => {
                            self.partial.drain(..valid);
                            return out;
                        }
                    }
                }
            }
        }
    }

    /// Normalise `\r\n` and lone `\r` in the buffer, holding back a trailing `\r` unless `final_`.
    fn normalize(&mut self, final_: bool) {
        if !self.buffer.contains('\r') {
            return;
        }
        let hold_cr = !final_ && self.buffer.ends_with('\r');
        let body = if hold_cr {
            &self.buffer[..self.buffer.len() - 1]
        } else {
            &self.buffer[..]
        };
        let mut normalized = body.replace("\r\n", "\n").replace('\r', "\n");
        if hold_cr {
            normalized.push('\r');
        }
        self.buffer = normalized;
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
        let events = p.push(b"event: response.created\ndata: {\"a\":1}\n\n");
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
        let events = p.push(b": keep-alive\ndata: a\ndata: b\n\n");
        assert_eq!(events[0].data, "a\nb");
        assert_eq!(events[0].event, None);
    }

    #[test]
    fn handles_chunk_boundaries_and_crlf() {
        let mut p = SseParser::new();
        assert!(p.push(b"event: x\r\ndata: {\"he").is_empty());
        let events = p.push(b"llo\":1}\r\n\r\ndata: [DONE]\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "{\"hello\":1}");
        assert_eq!(events[1].data, "[DONE]");
    }

    #[test]
    fn finish_flushes_unterminated_event() {
        let mut p = SseParser::new();
        assert!(p.push(b"data: tail").is_empty());
        assert_eq!(p.finish().unwrap().data, "tail");
        assert!(p.finish().is_none());
    }

    #[test]
    fn multi_byte_char_split_across_chunks_is_preserved() {
        let mut p = SseParser::new();
        assert!(p.push(b"data: caf").is_empty());
        assert!(p.push(b"\xc3").is_empty());
        let events = p.push(b"\xa9\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "café");
    }

    #[test]
    fn crlf_split_across_chunks_keeps_one_event() {
        let mut p = SseParser::new();
        assert!(p.push(b"event: content_block_delta\r").is_empty());
        let events = p.push(b"\ndata: {\"a\":1}\r\n\r\n");
        assert_eq!(
            events,
            vec![SseEvent {
                event: Some("content_block_delta".into()),
                data: "{\"a\":1}".into()
            }]
        );
    }

    #[test]
    fn finish_flushes_incomplete_utf8_tail_lossily() {
        let mut p = SseParser::new();
        assert!(p.push(b"data: caf\xc3").is_empty());
        assert_eq!(p.finish().unwrap().data, "caf\u{fffd}");
    }

    #[test]
    fn invalid_utf8_bytes_become_replacement_chars() {
        let mut p = SseParser::new();
        let events = p.push(b"data: a\xffb\n\n");
        assert_eq!(events[0].data, "a\u{fffd}b");
    }
}
