//! The code's `print()` output and the cap on the text returned to the model (TS `Output`, `cap`).

use std::borrow::Cow;

use monty_types::{MontyException, PrintWriterCallback};

/// Cap on the text returned to the model; a runaway `print` should not blow up the context window.
pub(crate) const MAX_OUTPUT_CHARS: usize = 200_000;

/// Collects the code's `print()` output, capped at [`MAX_OUTPUT_CHARS`].
#[derive(Debug, Default)]
pub(crate) struct Output {
    raw: String,
    size: usize,
}

impl Output {
    pub(crate) fn write(&mut self, text: &str) {
        // One char past the cap is kept so `cap` still sees that the output overflowed.
        let room = (MAX_OUTPUT_CHARS + 1).saturating_sub(self.size);
        for character in text.chars().take(room) {
            self.raw.push(character);
            self.size += 1;
        }
    }

    pub(crate) fn text(&self) -> String {
        cap(&self.raw, MAX_OUTPUT_CHARS)
    }

    /// The error message, preceded by whatever the code printed before failing. Both share the
    /// cap: the error keeps at least half, the output the rest.
    pub(crate) fn with_error(&self, message: &str) -> String {
        let message_limit = MAX_OUTPUT_CHARS
            .saturating_sub(self.size)
            .max(MAX_OUTPUT_CHARS / 2);
        let capped = cap(message, message_limit);
        let printed = cap(
            &self.raw,
            MAX_OUTPUT_CHARS - capped.chars().count().min(message_limit),
        );
        if printed.is_empty() {
            capped
        } else {
            format!("{printed}\n\n{capped}")
        }
    }

    #[cfg(test)]
    pub(crate) fn buffered_chars(&self) -> usize {
        self.size
    }
}

impl PrintWriterCallback for Output {
    fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        self.write(&output);
        Ok(())
    }

    fn stdout_push(&mut self, end: char) -> Result<(), MontyException> {
        self.write(end.encode_utf8(&mut [0; 4]));
        Ok(())
    }
}

pub(crate) fn cap(raw: &str, limit: usize) -> String {
    match raw.char_indices().nth(limit) {
        None => raw.trim().to_owned(),
        Some((end, _)) => format!("{}\n[output truncated at {limit} characters]", raw[..end].trim()),
    }
}
