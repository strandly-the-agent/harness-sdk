//! Inputs and outputs of sandbox operations (TS `sandbox/types.ts`).

/// Which stream a [`StreamChunk`] came from (TS `StreamType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StreamType {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// A chunk of output from a running command (TS `StreamChunk`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StreamChunk {
    /// The decoded text.
    pub data: String,
    /// The stream it came from.
    pub stream_type: StreamType,
}

impl StreamChunk {
    /// A chunk of `data` from `stream_type`.
    pub fn new(data: impl Into<String>, stream_type: StreamType) -> Self {
        Self {
            data: data.into(),
            stream_type,
        }
    }
}

/// Metadata about a file or directory in a sandbox (TS `FileInfo`).
///
/// `is_dir` and `size` are `None` when the backend can't determine them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileInfo {
    /// The entry name (not a full path).
    pub name: String,
    /// Whether the entry is a directory.
    pub is_dir: Option<bool>,
    /// The size in bytes.
    pub size: Option<u64>,
}

impl FileInfo {
    /// An entry with unknown kind and size.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_dir: None,
            size: None,
        }
    }

    /// Sets whether the entry is a directory.
    #[must_use]
    pub fn with_is_dir(mut self, is_dir: bool) -> Self {
        self.is_dir = Some(is_dir);
        self
    }

    /// Sets the size in bytes.
    #[must_use]
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }
}

/// A file produced by code execution (TS `OutputFile`); shell sandboxes produce none.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OutputFile {
    /// The file name.
    pub name: String,
    /// The file content.
    pub content: Vec<u8>,
    /// The MIME type.
    pub mime_type: String,
}

impl OutputFile {
    /// An output file.
    pub fn new(name: impl Into<String>, content: Vec<u8>, mime_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            content,
            mime_type: mime_type.into(),
        }
    }
}

/// The result of command or code execution (TS `ExecutionResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExecutionResult {
    /// The exit code; non-zero means the command failed.
    pub exit_code: i32,
    /// The complete standard output.
    pub stdout: String,
    /// The complete standard error.
    pub stderr: String,
    /// Files the execution produced.
    pub output_files: Vec<OutputFile>,
}

impl ExecutionResult {
    /// A result with no output files.
    pub fn new(exit_code: i32, stdout: impl Into<String>, stderr: impl Into<String>) -> Self {
        Self {
            exit_code,
            stdout: stdout.into(),
            stderr: stderr.into(),
            output_files: Vec::new(),
        }
    }

    /// Sets the output files.
    #[must_use]
    pub fn with_output_files(mut self, output_files: Vec<OutputFile>) -> Self {
        self.output_files = output_files;
        self
    }
}

/// An item of a streaming execution (TS `StreamChunk | ExecutionResult`): chunks as output
/// arrives, then exactly one result.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExecutionEvent {
    /// Output that arrived.
    StreamChunk(StreamChunk),
    /// The final result.
    ExecutionResult(ExecutionResult),
}
