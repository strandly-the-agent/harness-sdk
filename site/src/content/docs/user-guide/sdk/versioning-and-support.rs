//! Rust snippets for `versioning-and-support.mdx`.

mod deprecation {
    // --8<-- [start:deprecation_warning]
    #[deprecated(
        since = "1.1.0",
        note = "deprecated_function() will be removed in 2.0.0. \
                Use new_function() instead. See: https://strandsagents.com/..."
    )]
    pub fn deprecated_function() {}
    // --8<-- [end:deprecation_warning]
}
