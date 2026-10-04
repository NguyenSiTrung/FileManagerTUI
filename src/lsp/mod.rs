//! Installed-server LSP client (FR-10).
//!
//! LSP is optional: unsupported files, missing servers, and startup/crash
//! errors never block editing. This module hosts the position-encoding
//! adapters (`positions`) now; bounded JSON-RPC transport and the client
//! state machine land in Phase 10 Tasks 2+.

pub mod positions;
