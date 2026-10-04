//! Installed-server LSP client (FR-10).
//!
//! LSP is optional: unsupported files, missing servers, and startup/crash
//! errors never block editing. `positions` adapts position encodings,
//! `transport` bounds the byte-level frames, and `client` drives the
//! request/restart/shutdown lifecycle over one server generation at a time.

pub mod client;
pub mod positions;
pub mod transport;
