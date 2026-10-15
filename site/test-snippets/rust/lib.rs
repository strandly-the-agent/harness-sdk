//! See Cargo.toml. Snippet bodies are scoping functions that are never called, so the usual
//! unused-code lints are off here.
#![allow(dead_code, unused_imports, unused_variables, unused_mut, clippy::all)]

include!(concat!(env!("OUT_DIR"), "/snippets.rs"));
