//! salvage-core: UI-agnostic core for the local iOS backup browser.
//!
//! This crate has no Tauri or UI dependencies. It exposes the use-cases the
//! shell calls over IPC: backup discovery, import orchestration, and cached
//! artifact queries. See architecture.md §5.

pub mod cache;
pub mod crypto;
pub mod discovery;
pub mod engine;
mod error;
pub mod import;
pub mod install;
pub mod normalize;
pub mod parsers;
pub mod query;
pub mod sidecar;

pub use error::{Error, Result};
