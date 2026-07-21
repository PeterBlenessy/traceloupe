//! Safety Scan: local-only classification of backup text into the Forensic 9
//! taxonomy (CONTEXT.md §Safety Scan, ADR 0002, docs/safety-scan-plan.md).
//!
//! The model is a *stateless classifier*: deterministic code in this module
//! selects text from the parse cache, feeds it one [`chunker::Chunk`] at a
//! time, and persists verdicts to the [`crate::analysis`] store. Nothing here
//! ever gives the model tools, queries, or memory across calls.

pub mod chunker;
pub mod client;
pub mod engine;
pub mod prompt;
pub mod summary;
