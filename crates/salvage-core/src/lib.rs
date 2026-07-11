//! salvage-core: UI-agnostic core for the local iOS backup browser.
//!
//! This crate has no Tauri or UI dependencies. It exposes the use-cases the
//! shell calls over IPC: backup discovery, import orchestration, and cached
//! artifact queries. See architecture.md §5.

pub mod cache;
pub mod discovery;
mod error;

pub use error::{Error, Result};
