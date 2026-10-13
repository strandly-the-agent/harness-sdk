//! Strands Agents SDK for Rust.
//!
//! A model-driven framework for building AI agents: a [`Model`](models::Model), a system prompt,
//! and a list of [`Tool`](tools::Tool)s composed into an agent loop.

pub mod error;
pub mod types;

pub use error::{Error, Result};
