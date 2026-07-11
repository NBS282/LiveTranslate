//! Native (in-process) GGUF speech model support: a static catalog of known
//! models, a minimal GGUF header reader for pre-load validation, and a
//! `ModelManager` that downloads, verifies, and loads them via `transcribe-cpp`.
//!
//! Dormant in Phase 2: nothing calls this from the live translation pipeline
//! yet (that's Phase 3). The only caller today is the debug-only
//! `dev_native_transcribe` command in `lib.rs`.

pub mod catalog;
pub mod gguf_meta;
pub mod manager;
