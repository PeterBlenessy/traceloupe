//! Native Rust artifact parsers.
//!
//! These read a decrypted source database directly, rather than going through
//! iLEAPP's lava output. The MVP still relies on iLEAPP to *decrypt and
//! extract* the file (so no native Decryptor is needed yet); a native parser
//! is used where iLEAPP's lava output is missing or lossy — see
//! `docs/spike-ileapp.md`. Phase 2 will add native decryption underneath.
//!
//! Provenance (architecture §10): these are `reference` implementations —
//! original Rust written from the reverse-engineered schema facts, not ports of
//! iLEAPP source.

pub mod address_book;
pub mod camera_roll;
pub mod chats;
