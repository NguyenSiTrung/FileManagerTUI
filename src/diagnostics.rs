//! Versioned, per-server diagnostics state for language-server
//! `textDocument/publishDiagnostics` notifications.
//!
//! OWNERSHIP POLICY (documented contract):
//! - Entries are keyed per `(server language, uri)` — one server can never
//!   overwrite another server's results for the same document, and a
//!   publish is never attributed to a document version it did not carry.
//! - A publish carrying an explicit `version` that differs from the
//!   document's tracked LSP version is **stale**: it is ignored and the
//!   live entry is untouched.
//! - A publish with `version: null` (or absent) is **versionless**: LSP
//!   treats it as relevant to whatever the document currently holds, so
//!   it applies unconditionally.
//! - For documents we do not sync (project files the server pushes that
//!   we never opened) there is no tracked version; the publish's own
//!   version becomes the entry's baseline and later versioned publishes
//!   compare against it.
//! - A server restart clears that server's entries — the new generation
//!   owns a fresh diagnostic table (same rule as `SyncedDocuments`).
//! - Closing a document removes every server's entries for its URI.

use serde_json::Value;
use std::collections::BTreeMap;

/// Items kept per (server, document) — a runaway publish is truncated.
pub const MAX_ITEMS_PER_DOCUMENT: usize = 256;
/// Documents tracked per server — bounds the flat panel rows.
pub const MAX_DOCUMENTS_PER_SERVER: usize = 64;
/// Message length cap; sanitized via the shared server-text scrubber.
const MAX_MESSAGE_CHARS: usize = 160;
/// Flat rows the panel may list at once.
pub const MAX_PANEL_ROWS: usize = 512;

/// LSP `DiagnosticSeverity` (1..=4); anything else collapses to Error,
/// matching the published convention that an absent severity means error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    fn from_lsp(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_u64) {
            Some(2) => Self::Warning,
            Some(3) => Self::Information,
            Some(4) => Self::Hint,
            // Missing or unrecognized severities render as errors —
            // never silently weaker than the server intended.
            _ => Self::Error,
        }
    }

    /// Single-cell marker for gutter/panel rows.
    pub fn marker(self) -> &'static str {
        match self {
            Self::Error => "E",
            Self::Warning => "W",
            Self::Information => "I",
            Self::Hint => "H",
        }
    }
}

/// One parsed diagnostic — positions stay in raw server (line, character)
/// units; byte/display conversion happens at navigation/render time with
/// the session's negotiated encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
    pub severity: Severity,
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
}

/// A queued publish: the manager stamps it with language + generation so a
/// dead server's delayed notification is dropped before it lands here.
#[derive(Debug, Clone)]
pub struct Publish {
    pub language: String,
    pub generation: u64,
    pub uri: String,
    pub version: Option<i64>,
    pub diagnostics: Vec<Diagnostic>,
}

/// What the manager hands the app each drain pass.
#[derive(Debug, Clone)]
pub enum DiagnosticEvent {
    Publish(Publish),
    /// The server generation ended (died or restarted) — clear its rows.
    Clear {
        language: String,
    },
}

/// Entry for one (server, document) pair.
#[derive(Debug)]
struct Entry {
    generation: u64,
    /// The version this entry describes — either the publish's own
    /// version or the tracked version it matched at apply time.
    version: Option<i64>,
    items: Vec<Diagnostic>,
}

/// Per-severity counts for a document or the whole workspace.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub errors: usize,
    pub warnings: usize,
    pub informations: usize,
    pub hints: usize,
}

impl Summary {
    pub fn total(self) -> usize {
        self.errors + self.warnings + self.informations + self.hints
    }

    fn add(&mut self, severity: Severity) {
        match severity {
            Severity::Error => self.errors += 1,
            Severity::Warning => self.warnings += 1,
            Severity::Information => self.informations += 1,
            Severity::Hint => self.hints += 1,
        }
    }

    /// Compact status-bar form, e.g. "E:2 W:1" — empty string when clean.
    pub fn label(self) -> String {
        let mut parts = Vec::new();
        if self.errors > 0 {
            parts.push(format!("E:{}", self.errors));
        }
        if self.warnings > 0 {
            parts.push(format!("W:{}", self.warnings));
        }
        if self.informations > 0 {
            parts.push(format!("I:{}", self.informations));
        }
        if self.hints > 0 {
            parts.push(format!("H:{}", self.hints));
        }
        parts.join(" ")
    }
}

/// The store. Iterating `entries` yields `(language, uri)` keys in sorted
/// order so panel rows are deterministic.
#[derive(Debug, Default)]
pub struct Diagnostics {
    entries: BTreeMap<(String, String), Entry>,
    /// Bumped on every mutation — panels rebuild only when it changes.
    revision: u64,
}

/// Why an `apply` did or did not land; tests pin each arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Items replaced (possibly by an empty list — still an update).
    Applied,
    /// Versioned publish disagrees with the tracked/current version.
    StaleVersion,
    /// A publish from an older generation than the stored entry.
    StaleGeneration,
    /// The per-server document bound is full — publish refused.
    Full,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mutation counter for change detection (panel rebuild trigger).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Apply one publish. `tracked_version` is the LSP version the manager
    /// currently holds for the URI (`None` when the document is not one we
    /// synchronize). See the module-level policy comment for the rules.
    pub fn apply(&mut self, publish: Publish, tracked_version: Option<i64>) -> ApplyOutcome {
        let key = (publish.language.clone(), publish.uri.clone());
        let baseline = tracked_version.or_else(|| {
            self.entries
                .get(&key)
                .and_then(|entry| entry.version)
                .or(publish.version)
        });
        if publish.version.is_some() && publish.version != baseline {
            return ApplyOutcome::StaleVersion;
        }
        if let Some(entry) = self.entries.get(&key) {
            if publish.generation < entry.generation {
                return ApplyOutcome::StaleGeneration;
            }
        } else {
            let documents_for_server = self
                .entries
                .keys()
                .filter(|(language, _)| language == &publish.language)
                .count();
            if documents_for_server >= MAX_DOCUMENTS_PER_SERVER {
                return ApplyOutcome::Full;
            }
        }
        let mut items = publish.diagnostics;
        items.truncate(MAX_ITEMS_PER_DOCUMENT);
        items.sort_by(|a, b| {
            (a.start_line, a.start_character, a.severity).cmp(&(
                b.start_line,
                b.start_character,
                b.severity,
            ))
        });
        self.entries.insert(
            key,
            Entry {
                generation: publish.generation,
                version: publish.version.or(baseline),
                items,
            },
        );
        self.revision += 1;
        ApplyOutcome::Applied
    }

    /// Merged, position-sorted diagnostics for one document across all
    /// servers — each server's results stay in its own slot.
    pub fn for_document(&self, uri: &str) -> Vec<&Diagnostic> {
        let mut items: Vec<&Diagnostic> = self
            .entries
            .iter()
            .filter(|((_, u), _)| u == uri)
            .flat_map(|(_, entry)| entry.items.iter())
            .collect();
        items.sort_by(|a, b| {
            (a.start_line, a.start_character, a.severity).cmp(&(
                b.start_line,
                b.start_character,
                b.severity,
            ))
        });
        items
    }

    /// Severity counts for one document across all servers.
    pub fn summary(&self, uri: &str) -> Summary {
        let mut summary = Summary::default();
        for diagnostic in self.for_document(uri) {
            summary.add(diagnostic.severity);
        }
        summary
    }

    /// Severity counts across every tracked document.
    pub fn total_summary(&self) -> Summary {
        let mut summary = Summary::default();
        for entry in self.entries.values() {
            for diagnostic in &entry.items {
                summary.add(diagnostic.severity);
            }
        }
        summary
    }

    /// Worst severity per touched line — editor gutter markers.
    pub fn lines_for(&self, uri: &str) -> BTreeMap<u32, Severity> {
        let mut lines = BTreeMap::new();
        for ((_, u), entry) in &self.entries {
            if u != uri {
                continue;
            }
            for diagnostic in &entry.items {
                lines
                    .entry(diagnostic.start_line)
                    .and_modify(|severity| {
                        if diagnostic.severity < *severity {
                            *severity = diagnostic.severity;
                        }
                    })
                    .or_insert(diagnostic.severity);
            }
        }
        lines
    }

    /// One flat, bounded, sorted row set for the diagnostics panel.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for ((language, uri), entry) in &self.entries {
            for diagnostic in &entry.items {
                rows.push(Row {
                    language: language.clone(),
                    uri: uri.clone(),
                    line: diagnostic.start_line,
                    character: diagnostic.start_character,
                    severity: diagnostic.severity,
                    message: diagnostic.message.clone(),
                    source: diagnostic.source.clone(),
                });
                if rows.len() >= MAX_PANEL_ROWS {
                    return rows;
                }
            }
        }
        rows
    }

    /// Explicit close: drop every server's entries for the URI.
    pub fn remove_document(&mut self, uri: &str) {
        let before = self.entries.len();
        self.entries.retain(|(_, u), _| u != uri);
        if self.entries.len() != before {
            self.revision += 1;
        }
    }

    /// Server restart or death: its whole diagnostic table is gone.
    pub fn clear_language(&mut self, language: &str) {
        let before = self.entries.len();
        self.entries.retain(|(l, _), _| l != language);
        if self.entries.len() != before {
            self.revision += 1;
        }
    }

    /// Whether a URI has any tracked diagnostics.
    pub fn contains(&self, uri: &str) -> bool {
        self.entries.keys().any(|(_, u)| u == uri)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.values().all(|entry| entry.items.is_empty())
    }
}

/// One row of the navigable panel — denormalized for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub language: String,
    pub uri: String,
    /// 0-based server coordinates; rendered +1.
    pub line: u32,
    pub character: u32,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
}

/// Parse `publishDiagnostics` params into a `Publish`. Malformed bodies
/// (missing `uri` or a non-array `diagnostics`) return `None` — the
/// caller drops them without a user-visible error (notifications are
/// advisory). Individual items with missing/malformed ranges are skipped.
pub fn parse_publish(language: &str, generation: u64, params: &str) -> Option<Publish> {
    let params: Value = serde_json::from_str(params).ok()?;
    let uri = params["uri"].as_str()?.to_string();
    let version = params["version"].as_i64();
    let diagnostics = params["diagnostics"].as_array()?;
    Some(Publish {
        language: language.to_string(),
        generation,
        uri,
        version,
        diagnostics: diagnostics
            .iter()
            .filter_map(parse_diagnostic)
            .take(MAX_ITEMS_PER_DOCUMENT)
            .collect(),
    })
}

/// One `Diagnostic` object. `range` is mandatory (LSP); every other field
/// defaults defensively. Messages are sanitized + length-capped here so no
/// raw server text ever reaches the render path.
fn parse_diagnostic(value: &Value) -> Option<Diagnostic> {
    let range = value["range"].as_object()?;
    let start_line = range["start"]["line"].as_u64()?;
    let start_character = range["start"]["character"].as_u64()?;
    let end_line = range["end"]["line"].as_u64().unwrap_or(start_line);
    let end_character = range["end"]["character"]
        .as_u64()
        .unwrap_or(start_character);
    let message = value["message"].as_str().unwrap_or("");
    Some(Diagnostic {
        start_line: u32::try_from(start_line).ok()?,
        start_character: u32::try_from(start_character).ok()?,
        end_line: u32::try_from(end_line).ok()?,
        end_character: u32::try_from(end_character).ok()?,
        severity: Severity::from_lsp(value.get("severity")),
        code: value["code"]
            .as_str()
            .map(str::to_string)
            .or_else(|| value["code"].as_i64().map(|code| code.to_string())),
        source: value["source"]
            .as_str()
            .map(|source| crate::lsp::features::sanitize_server_text(source, 60)),
        message: crate::lsp::features::sanitize_server_text(message, MAX_MESSAGE_CHARS),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publish(uri: &str, version: Option<i64>, diagnostics: Vec<Diagnostic>) -> Publish {
        Publish {
            language: "rust".to_string(),
            generation: 0,
            uri: uri.to_string(),
            version,
            diagnostics,
        }
    }

    fn diagnostic(line: u32, severity: Severity, message: &str) -> Diagnostic {
        Diagnostic {
            start_line: line,
            start_character: 0,
            end_line: line,
            end_character: 1,
            severity,
            code: None,
            source: None,
            message: message.to_string(),
        }
    }

    #[test]
    fn apply_replaces_and_empty_publish_clears() {
        let mut diagnostics = Diagnostics::new();
        let first = publish(
            "file:///a.rs",
            Some(1),
            vec![diagnostic(0, Severity::Error, "boom")],
        );
        assert_eq!(diagnostics.apply(first, Some(1)), ApplyOutcome::Applied);
        assert_eq!(diagnostics.for_document("file:///a.rs").len(), 1);
        // An empty list is a real update: the document is now clean.
        let cleared = publish("file:///a.rs", Some(2), vec![]);
        assert_eq!(diagnostics.apply(cleared, Some(2)), ApplyOutcome::Applied);
        assert_eq!(diagnostics.for_document("file:///a.rs").len(), 0);
        assert!(diagnostics.is_empty());
        assert!(diagnostics.contains("file:///a.rs")); // entry lives, list empty
    }

    #[test]
    fn versioned_stale_publish_never_overwrites() {
        // Pinned plan test: the stale publish must not replace or append.
        let mut diagnostics = Diagnostics::new();
        let first = publish(
            "file:///a.rs",
            Some(5),
            vec![diagnostic(3, Severity::Warning, "w")],
        );
        diagnostics.apply(first, Some(5));
        assert_eq!(diagnostics.for_document("file:///a.rs").len(), 1);
        let stale = publish(
            "file:///a.rs",
            Some(4),
            vec![
                diagnostic(0, Severity::Error, "stale-1"),
                diagnostic(1, Severity::Error, "stale-2"),
            ],
        );
        diagnostics.apply(stale, Some(5));
        assert_eq!(diagnostics.for_document("file:///a.rs").len(), 1);
        assert_eq!(diagnostics.for_document("file:///a.rs")[0].message, "w");
    }

    #[test]
    fn apply_pinned_stale_version_via_pinned_test_shape() {
        // The plan's verbatim shape: apply(stale) leaves len() unchanged.
        let mut diagnostics = Diagnostics::new();
        diagnostics.apply(
            publish(
                "file:///a.rs",
                Some(1),
                vec![diagnostic(0, Severity::Error, "x")],
            ),
            Some(1),
        );
        assert_eq!(diagnostics.for_document("file:///a.rs").len(), 1);
        diagnostics.apply(
            publish(
                "file:///a.rs",
                Some(0),
                vec![diagnostic(9, Severity::Hint, "s")],
            ),
            Some(1),
        );
        assert_eq!(diagnostics.for_document("file:///a.rs").len(), 1);
    }

    #[test]
    fn versionless_publish_always_applies() {
        let mut diagnostics = Diagnostics::new();
        let first = publish(
            "file:///a.rs",
            Some(1),
            vec![diagnostic(0, Severity::Error, "v1")],
        );
        diagnostics.apply(first, Some(1));
        // Versionless: applies against whatever the doc currently holds.
        let versionless = publish(
            "file:///a.rs",
            None,
            vec![diagnostic(9, Severity::Hint, "fresh")],
        );
        assert_eq!(
            diagnostics.apply(versionless, Some(7)),
            ApplyOutcome::Applied
        );
        assert_eq!(diagnostics.for_document("file:///a.rs")[0].message, "fresh");
    }

    #[test]
    fn untracked_document_uses_publish_version_as_baseline() {
        // A server pushing diagnostics for a file we never opened: the
        // publish's own version is the baseline; a MISMATCHED later version
        // is stale relative to that baseline.
        let mut diagnostics = Diagnostics::new();
        assert_eq!(
            diagnostics.apply(
                publish(
                    "file:///dep.rs",
                    Some(3),
                    vec![diagnostic(1, Severity::Error, "e")]
                ),
                None
            ),
            ApplyOutcome::Applied
        );
        assert_eq!(
            diagnostics.apply(publish("file:///dep.rs", Some(2), vec![]), None),
            ApplyOutcome::StaleVersion
        );
        assert_eq!(diagnostics.for_document("file:///dep.rs").len(), 1);
    }

    #[test]
    fn servers_never_attribute_to_each_other() {
        let mut diagnostics = Diagnostics::new();
        diagnostics.apply(
            publish(
                "file:///a.rs",
                Some(1),
                vec![diagnostic(0, Severity::Error, "rust")],
            ),
            Some(1),
        );
        let mut other = publish(
            "file:///a.rs",
            None,
            vec![diagnostic(1, Severity::Warning, "ts")],
        );
        other.language = "typescript".to_string();
        diagnostics.apply(other, Some(1));
        let items = diagnostics.for_document("file:///a.rs");
        assert_eq!(items.len(), 2);
        let summary = diagnostics.summary("file:///a.rs");
        assert_eq!((summary.errors, summary.warnings), (1, 1));
    }

    #[test]
    fn stale_generation_is_ignored() {
        let mut diagnostics = Diagnostics::new();
        diagnostics.apply(
            publish(
                "file:///a.rs",
                Some(1),
                vec![diagnostic(0, Severity::Error, "new")],
            ),
            Some(1),
        );
        let mut old = publish("file:///a.rs", None, vec![]);
        old.generation = 0; // stored generation is also 0 — not stale
        assert_eq!(diagnostics.apply(old, Some(1)), ApplyOutcome::Applied);
        // Re-store at generation 2, then a generation-1 publish is stale.
        let mut newer = publish(
            "file:///a.rs",
            None,
            vec![diagnostic(2, Severity::Error, "g2")],
        );
        newer.generation = 2;
        diagnostics.apply(newer, Some(1));
        let mut stale_gen = publish("file:///a.rs", None, vec![]);
        stale_gen.generation = 1;
        assert_eq!(
            diagnostics.apply(stale_gen, Some(1)),
            ApplyOutcome::StaleGeneration
        );
        assert_eq!(diagnostics.for_document("file:///a.rs").len(), 1);
    }

    #[test]
    fn clear_language_drops_only_that_servers_entries() {
        let mut diagnostics = Diagnostics::new();
        diagnostics.apply(
            publish(
                "file:///a.rs",
                None,
                vec![diagnostic(0, Severity::Error, "r")],
            ),
            None,
        );
        let mut ts = publish(
            "file:///a.rs",
            None,
            vec![diagnostic(0, Severity::Error, "t")],
        );
        ts.language = "typescript".to_string();
        diagnostics.apply(ts, None);
        diagnostics.clear_language("rust");
        assert_eq!(diagnostics.for_document("file:///a.rs").len(), 1);
        assert_eq!(diagnostics.for_document("file:///a.rs")[0].message, "t");
        diagnostics.clear_language("typescript");
        assert!(diagnostics.for_document("file:///a.rs").is_empty());
        assert!(!diagnostics.contains("file:///a.rs"));
    }

    #[test]
    fn remove_document_drops_every_server() {
        let mut diagnostics = Diagnostics::new();
        diagnostics.apply(
            publish(
                "file:///a.rs",
                None,
                vec![diagnostic(0, Severity::Error, "a")],
            ),
            None,
        );
        diagnostics.apply(
            publish(
                "file:///b.rs",
                None,
                vec![diagnostic(0, Severity::Error, "b")],
            ),
            None,
        );
        diagnostics.remove_document("file:///a.rs");
        assert!(diagnostics.for_document("file:///a.rs").is_empty());
        assert_eq!(diagnostics.for_document("file:///b.rs").len(), 1);
    }

    #[test]
    fn parse_publish_covers_all_severities_and_defaults() {
        let params = r#"{
            "uri": "file:///a.rs",
            "version": 4,
            "diagnostics": [
                {"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}},
                 "severity": 1, "code": "E0001", "source": "rustc", "message": "err"},
                {"range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 1}},
                 "severity": 2, "message": "warn"},
                {"range": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 1}},
                 "severity": 3, "message": "info"},
                {"range": {"start": {"line": 4, "character": 0}, "end": {"line": 4, "character": 1}},
                 "severity": 4, "message": "hint"},
                {"range": {"start": {"line": 5, "character": 0}, "end": {"line": 5, "character": 1}},
                 "severity": 99, "message": "unknown→error"},
                {"range": {"start": {"line": 6, "character": 0}, "end": {"line": 6, "character": 1}},
                 "message": "absent→error"}
            ]
        }"#;
        let publish = parse_publish("rust", 0, params).unwrap();
        assert_eq!(publish.version, Some(4));
        assert_eq!(publish.diagnostics.len(), 6);
        assert_eq!(publish.diagnostics[0].severity, Severity::Error);
        assert_eq!(publish.diagnostics[1].severity, Severity::Warning);
        assert_eq!(publish.diagnostics[2].severity, Severity::Information);
        assert_eq!(publish.diagnostics[3].severity, Severity::Hint);
        assert_eq!(publish.diagnostics[4].severity, Severity::Error);
        assert_eq!(publish.diagnostics[5].severity, Severity::Error);
        assert_eq!(publish.diagnostics[0].code.as_deref(), Some("E0001"));
        assert_eq!(publish.diagnostics[0].source.as_deref(), Some("rustc"));
        assert_eq!(publish.diagnostics[0].start_character, 2);
    }

    #[test]
    fn parse_publish_drops_malformed_bodies_and_items() {
        assert!(parse_publish("rust", 0, "not json").is_none());
        assert!(parse_publish("rust", 0, r#"{"uri":"file:///a.rs"}"#).is_none());
        assert!(parse_publish("rust", 0, r#"{"diagnostics":[]}"#).is_none());
        assert!(parse_publish("rust", 0, r#"{"uri":"file:///a.rs","diagnostics":{}}"#).is_none());
        // Items without a usable range are skipped; the rest still land.
        let publish = parse_publish(
            "rust",
            0,
            r#"{"uri":"file:///a.rs","diagnostics":[
                {"message": "no range"},
                {"range": {"start": {"line": "x", "character": 0}}, "message": "bad line"},
                {"range": {"start": {"line": 1, "character": 2}, "end": {}}, "message": "kept"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(publish.diagnostics.len(), 1);
        assert_eq!(publish.diagnostics[0].message, "kept");
        // Missing end fields default to the start position.
        assert_eq!(
            (
                publish.diagnostics[0].end_line,
                publish.diagnostics[0].end_character
            ),
            (1, 2)
        );
    }

    #[test]
    fn parse_publish_sanitizes_and_caps() {
        let long = "x".repeat(MAX_MESSAGE_CHARS + 50);
        // JSON escapes decode to real control bytes before sanitize runs.
        let params = format!(
            r#"{{"uri":"file:///a.rs","diagnostics":[
                {{"range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":1}}}},
                  "message":"{e}]8;;evil{b}payload{e}]8;;{b} tail{e}[31m"}},
                {{"range":{{"start":{{"line":1,"character":0}},"end":{{"line":1,"character":1}}}},
                  "message":"{long}"}},
                {{"range":{{"start":{{"line":2,"character":0}},"end":{{"line":2,"character":1}}}},
                  "code": 42, "source": "s{e}[0mrc", "message":"coded"}}
            ]}}"#,
            e = r"\u001b",
            b = r"\u0007",
            long = long,
        );
        let publish = parse_publish("rust", 0, &params).unwrap();
        assert_eq!(publish.diagnostics[0].message, "payload tail");
        assert_eq!(
            publish.diagnostics[1].message.chars().count(),
            MAX_MESSAGE_CHARS
        );
        assert_eq!(publish.diagnostics[2].code.as_deref(), Some("42"));
        assert_eq!(publish.diagnostics[2].source.as_deref(), Some("src"));
    }

    #[test]
    fn unicode_messages_and_wide_ranges_round_trip() {
        let params = r#"{"uri":"file:///a.rs","diagnostics":[
            {"range":{"start":{"line":10,"character":7},"end":{"line":10,"character":9}},
             "severity":2,"message":"héllo wörld — ","source":"clippy"}
        ]}"#;
        let publish = parse_publish("rust", 0, params).unwrap();
        assert_eq!(publish.diagnostics[0].start_character, 7);
        assert_eq!(publish.diagnostics[0].message, "héllo wörld — ");
        assert_eq!(publish.diagnostics[0].source.as_deref(), Some("clippy"));
    }

    #[test]
    fn multi_file_aggregates_and_lines_for_mark_worst_per_line() {
        let mut diagnostics = Diagnostics::new();
        diagnostics.apply(
            publish(
                "file:///a.rs",
                None,
                vec![
                    diagnostic(0, Severity::Warning, "w"),
                    diagnostic(0, Severity::Error, "e"), // same line, worse
                    diagnostic(5, Severity::Hint, "h"),
                ],
            ),
            None,
        );
        diagnostics.apply(
            publish(
                "file:///b.rs",
                None,
                vec![diagnostic(2, Severity::Information, "i")],
            ),
            None,
        );
        let lines = diagnostics.lines_for("file:///a.rs");
        assert_eq!(lines.get(&0), Some(&Severity::Error));
        assert_eq!(lines.get(&5), Some(&Severity::Hint));
        assert!(diagnostics.lines_for("file:///b.rs").contains_key(&2));
        let total = diagnostics.total_summary();
        assert_eq!(
            (
                total.errors,
                total.warnings,
                total.informations,
                total.hints
            ),
            (1, 1, 1, 1)
        );
        assert_eq!(total.total(), 4);
        assert_eq!(total.label(), "E:1 W:1 I:1 H:1");
    }

    #[test]
    fn per_document_cap_truncates_and_revision_bumps() {
        let mut diagnostics = Diagnostics::new();
        let items: Vec<Diagnostic> = (0..MAX_ITEMS_PER_DOCUMENT + 20)
            .map(|i| diagnostic(i as u32, Severity::Error, "e"))
            .collect();
        let revision = diagnostics.revision();
        diagnostics.apply(publish("file:///a.rs", None, items), None);
        assert_eq!(
            diagnostics.for_document("file:///a.rs").len(),
            MAX_ITEMS_PER_DOCUMENT
        );
        assert!(diagnostics.revision() > revision);
    }

    #[test]
    fn per_server_document_bound_refuses_overflow() {
        let mut diagnostics = Diagnostics::new();
        for i in 0..MAX_DOCUMENTS_PER_SERVER {
            let uri = format!("file:///f{i}.rs");
            diagnostics.apply(
                publish(&uri, None, vec![diagnostic(0, Severity::Error, "e")]),
                None,
            );
        }
        let outcome = diagnostics.apply(
            publish(
                "file:///overflow.rs",
                None,
                vec![diagnostic(0, Severity::Error, "e")],
            ),
            None,
        );
        assert_eq!(outcome, ApplyOutcome::Full);
        assert!(!diagnostics.contains("file:///overflow.rs"));
    }

    #[test]
    fn rows_are_bounded_sorted_and_denormalized() {
        let mut diagnostics = Diagnostics::new();
        diagnostics.apply(
            publish(
                "file:///b.rs",
                None,
                vec![
                    diagnostic(4, Severity::Information, "b-info"),
                    diagnostic(0, Severity::Error, "b-err"),
                ],
            ),
            None,
        );
        diagnostics.apply(
            publish(
                "file:///a.rs",
                None,
                vec![diagnostic(2, Severity::Warning, "a-warn")],
            ),
            None,
        );
        let rows = diagnostics.rows();
        assert_eq!(rows.len(), 3);
        // Sorted by (uri, line, severity): a.rs first, then b.rs:0, b.rs:4.
        assert_eq!(rows[0].uri, "file:///a.rs");
        assert_eq!(rows[1].uri, "file:///b.rs");
        assert_eq!(rows[1].line, 0);
        assert_eq!(rows[1].severity, Severity::Error);
        assert_eq!(rows[2].line, 4);
        assert_eq!(rows[0].language, "rust");
        assert_eq!(rows[0].source, None);
    }

    #[test]
    fn severity_order_is_error_first() {
        assert!(Severity::Error < Severity::Warning);
        assert!(Severity::Warning < Severity::Information);
        assert!(Severity::Information < Severity::Hint);
        assert_eq!(Severity::Error.marker(), "E");
        assert_eq!(Severity::Hint.marker(), "H");
    }
}
