//! Native Rust artifact parsers.
//!
//! These read a source database directly, rather than going through iLEAPP's
//! lava output — used where iLEAPP's output is missing or lossy (see
//! `docs/spike-ileapp.md`). `address_book` still reads a file iLEAPP decrypted
//! and extracted; `camera_roll` now decrypts encrypted backups itself via the
//! native [`crate::crypto`] Decryptor (Phase 2), and reads plaintext backups raw.
//!
//! Provenance (architecture §10): these are `reference` implementations —
//! original Rust written from the reverse-engineered schema facts, not ports of
//! iLEAPP source.

pub mod address_book;
pub mod camera_roll;
pub mod chats;
