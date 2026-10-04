//! Versioned text-document synchronization (FR-10): `didOpen`/`didChange`/
//! `didSave`/`didClose` with monotonic per-document versions, the negotiated
//! synchronization kind, and stable `file://` URIs.
//!
//! The structures here are pure models — they build JSON-RPC `params` bodies
//! and remember the exact mirror text each server holds, so incremental
//! changes diff against the server's view (never the editor's local copy)
//! and a restarted server can be re-opened without touching the document
//! store.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::lsp::positions::{byte_to_lsp, PositionEncoding};

/// How the server wants `didChange` payloads, from `initialize`'s
/// `capabilities.textDocumentSync` (`change` when the object form is used).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextSyncKind {
    /// No synchronization — the server takes no document text.
    #[default]
    None,
    /// Whole-document content on every change.
    Full,
    /// Ranged content changes.
    Incremental,
}

impl TextSyncKind {
    fn from_kind_number(n: Option<u64>) -> Self {
        match n.unwrap_or(0) {
            1 => Self::Full,
            2 => Self::Incremental,
            _ => Self::None,
        }
    }
}

/// The negotiated document-sync surface, captured once per server generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextSync {
    /// Server wants `didOpen`/`didClose`. Documents cannot sync without it —
    /// the server never learns their text.
    pub open_close: bool,
    pub change: TextSyncKind,
    /// Server wants `didSave`.
    pub save: bool,
    /// `didSave` should carry the document text (`SaveOptions.includeText`).
    pub save_include_text: bool,
}

impl TextSync {
    /// Parse `capabilities.textDocumentSync`: a bare `TextDocumentSyncKind`
    /// number implies open/close; the object form names each switch.
    pub fn from_capability(value: Option<&Value>) -> Self {
        match value {
            Some(Value::Number(n)) => {
                let change = TextSyncKind::from_kind_number(n.as_u64());
                Self {
                    open_close: change != TextSyncKind::None,
                    change,
                    ..Self::default()
                }
            }
            Some(v @ Value::Object(_)) => {
                let save = v.get("save");
                Self {
                    open_close: v.get("openClose").and_then(Value::as_bool).unwrap_or(false),
                    change: TextSyncKind::from_kind_number(v.get("change").and_then(Value::as_u64)),
                    save: save.is_some_and(|s| s.as_bool().unwrap_or(true) || s.is_object()),
                    save_include_text: save
                        .and_then(|s| s.get("includeText"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                }
            }
            _ => Self::default(),
        }
    }
}

/// Stable `file://` URI for an absolute path. `/` and unreserved characters
/// pass through; everything else (spaces, non-ASCII, delimiters) is UTF-8
/// percent-encoded — the same path always maps to the same URI.
pub fn uri_for_path(path: &Path) -> String {
    const UNRESERVED: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~/";
    let mut uri = String::from("file://");
    for byte in path.as_os_str().to_string_lossy().as_bytes() {
        if UNRESERVED.contains(byte) {
            uri.push(*byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// `{line, character}` position for a flat byte offset in the negotiated
/// encoding. `byte` must be a char boundary (callers produce boundaries only).
fn position_at(text: &str, byte: usize, encoding: PositionEncoding) -> Value {
    let line = text[..byte].bytes().filter(|b| *b == b'\n').count();
    let line_start = text[..byte].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_text = &text[line_start..byte];
    let character = byte_to_lsp(line_text, line_text.len(), encoding).unwrap_or(0);
    json!({ "line": line, "character": character })
}

/// One minimal `TextDocumentContentChangeEvent` covering the difference
/// between `old` and `new` (common prefix + common suffix trimmed), or
/// `None` when the texts are identical. Boundaries are char-safe by
/// construction.
pub fn incremental_change(old: &str, new: &str, encoding: PositionEncoding) -> Option<Value> {
    if old == new {
        return None;
    }
    let mut prefix = 0usize;
    for (a, b) in old.bytes().zip(new.bytes()) {
        if a != b {
            break;
        }
        prefix += 1;
    }
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let mut suffix = 0usize;
    while suffix < old.len() - prefix && suffix < new.len() - prefix {
        let ob = old.as_bytes()[old.len() - 1 - suffix];
        let nb = new.as_bytes()[new.len() - 1 - suffix];
        if ob != nb {
            break;
        }
        suffix += 1;
    }
    let mut old_end = old.len() - suffix;
    let mut new_end = new.len() - suffix;
    while !old.is_char_boundary(old_end) || !new.is_char_boundary(new_end) {
        suffix -= 1;
        old_end = old.len() - suffix;
        new_end = new.len() - suffix;
    }
    Some(json!({
        "range": {
            "start": position_at(old, prefix, encoding),
            "end": position_at(old, old_end, encoding),
        },
        "text": &new[prefix..new_end],
    }))
}

/// Per-document mirror of what the server was told.
struct SyncedDoc {
    /// Version the server last saw (didOpen or didChange). Never rewound —
    /// a restart re-opens with the same version, a brand-new open resets.
    version: i64,
    /// Exact text the server holds: diff base for incremental changes and
    /// the payload for re-open after a restart.
    mirror: String,
}

/// The synced-document table for one server session.
#[derive(Default)]
pub struct SyncedDocuments {
    docs: HashMap<String, SyncedDoc>,
}

impl SyncedDocuments {
    /// Build `didOpen` params and record the document at version 1.
    pub fn did_open(&mut self, uri: &str, language_id: &str, text: &str) -> Value {
        self.docs.insert(
            uri.to_string(),
            SyncedDoc {
                version: 1,
                mirror: text.to_string(),
            },
        );
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": 1,
                "text": text,
            }
        })
    }

    /// Re-open params for a server restart: same version, mirrored text —
    /// the new generation's document table is empty, so nothing is
    /// duplicated and no version needs to move.
    pub fn reopen_params(&self, language_id: &str) -> Vec<Value> {
        self.docs
            .iter()
            .map(|(uri, doc)| {
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": language_id,
                        "version": doc.version,
                        "text": doc.mirror,
                    }
                })
            })
            .collect()
    }

    /// Build `didChange` params for `new_text` under the negotiated kind,
    /// bumping the version. Returns `None` when the kind is `None` (the
    /// server takes no text — the mirror still advances so a later
    /// capability change diffs correctly) or the content is unchanged.
    pub fn did_change(
        &mut self,
        uri: &str,
        new_text: &str,
        sync: TextSync,
        encoding: PositionEncoding,
    ) -> Option<Value> {
        let doc = self.docs.get_mut(uri)?;
        let params = match sync.change {
            TextSyncKind::None => None,
            TextSyncKind::Full => Some(json!([{ "text": new_text }])),
            TextSyncKind::Incremental => {
                incremental_change(&doc.mirror, new_text, encoding).map(|c| json!([c]))
            }
        };
        doc.mirror = new_text.to_string();
        let params = params?;
        doc.version += 1;
        Some(json!({
            "textDocument": { "uri": uri, "version": doc.version },
            "contentChanges": params,
        }))
    }

    /// Build `didSave` params (no version in the protocol). `text` rides
    /// along only when the server advertised `includeText`.
    pub fn did_save(&self, uri: &str, include_text: bool) -> Option<Value> {
        let doc = self.docs.get(uri)?;
        let mut text_document = json!({ "uri": uri });
        if include_text {
            text_document
                .as_object_mut()
                .unwrap()
                .insert("text".to_string(), json!(doc.mirror));
        }
        Some(json!({ "textDocument": text_document }))
    }

    /// Build `didClose` params and forget the document.
    pub fn did_close(&mut self, uri: &str) -> Option<Value> {
        self.docs.remove(uri)?;
        Some(json!({ "textDocument": { "uri": uri } }))
    }

    /// Drop a synced document without emitting params — used when the
    /// session is not live enough to hear `didClose` (dead/starting).
    #[allow(dead_code)] // Kept for callers that untrack without a session.
    pub fn forget(&mut self, uri: &str) {
        self.docs.remove(uri);
    }

    pub fn is_open(&self, uri: &str) -> bool {
        self.docs.contains_key(uri)
    }

    /// Version last sent for `uri` — test/support surface.
    #[allow(dead_code)] // Asserted by the transcript tests; UI reads status.
    pub fn version(&self, uri: &str) -> Option<i64> {
        self.docs.get(uri).map(|d| d.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_sync_parses_number_and_object_forms() {
        assert_eq!(
            TextSync::from_capability(Some(&json!(2))),
            TextSync {
                open_close: true,
                change: TextSyncKind::Incremental,
                save: false,
                save_include_text: false,
            }
        );
        assert_eq!(
            TextSync::from_capability(Some(&json!({
                "openClose": true,
                "change": 1,
                "save": {"includeText": true}
            }))),
            TextSync {
                open_close: true,
                change: TextSyncKind::Full,
                save: true,
                save_include_text: true,
            }
        );
        assert_eq!(TextSync::from_capability(None), TextSync::default());
        assert_eq!(
            TextSync::from_capability(Some(&json!(0))),
            TextSync::default()
        );
        // Bare `save: true` advertises didSave without text.
        let s =
            TextSync::from_capability(Some(&json!({"openClose": true, "change": 2, "save": true})));
        assert!(s.save && !s.save_include_text);
    }

    #[test]
    fn uri_for_path_is_stable_and_percent_encodes() {
        let p = Path::new("/tmp/some dir/ünïcode.rs");
        assert_eq!(uri_for_path(p), uri_for_path(p));
        let uri = uri_for_path(p);
        assert!(uri.starts_with("file:///"), "{uri}");
        assert!(uri.contains("some%20dir"), "{uri}");
        assert!(!uri.contains(' '), "{uri}");
        assert!(uri.ends_with(".rs"), "{uri}");
    }

    #[test]
    fn incremental_change_covers_middle_edit() {
        let change = incremental_change("hello world", "hello wide world", PositionEncoding::Utf8)
            .expect("edit must produce a change");
        assert_eq!(change["range"]["start"]["line"], 0);
        // Common prefix is "hello w" (7 units).
        assert_eq!(change["range"]["start"]["character"], 7);
        assert_eq!(change["range"]["end"]["character"], 7);
        // The common suffix is bounded by the prefix ("orld", 4 units) —
        // the inserted span is "ide w".
        assert_eq!(change["text"], "ide w");
    }

    #[test]
    fn incremental_change_respects_utf16_positions_and_boundaries() {
        // Emoji past BMP: surrogate pair is 2 UTF-16 units; an edit after it
        // must place positions in units, never split bytes.
        let old = "a😀b";
        let new = "a😀xb";
        let change = incremental_change(old, new, PositionEncoding::Utf16).unwrap();
        assert_eq!(change["range"]["start"]["character"], 3); // a(1) + 😀(2)
        assert_eq!(change["text"], "x");
        // Identical text sends nothing.
        assert!(incremental_change("same", "same", PositionEncoding::Utf16).is_none());
        // Pure append.
        let append = incremental_change("abc", "abcX", PositionEncoding::Utf32).unwrap();
        assert_eq!(append["range"]["start"]["character"], 3);
        assert_eq!(append["text"], "X");
    }

    #[test]
    fn synced_documents_versions_are_monotonic_per_uri() {
        let sync = TextSync {
            open_close: true,
            change: TextSyncKind::Incremental,
            save: true,
            save_include_text: false,
        };
        let mut docs = SyncedDocuments::default();
        let uri = "file:///tmp/a.rs";
        let mut sent_versions = vec![];
        let open = docs.did_open(uri, "rust", "fn main() {}\n");
        sent_versions.push(open["textDocument"]["version"].as_i64().unwrap());
        let params = docs
            .did_change(uri, "fn main() { 1 }\n", sync, PositionEncoding::Utf16)
            .unwrap();
        sent_versions.push(params["textDocument"]["version"].as_i64().unwrap());
        let params = docs
            .did_change(uri, "fn main() { 12 }\n", sync, PositionEncoding::Utf16)
            .unwrap();
        sent_versions.push(params["textDocument"]["version"].as_i64().unwrap());
        assert!(sent_versions.windows(2).all(|v| v[0] < v[1]));
        assert_eq!(docs.version(uri), Some(3));
        assert!(docs.did_save(uri, false).is_some());
        assert!(docs.did_close(uri).is_some());
        assert!(!docs.is_open(uri));
        // Re-open resets the version: the server forgot the doc at didClose.
        let open = docs.did_open(uri, "rust", "new");
        assert_eq!(open["textDocument"]["version"], 1);
    }

    #[test]
    fn reopen_params_preserve_versions_and_mirror() {
        let mut docs = SyncedDocuments::default();
        docs.did_open("file:///a.rs", "rust", "body");
        docs.did_change(
            "file:///a.rs",
            "body2",
            TextSync {
                open_close: true,
                change: TextSyncKind::Full,
                save: false,
                save_include_text: false,
            },
            PositionEncoding::Utf16,
        );
        let reopened = docs.reopen_params("rust");
        assert_eq!(reopened.len(), 1);
        assert_eq!(reopened[0]["textDocument"]["version"], 2);
        assert_eq!(reopened[0]["textDocument"]["text"], "body2");
    }

    #[test]
    fn did_change_with_sync_none_still_mirrors_but_sends_nothing() {
        let mut docs = SyncedDocuments::default();
        docs.did_open("file:///a", "text", "v1");
        assert!(docs
            .did_change(
                "file:///a",
                "v2",
                TextSync::default(),
                PositionEncoding::Utf8
            )
            .is_none());
        assert_eq!(docs.version("file:///a"), Some(1));
        // Unknown uri produces nothing and cannot panic.
        assert!(docs
            .did_change(
                "file:///gone",
                "x",
                TextSync {
                    open_close: true,
                    change: TextSyncKind::Full,
                    save: false,
                    save_include_text: false,
                },
                PositionEncoding::Utf8,
            )
            .is_none());
    }

    #[test]
    fn incremental_change_backs_off_mid_char_boundaries() {
        // Prefix lands inside é (C3 A9) vs ü (C3 BC): bytes 0-1 match, byte 1
        // is mid-char — the common prefix backs up to the boundary at 1.
        let p = incremental_change("aé", "aü", PositionEncoding::Utf16).unwrap();
        assert_eq!(p["range"]["start"]["character"], 1);
        // "xé" → "wĩ": only the shared 0xA9 tail byte matches, so the range
        // end lands inside é/ĩ — the suffix backs to 0, replacing everything.
        let s = incremental_change("xé", "wĩ", PositionEncoding::Utf16).unwrap();
        assert_eq!(s["range"]["start"]["character"], 0);
        assert_eq!(s["range"]["end"]["character"], 2); // "xé" in UTF-16 units
        assert_eq!(s["text"], "wĩ");
    }

    #[test]
    fn forget_untracks_without_sending() {
        let mut docs = SyncedDocuments::default();
        docs.did_open("file:///x", "rust", "t");
        docs.forget("file:///x");
        assert!(!docs.is_open("file:///x"));
        assert!(docs.did_close("file:///x").is_none());
    }
}
