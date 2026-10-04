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

use crate::lsp::positions::{byte_to_lsp, lsp_to_byte, PositionEncoding};

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

/// Inverse of `uri_for_path`: decode a `file://` URI back to a path.
///
/// Returns `None` for every other scheme — navigation must reject
/// `untitled:`, `vscode-*:` and friends visibly rather than guessing a
/// filesystem meaning.
pub fn path_for_uri(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut bytes = Vec::with_capacity(path.len());
    let raw = path.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            let hi = (raw[i + 1] as char).to_digit(16)?;
            let lo = (raw[i + 2] as char).to_digit(16)?;
            bytes.push(((hi << 4) | lo) as u8);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(bytes).ok()?;
    Some(std::path::PathBuf::from(decoded))
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

    /// Version last sent for `uri` — the staleness baseline for versioned
    /// publishDiagnostics (see `crate::diagnostics`).
    pub fn version(&self, uri: &str) -> Option<i64> {
        self.docs.get(uri).map(|d| d.version)
    }
}

// ═══════════════════ Language-feature requests ═══════════════════

/// Feature capabilities advertised by the server at initialize — requests
/// for an unadvertised feature fail visibly instead of being sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerFeatures {
    pub completion: bool,
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub document_symbol: bool,
}

impl ServerFeatures {
    /// Each provider key accepts `bool` or an options object per the spec.
    pub fn from_capability(caps: &Value) -> Self {
        let on = |key: &str| {
            let v = &caps[key];
            v.is_object() || v.as_bool() == Some(true)
        };
        Self {
            completion: on("completionProvider"),
            hover: on("hoverProvider"),
            definition: on("definitionProvider"),
            references: on("referencesProvider"),
            document_symbol: on("documentSymbolProvider"),
        }
    }
}

/// `TextDocumentPositionParams` for the point features.
pub fn position_params(uri: &str, line: u64, character: u64) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character },
    })
}

/// `ReferenceParams` — declarations included like most clients ask.
pub fn references_params(uri: &str, line: u64, character: u64) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character },
        "context": { "includeDeclaration": true },
    })
}

/// `DocumentSymbolParams`.
pub fn document_symbol_params(uri: &str) -> Value {
    json!({ "textDocument": { "uri": uri } })
}

// ═══════════════════ Sanitization ═══════════════════

/// Make server-supplied display text safe to render: strip ANSI escapes
/// and control characters (newlines/tabs survive), then bound the byte
/// count at a char boundary. Never lets escape sequences reach the TUI.
pub fn sanitize_server_text(raw: &str, max_bytes: usize) -> String {
    let mut out = String::with_capacity(raw.len().min(max_bytes));
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI: ESC [ ... final byte; OSC: ESC ] ... (BEL | ESC \)
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for c2 in chars.by_ref() {
                        if ('@'..='~').contains(&c2) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(c2) = chars.next() {
                        if c2 == '\x07' {
                            break;
                        }
                        if c2 == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        if c == '\n' || c == '\t' {
            out.push(c);
        } else if c.is_control() {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    if out.len() > max_bytes {
        let mut end = max_bytes;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

// ═══════════════════ Completion ═══════════════════

/// Hard cap on completion items the UI will ever carry.
pub const MAX_FEATURE_ITEMS: usize = 200;
/// Byte cap for any single rendered server string.
pub const MAX_FEATURE_TEXT: usize = 4096;

/// One validated replacement in byte coordinates, ready for the editor's
/// batch apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeEdit {
    pub start: crate::text::TextPosition,
    pub end: crate::text::TextPosition,
    pub new_text: String,
}

/// The edit a completion entry applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionEdit {
    /// Literal text inserted at the cursor (insertText, or the label
    /// default when the server supplies neither).
    Insert { text: String },
    /// Replace a resolved byte range — `textEdit.range`, or the `insert`
    /// half of an `InsertReplaceEdit` (non-destructive accept, like other
    /// editors' default).
    Replace {
        start: crate::text::TextPosition,
        end: crate::text::TextPosition,
        new_text: String,
    },
}

/// A completion item decoded for display and application.
#[derive(Debug, Clone)]
pub struct CompletionEntry {
    pub label: String,
    pub detail: Option<String>,
    pub kind: Option<u64>,
    pub documentation: Option<String>,
    /// LSP position fields, retained verbatim for the protocol record.
    pub edit: CompletionEdit,
    /// Same-document `additionalTextEdits`, resolved like the main edit.
    pub additional_edits: Vec<RangeEdit>,
    /// `insertTextFormat == 2`: refused at apply time, never expanded.
    pub snippet: bool,
    /// Server attached a `command`: flagged, NEVER executed.
    pub has_command: bool,
    pub deprecated: bool,
}

fn lsp_pos_to_text(
    lines: &[String],
    pos: &Value,
    encoding: PositionEncoding,
) -> Result<crate::text::TextPosition, String> {
    let line = pos["line"].as_u64().ok_or("missing position line")? as usize;
    let character = pos["character"]
        .as_u64()
        .ok_or("missing position character")? as usize;
    let text = lines
        .get(line)
        .ok_or_else(|| "range line out of bounds".to_string())?;
    let byte = lsp_to_byte(text, character, encoding)
        .ok_or_else(|| "position lands inside a unit sequence".to_string())?;
    Ok(crate::text::TextPosition { line, byte })
}

fn lsp_range_to_edit(
    lines: &[String],
    range: &Value,
    new_text: &str,
    encoding: PositionEncoding,
) -> Result<RangeEdit, String> {
    Ok(RangeEdit {
        start: lsp_pos_to_text(lines, &range["start"], encoding)?,
        end: lsp_pos_to_text(lines, &range["end"], encoding)?,
        new_text: new_text.to_string(),
    })
}

fn sanitize_line(raw: &str, max: usize) -> String {
    sanitize_server_text(raw, max)
        .replace('\n', " ")
        .trim()
        .to_string()
}

/// Parse a `completion` result (`CompletionItem[] | CompletionList | null`)
/// against the document text it was requested on, resolving every position
/// into byte offsets. Any malformed entry or undecodable range rejects the
/// whole response — the user gets a visible error, not silent partial data.
pub fn parse_completion(
    result: &Value,
    lines: &[String],
    encoding: PositionEncoding,
) -> Result<Vec<CompletionEntry>, String> {
    let items = if result.is_null() {
        return Ok(Vec::new());
    } else if let Some(items) = result.as_array() {
        items.clone()
    } else if let Some(items) = result["items"].as_array() {
        items.clone()
    } else {
        return Err("malformed completion response".to_string());
    };
    let mut out = Vec::with_capacity(items.len().min(MAX_FEATURE_ITEMS));
    for item in items.iter().take(MAX_FEATURE_ITEMS) {
        let label_raw = item["label"].as_str().unwrap_or("");
        if label_raw.is_empty() {
            continue;
        }
        let snippet = item["insertTextFormat"].as_u64() == Some(2);
        let deprecated = item["deprecated"].as_bool() == Some(true)
            || item["tags"]
                .as_array()
                .is_some_and(|t| t.iter().any(|v| v.as_u64() == Some(1)));
        let has_command = item.get("command").is_some_and(|c| !c.is_null());
        let edit = if let Some(edit) = item.get("textEdit") {
            let new_text = edit["newText"].as_str().unwrap_or("").to_string();
            if let (Some(insert), Some(replace)) = (edit.get("insert"), edit.get("replace")) {
                // InsertReplaceEdit: accept in insert mode — non-destructive.
                let resolved = lsp_range_to_edit(lines, insert, &new_text, encoding)
                    .or_else(|_| lsp_range_to_edit(lines, replace, &new_text, encoding))?;
                CompletionEdit::Replace {
                    start: resolved.start,
                    end: resolved.end,
                    new_text,
                }
            } else {
                let resolved = lsp_range_to_edit(lines, &edit["range"], &new_text, encoding)?;
                CompletionEdit::Replace {
                    start: resolved.start,
                    end: resolved.end,
                    new_text,
                }
            }
        } else if let Some(text) = item["insertText"].as_str() {
            CompletionEdit::Insert {
                text: text.to_string(),
            }
        } else {
            CompletionEdit::Insert {
                text: label_raw.to_string(),
            }
        };
        let mut additional_edits = Vec::new();
        if let Some(edits) = item["additionalTextEdits"].as_array() {
            for e in edits {
                additional_edits.push(lsp_range_to_edit(
                    lines,
                    &e["range"],
                    e["newText"].as_str().unwrap_or(""),
                    encoding,
                )?);
            }
        }
        out.push(CompletionEntry {
            label: sanitize_line(label_raw, 120),
            detail: item["detail"].as_str().map(|d| sanitize_line(d, 240)),
            kind: item["kind"].as_u64(),
            documentation: item["documentation"]
                .as_str()
                .map(|d| sanitize_server_text(d, MAX_FEATURE_TEXT)),
            edit,
            additional_edits,
            snippet,
            has_command,
            deprecated,
        });
    }
    Ok(out)
}

// ═══════════════════ Hover ═══════════════════

/// Parse `hover.contents` into bounded sanitized plain text.
/// Accepts `MarkupContent`, `MarkedString`, and `MarkedString[]`.
pub fn parse_hover(result: &Value) -> Option<String> {
    let contents = &result["contents"];
    let mut parts: Vec<String> = Vec::new();
    let mut push = |value: &Value| {
        if let Some(s) = value.as_str() {
            parts.push(s.to_string());
        } else if let Some(v) = value["value"].as_str() {
            parts.push(v.to_string());
        } else if let Some(v) = value["contents"].as_str() {
            parts.push(v.to_string());
        }
    };
    if let Some(list) = contents.as_array() {
        for item in list.iter().take(16) {
            push(item);
        }
    } else {
        push(contents);
    }
    let joined = parts.join("\n");
    let cleaned = sanitize_server_text(&joined, MAX_FEATURE_TEXT);
    if cleaned.trim().is_empty() {
        None
    } else {
        Some(cleaned.trim_end_matches('\n').to_string())
    }
}

// ═══════════════════ Locations (definition / references) ═══════════════════

/// A resolved `Location`/`LocationLink` — URI kept verbatim for the
/// navigation scheme check; range in LSP position units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationEntry {
    pub uri: String,
    pub start_line: u64,
    pub start_character: u64,
    pub end_line: u64,
    pub end_character: u64,
}

fn location_from(uri: &str, range: &Value) -> Option<LocationEntry> {
    Some(LocationEntry {
        uri: uri.to_string(),
        start_line: range["start"]["line"].as_u64()?,
        start_character: range["start"]["character"].as_u64()?,
        end_line: range["end"]["line"].as_u64()?,
        end_character: range["end"]["character"].as_u64()?,
    })
}

/// Parse a `Location | Location[] | LocationLink[] | null` result.
pub fn parse_locations(result: &Value) -> Vec<LocationEntry> {
    fn push_one(entry: &Value, out: &mut Vec<LocationEntry>) {
        if let (Some(uri), Some(range)) = (entry["uri"].as_str(), entry.get("range")) {
            if let Some(loc) = location_from(uri, range) {
                out.push(loc);
            }
            return;
        }
        // LocationLink: navigate to the *selection* inside the target.
        if let (Some(uri), Some(range)) = (
            entry["targetUri"].as_str(),
            entry
                .get("targetSelectionRange")
                .or_else(|| entry.get("targetRange")),
        ) {
            if let Some(loc) = location_from(uri, range) {
                out.push(loc);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(list) = result.as_array() {
        for entry in list.iter().take(MAX_FEATURE_ITEMS) {
            push_one(entry, &mut out);
        }
    } else if result.is_object() {
        push_one(result, &mut out);
    }
    out
}

// ═══════════════════ Document symbols ═══════════════════

/// One row of a flattened symbol tree: a targetable (line, character) plus
/// display data. `container` names the parent symbol for hierarchical
/// results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEntry {
    pub name: String,
    pub kind: u64,
    pub container: Option<String>,
    pub line: u64,
    pub character: u64,
}

fn walk_symbol(sym: &Value, container: Option<&str>, out: &mut Vec<SymbolEntry>) {
    if out.len() >= MAX_FEATURE_ITEMS {
        return;
    }
    let raw_name = sym["name"].as_str().unwrap_or("");
    if raw_name.is_empty() {
        return;
    }
    let name = sanitize_line(raw_name, 120);
    // DocumentSymbol: selectionRange is the precise target; fall back to
    // range. SymbolInformation: location.range.
    let range = sym
        .get("selectionRange")
        .or_else(|| sym.get("range"))
        .or_else(|| sym["location"].get("range"));
    let (line, character) = match range {
        Some(r) => (
            r["start"]["line"].as_u64().unwrap_or(0),
            r["start"]["character"].as_u64().unwrap_or(0),
        ),
        None => return,
    };
    let container = container
        .map(str::to_string)
        .or_else(|| sym["containerName"].as_str().map(|s| sanitize_line(s, 120)));
    out.push(SymbolEntry {
        name: name.clone(),
        kind: sym["kind"].as_u64().unwrap_or(0),
        container,
        line,
        character,
    });
    if let Some(children) = sym["children"].as_array() {
        for child in children {
            walk_symbol(child, Some(&name), out);
        }
    }
}

/// Parse `documentSymbol` — `DocumentSymbol[]` (hierarchical, flattened
/// pre-order) or `SymbolInformation[]` (already flat). Bounded like every
/// other server list.
pub fn parse_symbols(result: &Value) -> Vec<SymbolEntry> {
    let mut out = Vec::new();
    if let Some(list) = result.as_array() {
        for sym in list {
            walk_symbol(sym, None, &mut out);
        }
    }
    out
}

// ═══════════════════ Completion application ═══════════════════

/// Apply a completion entry to `document` as ONE undoable transaction.
///
/// Fails *before* touching the buffer when the entry uses an unsupported
/// format (snippet), when any range is invalid or edits overlap — the
/// user sees an error and no partial edit lands. A `command` field on the
/// item is ignored entirely: server-supplied commands are never executed.
pub fn apply_completion(
    document: &mut crate::workspace::documents::Document,
    entry: &CompletionEntry,
) -> Result<(), String> {
    if entry.snippet {
        return Err("completion item uses a snippet — unsupported format".to_string());
    }
    let mut edits = entry.additional_edits.clone();
    match &entry.edit {
        CompletionEdit::Insert { text } => {
            let cursor = document.editor.cursor_position();
            edits.push(RangeEdit {
                start: cursor,
                end: cursor,
                new_text: text.clone(),
            });
        }
        CompletionEdit::Replace {
            start,
            end,
            new_text,
        } => edits.push(RangeEdit {
            start: *start,
            end: *end,
            new_text: new_text.clone(),
        }),
    }
    let primary = edits.len() - 1;
    document
        .editor
        .apply_edit_ranges(&edits, primary)
        .map_err(str::to_string)
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

    // ── Language-feature surface (Phase 11 Task 2) ────────────────────────

    /// Real DocumentStore + one opened document (the tempdir is kept by the
    /// returned TempDir so the file outlives setup).
    fn doc_with(
        text: &str,
    ) -> (
        crate::workspace::documents::DocumentStore,
        crate::workspace::documents::DocumentId,
        tempfile::TempDir,
    ) {
        use crate::workspace::documents::{DocumentStore, OpenDisposition};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        std::fs::write(&path, text).unwrap();
        let mut store = DocumentStore::new();
        let id = store.open(&path, OpenDisposition::Pinned).unwrap();
        (store, id, dir)
    }

    fn caps() -> serde_json::Value {
        serde_json::json!({
            "completionProvider": { "triggerCharacters": ["."] },
            "hoverProvider": true,
            "definitionProvider": false,
            "referencesProvider": true,
            "documentSymbolProvider": {}
        })
    }

    #[test]
    fn server_features_parse_bool_and_object_forms() {
        let f = ServerFeatures::from_capability(&caps());
        assert!(f.completion && f.hover && f.references && f.document_symbol);
        assert!(!f.definition);
        assert_eq!(
            ServerFeatures::from_capability(&serde_json::json!({})),
            ServerFeatures::default()
        );
    }

    #[test]
    fn position_params_and_references_params_shape() {
        let p = position_params("file:///x", 3, 9);
        assert_eq!(p["textDocument"]["uri"], "file:///x");
        assert_eq!(p["position"]["line"], 3);
        assert_eq!(p["position"]["character"], 9);
        let r = references_params("file:///x", 1, 2);
        assert_eq!(r["context"]["includeDeclaration"], true);
        let s = document_symbol_params("file:///x");
        assert_eq!(s["textDocument"]["uri"], "file:///x");
    }

    #[test]
    fn sanitize_strips_escapes_controls_and_bounds() {
        let dirty = "ok\x1b[31mred\x1b[0m\x00\x07 bell \x1b]8;;http://x\x07link\x1b]8;;\x07";
        let clean = sanitize_server_text(dirty, 4096);
        assert!(!clean.contains('\x1b'));
        assert!(!clean
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t'));
        assert!(clean.contains("red") && clean.contains("link"), "{clean}");
        let bounded = sanitize_server_text("abcdefghi", 4);
        assert_eq!(bounded, "abcd");
        // multi-byte: bound never splits a char
        let mb = sanitize_server_text("aébé", 3);
        assert_eq!(mb, "aé");
    }

    #[test]
    fn completion_parses_list_form_and_item_defaults() {
        let lines = vec!["let x = foo".to_string()];
        let result = serde_json::json!([
            {"label": "foobar", "kind": 3, "detail": "fn foobar()"},
            {"label": "foo_baz"},
        ]);
        let items = parse_completion(&result, &lines, PositionEncoding::Utf16).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "foobar");
        assert_eq!(items[0].kind, Some(3));
        // no textEdit / insertText → label is the insert default
        #[rustfmt::skip]
        let CompletionEdit::Insert { text } = &items[1].edit else { unreachable!("expected Insert") };
        assert_eq!(text, "foo_baz");
        // CompletionList form also parses
        let list = serde_json::json!({"isIncomplete": true, "items": [{"label": "x"}]});
        assert_eq!(
            parse_completion(&list, &lines, PositionEncoding::Utf16)
                .unwrap()
                .len(),
            1
        );
        // null → empty
        assert!(
            parse_completion(&serde_json::Value::Null, &lines, PositionEncoding::Utf16)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn completion_decodes_textedit_and_insert_replace_forms() {
        let lines = vec!["foo bar".to_string()];
        // TextEdit: replace {line0, ch0..ch3}
        let result = serde_json::json!([
            {"label": "x", "textEdit": {"range": {"start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 3}}, "newText": "baz"}},
            // InsertReplaceEdit: insert 0..3, replace 0..7 → insert chosen
            {"label": "y", "textEdit": {"insert": {"start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 3}},
                "replace": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 7}},
                "newText": "qux"}},
        ]);
        let items = parse_completion(&result, &lines, PositionEncoding::Utf16).unwrap();
        #[rustfmt::skip]
        let CompletionEdit::Replace { start, end, new_text } = &items[0].edit else { unreachable!("expected Replace") };
        assert_eq!((start.line, start.byte, end.line, end.byte), (0, 0, 0, 3));
        assert_eq!(new_text, "baz");
        #[rustfmt::skip]
        let CompletionEdit::Replace { end, .. } = &items[1].edit else { unreachable!("expected Replace") }; // insert half
        assert_eq!(end.byte, 3);
    }

    #[test]
    fn completion_flags_snippet_command_deprecated_and_additional_edits() {
        let lines = vec!["abc".to_string()];
        let result = serde_json::json!([
            {"label": "s", "insertText": "s(${1:x})", "insertTextFormat": 2},
            {"label": "c", "command": {"command": "x.y", "title": "run"}},
            {"label": "d", "tags": [1]},
            {"label": "e", "deprecated": true},
            {"label": "a", "additionalTextEdits": [
                {"range": {"start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 1}}, "newText": "Z"}]},
        ]);
        let items = parse_completion(&result, &lines, PositionEncoding::Utf16).unwrap();
        assert!(items[0].snippet);
        assert!(items[1].has_command && !items[1].snippet);
        assert!(items[2].deprecated && items[3].deprecated);
        assert_eq!(items[4].additional_edits.len(), 1);
        assert_eq!(items[4].additional_edits[0].new_text, "Z");
    }

    #[test]
    fn completion_rejects_undecodable_range_visibly() {
        let lines = vec!["ab".to_string()];
        // UTF-16 char 1 inside a non-BMP scalar can't decode → Err, not a
        // silently dropped item.
        let result = serde_json::json!([
            {"label": "x", "textEdit": {"range": {"start": {"line": 5, "character": 0},
                "end": {"line": 5, "character": 1}}, "newText": "z"}},
        ]);
        assert!(parse_completion(&result, &lines, PositionEncoding::Utf16).is_err());
    }

    #[test]
    fn apply_completion_round_trips_through_one_undo() {
        let before = "let fo = x";
        let (mut store, id, _dir) = doc_with(before);
        let document = store.get_mut(id).unwrap();
        let before_completion = document.text();
        // replace "fo" (line 0, bytes 4..6) with "foobar"
        let entry = CompletionEntry {
            label: "foobar".into(),
            detail: None,
            kind: None,
            documentation: None,
            edit: CompletionEdit::Replace {
                start: crate::text::TextPosition { line: 0, byte: 4 },
                end: crate::text::TextPosition { line: 0, byte: 6 },
                new_text: "foobar".into(),
            },
            additional_edits: vec![],
            snippet: false,
            has_command: false,
            deprecated: false,
        };
        apply_completion(document, &entry).unwrap();
        assert_eq!(document.text(), "let foobar = x");
        document.editor.undo();
        assert_eq!(document.text(), before_completion);
    }

    #[test]
    fn apply_completion_inserts_at_cursor_and_combines_additional_edits() {
        let (mut store, id, _dir) = doc_with("hello");
        let document = store.get_mut(id).unwrap();
        document.editor.set_cursor_position(0, 5);
        let entry = CompletionEntry {
            label: "world".into(),
            detail: None,
            kind: None,
            documentation: None,
            edit: CompletionEdit::Insert {
                text: " world".into(),
            },
            additional_edits: vec![RangeEdit {
                start: crate::text::TextPosition { line: 0, byte: 0 },
                end: crate::text::TextPosition { line: 0, byte: 1 },
                new_text: "H".into(),
            }],
            snippet: false,
            has_command: false,
            deprecated: false,
        };
        apply_completion(document, &entry).unwrap();
        assert_eq!(document.text(), "Hello world");
        document.editor.undo();
        assert_eq!(document.text(), "hello");
    }

    #[test]
    fn apply_completion_refuses_snippets_and_invalid_edits_without_touching() {
        let (mut store, id, _dir) = doc_with("data");
        let document = store.get_mut(id).unwrap();
        let snippet = CompletionEntry {
            label: "s".into(),
            detail: None,
            kind: None,
            documentation: None,
            edit: CompletionEdit::Insert {
                text: "x(${1})".into(),
            },
            additional_edits: vec![],
            snippet: true,
            has_command: false,
            deprecated: false,
        };
        assert!(apply_completion(document, &snippet).is_err());
        assert_eq!(document.text(), "data");

        // overlapping edits → reject entirely
        let overlap = CompletionEntry {
            label: "o".into(),
            detail: None,
            kind: None,
            documentation: None,
            edit: CompletionEdit::Replace {
                start: crate::text::TextPosition { line: 0, byte: 1 },
                end: crate::text::TextPosition { line: 0, byte: 3 },
                new_text: "X".into(),
            },
            additional_edits: vec![RangeEdit {
                start: crate::text::TextPosition { line: 0, byte: 0 },
                end: crate::text::TextPosition { line: 0, byte: 2 },
                new_text: "Y".into(),
            }],
            snippet: false,
            has_command: false,
            deprecated: false,
        };
        assert!(apply_completion(document, &overlap).is_err());
        assert_eq!(document.text(), "data");
        // invalid range (out of bounds byte) → reject entirely
        let bad = CompletionEntry {
            label: "b".into(),
            detail: None,
            kind: None,
            documentation: None,
            edit: CompletionEdit::Replace {
                start: crate::text::TextPosition { line: 0, byte: 0 },
                end: crate::text::TextPosition { line: 0, byte: 99 },
                new_text: "Z".into(),
            },
            additional_edits: vec![],
            snippet: false,
            has_command: false,
            deprecated: false,
        };
        assert!(apply_completion(document, &bad).is_err());
        assert_eq!(document.text(), "data");
    }

    #[test]
    fn hover_parses_marked_strings_markup_and_sanitizes() {
        // MarkupContent
        let r = serde_json::json!({"contents": {"kind": "markdown", "value": "**bold** text"}});
        assert_eq!(parse_hover(&r).unwrap(), "**bold** text");
        // MarkedString[]
        let r = serde_json::json!({"contents": [
            {"language": "rust", "value": "fn f()"},
            "plain note",
        ]});
        let h = parse_hover(&r).unwrap();
        assert!(h.contains("fn f()") && h.contains("plain note"));
        // escapes stripped
        let r = serde_json::json!({"contents": "\x1b[35mPINK\x1b[0m \x01"});
        let h = parse_hover(&r).unwrap();
        assert!(!h.contains('\x1b') && !h.contains('\x01'), "{h}");
        // empty/null → None
        assert!(parse_hover(&serde_json::json!({"contents": ""})).is_none());
        assert!(parse_hover(&serde_json::json!({"contents": null})).is_none());
    }

    #[test]
    fn locations_parse_all_forms_and_keep_other_file_uris() {
        let one = serde_json::json!({"uri": "file:///other.rs",
            "range": {"start": {"line": 9, "character": 2}, "end": {"line": 9, "character": 8}}});
        let locs = parse_locations(&one);
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].uri, "file:///other.rs");
        assert_eq!((locs[0].start_line, locs[0].start_character), (9, 2));
        // LocationLink → targetSelectionRange
        let link = serde_json::json!([{"targetUri": "file:///t.rs",
            "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 4, "character": 0}},
            "targetSelectionRange": {"start": {"line": 2, "character": 5},
                "end": {"line": 2, "character": 9}}}]);
        let locs = parse_locations(&link);
        assert_eq!(locs[0].uri, "file:///t.rs");
        assert_eq!(locs[0].start_line, 2);
        assert_eq!(locs[0].start_character, 5);
        // null/array
        assert!(parse_locations(&serde_json::Value::Null).is_empty());
        let many = serde_json::json!([
            {"uri": "file:///a", "range": {"start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 1}}},
            {"uri": "untitled:u1", "range": {"start": {"line": 1, "character": 0},
                "end": {"line": 1, "character": 1}}},
        ]);
        let locs = parse_locations(&many);
        assert_eq!(locs.len(), 2);
        assert_eq!(locs[1].uri, "untitled:u1");
    }

    #[test]
    fn path_for_uri_decodes_file_and_rejects_other_schemes() {
        assert_eq!(
            path_for_uri("file:///a%20b/x.rs").unwrap(),
            std::path::PathBuf::from("/a b/x.rs")
        );
        assert!(path_for_uri("untitled:u1").is_none());
        assert!(path_for_uri("vscode-remote://x").is_none());
        // bad % escape → None
        assert!(path_for_uri("file:///a%zz").is_none());
    }

    #[test]
    fn sanitize_handles_osc_st_lone_esc_and_midchar_bounds() {
        // OSC terminated by ST (ESC \\) — the second escape form.
        let osc_st = "a\x1b]0;title\x1b\\b";
        assert_eq!(sanitize_server_text(osc_st, 100), "ab");
        // Lone trailing ESC / ESC + unknown introducer falls through.
        assert_eq!(sanitize_server_text("x\x1b", 10), "x");
        assert_eq!(sanitize_server_text("x\x1by", 10), "xy");
        // Bound landing mid-scalar backs to the boundary.
        assert_eq!(sanitize_server_text("aéz", 2), "a");
        // Tab and newline survive; other controls become spaces.
        assert_eq!(sanitize_server_text("a\tb\nc\x02d", 50), "a\tb\nc d");
    }

    #[test]
    fn completion_rejects_malformed_and_skips_unnamed_items() {
        let lines = vec!["ab".to_string()];
        // An object that is neither an array nor {"items": [...]} is malformed.
        assert!(parse_completion(
            &serde_json::json!({"notItems": 1}),
            &lines,
            PositionEncoding::Utf16
        )
        .is_err());
        // A bad range inside additionalTextEdits propagates the same Err —
        // never a partially-parsed item.
        assert!(parse_completion(
            &serde_json::json!([{"label": "x", "additionalTextEdits": [{
                "range": {"start": {"line": 9, "character": 0},
                    "end": {"line": 9, "character": 1}}, "newText": "y"}]}]),
            &lines,
            PositionEncoding::Utf16
        )
        .is_err());
        // Empty labels are skipped; insertText (PlainText) inserts literally.
        let result = serde_json::json!([
            {"label": ""},
            {"label": "ins", "insertText": "inserted()"},
        ]);
        let items = parse_completion(&result, &lines, PositionEncoding::Utf16).unwrap();
        assert_eq!(items.len(), 1);
        #[rustfmt::skip]
        let CompletionEdit::Insert { text } = &items[0].edit else { unreachable!("expected Insert") };
        assert_eq!(text, "inserted()");
    }

    #[test]
    fn hover_reads_nested_contents_variant() {
        // A MarkedString-shaped {"contents": ...} member decodes too.
        let r = serde_json::json!({"contents": [{"contents": "nested value"}]});
        assert_eq!(parse_hover(&r).unwrap(), "nested value");
        // Shapes carrying no text at all → None.
        assert!(parse_hover(&serde_json::json!({"contents": [{"x": 1}]})).is_none());
    }

    #[test]
    fn locations_skip_unreadable_entries() {
        let r = serde_json::json!([
            {"uri": "file:///ok", "range": {"start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 1}}},
            {"nothing": "usable"},
            {"uri": "file:///norange"},
        ]);
        let locs = parse_locations(&r);
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].uri, "file:///ok");
    }

    #[test]
    fn symbols_skip_nameless_and_rangefree_and_cap() {
        let mut many: Vec<serde_json::Value> = (0..220)
            .map(|i| {
                serde_json::json!({"name": format!("s{i}"), "kind": 5,
                 "selectionRange": {"start": {"line": i, "character": 0},
                     "end": {"line": i, "character": 1}}})
            })
            .collect();
        many.insert(
            0,
            serde_json::json!({"kind": 5, "selectionRange":
            {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}}),
        ); // no name
        many.insert(1, serde_json::json!({"name": "norange", "kind": 5})); // no range
        let syms = parse_symbols(&serde_json::Value::Array(many));
        assert_eq!(syms.len(), MAX_FEATURE_ITEMS);
        assert!(!syms.iter().any(|s| s.name == "norange"));
        // children of a capped parent still bounded
        let tree = serde_json::json!([{"name": "p", "kind": 5,
            "selectionRange": {"start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 1}},
            "children": (0..300).map(|i| serde_json::json!({"name": format!("c{i}"),
                "kind": 6, "selectionRange": {"start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 1}}})).collect::<Vec<_>>()}]);
        let syms = parse_symbols(&tree);
        assert_eq!(syms.len(), MAX_FEATURE_ITEMS);
        assert!(syms[1].name.starts_with('c'));
        // Non-array results yield nothing rather than error.
        assert!(parse_symbols(&serde_json::Value::Null).is_empty());
        assert!(parse_symbols(&serde_json::json!({"x": 1})).is_empty());
    }

    #[test]
    fn symbols_flatten_document_symbols_and_symbol_information() {
        let doc = serde_json::json!([
            {"name": "outer", "kind": 5,
             "range": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}},
             "selectionRange": {"start": {"line": 0, "character": 4},
                 "end": {"line": 0, "character": 9}},
             "children": [
                {"name": "inner", "kind": 6,
                 "range": {"start": {"line": 2, "character": 4},
                     "end": {"line": 2, "character": 20}},
                 "selectionRange": {"start": {"line": 2, "character": 4},
                     "end": {"line": 2, "character": 9}}}]},
            {"name": "sibling", "kind": 12,
             "selectionRange": {"start": {"line": 10, "character": 0},
                 "end": {"line": 10, "character": 7}}},
        ]);
        let syms = parse_symbols(&doc);
        assert_eq!(syms.len(), 3);
        assert_eq!(syms[0].name, "outer");
        assert_eq!((syms[0].line, syms[0].character), (0, 4)); // selectionRange
        assert_eq!(syms[1].name, "inner");
        assert_eq!(syms[1].container.as_deref(), Some("outer"));
        assert_eq!(syms[2].name, "sibling");

        // SymbolInformation form (location.uri + containerName)
        let info = serde_json::json!([
            {"name": "fn1", "kind": 12, "containerName": "mod m",
             "location": {"uri": "file:///x.rs",
                "range": {"start": {"line": 7, "character": 3},
                    "end": {"line": 7, "character": 10}}}}]);
        let syms = parse_symbols(&info);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].container.as_deref(), Some("mod m"));
        assert_eq!((syms[0].line, syms[0].character), (7, 3));
    }
}
