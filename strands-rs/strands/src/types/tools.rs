//! Tool data types (TS `tools/types.ts`).

open_enum! {
    /// The outcome of a tool call (TS `ToolResultStatus`).
    pub enum ToolResultStatus / UnknownToolResultStatus {
        /// The tool succeeded.
        Success => "success",
        /// The tool failed; the content describes the error.
        Error => "error",
    }
}
