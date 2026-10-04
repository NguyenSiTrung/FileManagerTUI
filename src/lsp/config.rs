//! Installed-server configuration and execution trust (FR-10, AC-8, AC-11).
//!
//! `[lsp]` config maps languages to argv command lines. Two safety rules are
//! enforced here, before any process exists:
//!
//! 1. **Provenance decides trust.** A server argv from the user's global
//!    config or an explicit CLI grant is trusted configuration. A argv from a
//!    *project-local* `.fm-tui.toml` is untrusted input — the repository could
//!    contain it — and never executes on resolution alone. Trust is bound to
//!    the exact (workspace root, argv) pair, granted in-memory for the
//!    session or explicitly listed in the *global* config; a project file can
//!    never grant itself.
//! 2. **Headless defaults to off.** Without an interactive dialog (or a prior
//!    grant) a project command is refused — startup stays asynchronous and
//!    editing works regardless.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Upper bounds on a configured argv — generous, but finite, so a malformed
/// project file cannot push megabytes into a spawn vector.
pub const MAX_ARGV_LEN: usize = 64;
pub const MAX_ARG_BYTES: usize = 4096;

/// Where a server spec came from. Provenance is the trust input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    /// `~/.config/fm-tui/config.toml` — the user's own trusted config.
    Global,
    /// `.fm-tui.toml` inside the opened project — untrusted input.
    ProjectLocal,
    /// An explicit CLI flag — the user said it this invocation. The CLI
    /// server flag lands with Phase 11's client wiring.
    #[allow(dead_code)]
    Cli,
}

/// One `[lsp.servers.<language>]` table entry.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ServerEntry {
    /// Executable + arguments, exec'd directly (never through a shell).
    pub argv: Vec<String>,
    /// Filenames that mark this language's workspace root (e.g.
    /// `["Cargo.toml"]`); empty falls back to `.git` then the file's parent.
    pub root_markers: Vec<String>,
}

/// An explicit global-config grant: `[[lsp.trust]] root = "…" argv = […]`.
/// Loaded only from global/CLI layers — project-local trust entries are
/// stripped with a warning by `AppConfig::load_checked`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TrustGrantEntry {
    /// Workspace root the grant applies to (`~` expands at load).
    pub root: String,
    /// Exact argv the grant covers — trust binds to the command, not a
    /// language name.
    pub argv: Vec<String>,
}

/// The `[lsp]` TOML section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LspConfig {
    /// Master switch (default true — LSP is optional but available).
    pub enabled: Option<bool>,
    /// Language name → server spec.
    pub servers: BTreeMap<String, ServerEntry>,
    /// Explicit (root, argv) execution grants. Honored only from
    /// global/CLI layers.
    pub trust: Vec<TrustGrantEntry>,
    /// Extension → language overrides (e.g. `templ = "html"`).
    pub languages: BTreeMap<String, String>,
}

impl LspConfig {
    /// Whether the subsystem is enabled (default: true — servers still only
    /// start when configured and trusted).
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    /// Section merge: `other`'s fields win; maps merge per key so a project
    /// file can override one language's argv without wiping the rest.
    pub fn merge(&self, other: &LspConfig) -> LspConfig {
        let mut servers = self.servers.clone();
        for (language, entry) in &other.servers {
            servers.insert(language.clone(), entry.clone());
        }
        let mut languages = self.languages.clone();
        for (ext, language) in &other.languages {
            languages.insert(ext.clone(), language.clone());
        }
        let mut trust = self.trust.clone();
        trust.extend(other.trust.iter().cloned());
        LspConfig {
            enabled: other.enabled.or(self.enabled),
            servers,
            languages,
            trust,
        }
    }
}

/// A configured server after merging the config layers.
#[derive(Debug, Clone)]
pub struct ServerSpec {
    pub language: String,
    pub argv: Vec<String>,
    /// Kept for re-resolution when the workspace root changes (Phase 11).
    #[allow(dead_code)]
    pub root_markers: Vec<String>,
    pub source: ConfigSource,
}

/// A spec resolved against an actual file: workspace root found, executable
/// located (or reported missing).
#[derive(Debug, Clone)]
pub struct ResolvedServer {
    pub spec: ServerSpec,
    /// The workspace root the server runs in — part of the trust identity.
    pub root: PathBuf,
    /// Resolved argv[0], when it exists and is executable.
    pub executable: Option<PathBuf>,
}

/// The spawn gate's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnDecision {
    /// Spawn it.
    Allowed,
    /// Interactive approval required — show the trust dialog.
    NeedsTrust,
    /// argv[0] does not resolve to an executable; LSP stays off for this file.
    MissingExecutable(String),
    /// No dialog possible (headless) — refused, editing unaffected.
    DeniedHeadless,
    /// The user refused this (root, argv) earlier in the session.
    DeniedByUser,
    /// `[lsp] enabled = false` or the argv fails validation.
    Disabled(&'static str),
}

impl ResolvedServer {
    /// The pinned contract: a project-local argv is *never* executable purely
    /// on resolution — only global/CLI provenance runs without a grant.
    /// (Tests pin `assert!(!project_server.allowed_without_explicit_trust())`.)
    pub fn allowed_without_explicit_trust(&self) -> bool {
        !matches!(self.spec.source, ConfigSource::ProjectLocal)
    }

    /// Full decision given the session's trust state.
    pub fn decide(&self, trust: &TrustStore, interactive: bool) -> SpawnDecision {
        if let Err(reason) = validate_argv(&self.spec.argv) {
            return SpawnDecision::Disabled(reason);
        }
        if self.executable.is_none() {
            return SpawnDecision::MissingExecutable(
                self.spec.argv.first().cloned().unwrap_or_default(),
            );
        }
        if self.allowed_without_explicit_trust() {
            return SpawnDecision::Allowed;
        }
        if trust.denied(&self.root, &self.spec.argv) {
            return SpawnDecision::DeniedByUser;
        }
        if trust.allows(&self.root, &self.spec.argv) {
            return SpawnDecision::Allowed;
        }
        if interactive {
            SpawnDecision::NeedsTrust
        } else {
            SpawnDecision::DeniedHeadless
        }
    }
}

/// Session-scoped execution trust: (workspace root, argv) pairs the user has
/// explicitly approved or denied this run.
#[derive(Debug, Default)]
pub struct TrustStore {
    granted: HashSet<(PathBuf, Vec<String>)>,
    denied: HashSet<(PathBuf, Vec<String>)>,
}

fn trust_key(root: &Path, argv: &[String]) -> (PathBuf, Vec<String>) {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    (canonical, argv.to_vec())
}

impl TrustStore {
    /// Approve (root, argv) for this session. Never persisted anywhere —
    /// durable trust must be written to the *global* config by the user.
    pub fn grant(&mut self, root: &Path, argv: &[String]) {
        let key = trust_key(root, argv);
        self.denied.remove(&key);
        self.granted.insert(key);
    }

    /// Record a refusal: we do not re-ask the same (root, argv) this session.
    pub fn deny(&mut self, root: &Path, argv: &[String]) {
        let key = trust_key(root, argv);
        self.granted.remove(&key);
        self.denied.insert(key);
    }

    pub fn allows(&self, root: &Path, argv: &[String]) -> bool {
        self.granted.contains(&trust_key(root, argv))
    }

    pub fn denied(&self, root: &Path, argv: &[String]) -> bool {
        self.denied.contains(&trust_key(root, argv))
    }

    /// Seed session trust from *global/CLI* config grant entries. Entries
    /// that fail validation or whose root does not exist are skipped, not
    /// silently widened.
    pub fn apply_config_grants(&mut self, entries: &[TrustGrantEntry]) {
        for entry in entries {
            if entry.argv.is_empty() || validate_argv(&entry.argv).is_err() {
                continue;
            }
            let root = expand_home(Path::new(&entry.root));
            if !root.is_dir() {
                continue;
            }
            self.grant(&root, &entry.argv);
        }
    }

    /// Pinned contract: session restore never carries execution trust — an
    /// interactive grant dies with the process and must be re-earned.
    /// (Tests pin `assert!(!restored_session_grants_execution)`.)
    #[allow(dead_code)] // Session-restore path lands with session wiring.
    pub fn restored_from_session(_record: &crate::session::SessionRecord) -> Self {
        Self::default()
    }

    /// Explicit predicate for the pinned assertion: a restored store grants
    /// nothing for any (root, argv).
    #[allow(dead_code)] // Pinned contract surface; session wiring consumes it.
    pub fn grants_execution(&self, root: &Path, argv: &[String]) -> bool {
        self.allows(root, argv)
    }
}

fn expand_home(path: &Path) -> PathBuf {
    if let Ok(rest) = path.strip_prefix("~") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    path.to_path_buf()
}

/// argv hygiene: bounded size, no empty program/arguments, no NUL bytes.
/// (No shell is involved, so metacharacters are data, not hazards.)
pub fn validate_argv(argv: &[String]) -> Result<(), &'static str> {
    if argv.is_empty() {
        return Err("empty argv");
    }
    if argv.len() > MAX_ARGV_LEN {
        return Err("argv too long");
    }
    if argv
        .iter()
        .any(|a| a.is_empty() || a.len() > MAX_ARG_BYTES || a.contains('\0'))
    {
        return Err("invalid argv entry");
    }
    Ok(())
}

/// Resolve argv[0] to an executable path without a shell: absolute/relative
/// paths must exist and be executable; bare names search `PATH`.
pub fn resolve_executable(program: &str) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    let candidate = Path::new(program);
    if candidate.components().count() > 1 || program.starts_with('.') || program.starts_with('/') {
        return executable_if(candidate.is_file(), candidate);
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(program);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    // Windows executability is extension-driven; existence suffices here.
    let _ = path;
    true
}

fn executable_if(is_file: bool, path: &Path) -> Option<PathBuf> {
    if is_file && is_executable(path) {
        Some(path.to_path_buf())
    } else {
        None
    }
}

/// Built-in extension → language table. Users override via
/// `[lsp.languages]`; an unmapped extension simply yields no server.
pub fn language_for_path(path: &Path, overrides: &BTreeMap<String, String>) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if let Some(language) = overrides.get(&ext) {
        return Some(language.clone());
    }
    let language = match ext.as_str() {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" | "tsx" => "typescript",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "rb" => "ruby",
        "php" => "php",
        "cs" => "csharp",
        "swift" => "swift",
        "scala" | "sc" => "scala",
        "hs" | "lhs" => "haskell",
        "ex" | "exs" => "elixir",
        "erl" | "hrl" => "erlang",
        "clj" | "cljs" | "cljc" => "clojure",
        "zig" => "zig",
        "nim" => "nim",
        "d" => "d",
        "sh" | "bash" => "shellscript",
        "pl" | "pm" => "perl",
        "r" => "r",
        "jl" => "julia",
        "lua" | "fnl" => "lua",
        "json" | "jsonc" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "xml" => "xml",
        "html" | "htm" => "html",
        "css" | "scss" | "less" => "css",
        "md" | "markdown" => "markdown",
        "tex" | "bib" => "latex",
        "sql" => "sql",
        "dockerfile" => "dockerfile",
        _ => return None,
    };
    Some(language.to_string())
}

/// Walk ancestors of `file` for a directory containing a marker file; the
/// nearest wins. Fallback order: configured markers, `.git`, the file's
/// parent directory — there is always *some* bounded root.
pub fn find_workspace_root(file: &Path, markers: &[String]) -> PathBuf {
    let start = if file.is_dir() {
        file.to_path_buf()
    } else {
        file.parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    };
    let mut dir: Option<&Path> = Some(start.as_path());
    let default_marker = ".git";
    while let Some(current) = dir {
        for marker in markers.iter().map(|s| s.as_str()).chain([default_marker]) {
            if !marker.is_empty() && current.join(marker).exists() {
                return current.to_path_buf();
            }
        }
        dir = current.parent();
    }
    start
}

/// Resolve the effective spec for `language` at `file`: the project-local
/// layer wins per language (it is *configuration*, just untrusted for
/// execution), global supplies the rest.
pub fn resolve_server(
    language: &str,
    file: &Path,
    global: &LspConfig,
    local: &LspConfig,
) -> Option<ResolvedServer> {
    let (entry, source) = local
        .servers
        .get(language)
        .map(|e| (e, ConfigSource::ProjectLocal))
        .or_else(|| {
            global
                .servers
                .get(language)
                .map(|e| (e, ConfigSource::Global))
        })?;
    if entry.argv.is_empty() {
        return None;
    }
    let root = find_workspace_root(file, &entry.root_markers);
    Some(ResolvedServer {
        spec: ServerSpec {
            language: language.to_string(),
            argv: entry.argv.clone(),
            root_markers: entry.root_markers.clone(),
            source,
        },
        executable: resolve_executable(&entry.argv[0]),
        root,
    })
}

/// Compact status line for status bars / the LSP status dialog.
/// `state` is one of the client's lifecycle words ("starting", "ready", ...).
pub fn status_line(resolved: Option<&ResolvedServer>, state: &str, detail: &str) -> String {
    match resolved {
        Some(server) => {
            let program = server.spec.argv.join(" ");
            let suffix = if detail.is_empty() {
                String::new()
            } else {
                format!(" — {detail}")
            };
            format!(
                "LSP {} [{}] {}{}",
                server.spec.language, state, program, suffix
            )
        }
        None => "LSP off".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(source: ConfigSource, argv: &[&str]) -> ServerSpec {
        ServerSpec {
            language: "rust".to_string(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
            root_markers: vec!["Cargo.toml".to_string()],
            source,
        }
    }

    #[test]
    fn argv_validation_bounds_and_rejects_empties() {
        assert!(validate_argv(&[]).is_err());
        assert!(validate_argv(&["".to_string()]).is_err());
        assert!(validate_argv(&["ok".to_string(), "".to_string()]).is_err());
        assert!(validate_argv(&["nul\0byte".to_string()]).is_err());
        assert!(validate_argv(&vec!["x".to_string(); MAX_ARGV_LEN + 1]).is_err());
        assert!(validate_argv(&["rust-analyzer".to_string()]).is_ok());
        // Shell metacharacters are inert data — we exec argv directly.
        assert!(
            validate_argv(&["sh".to_string(), "-c".to_string(), "rm -rf /".to_string()]).is_ok()
        );
    }

    #[test]
    fn resolve_executable_finds_path_entries_and_rejects_missing() {
        // `sh` must exist for this repo's unix test environment.
        #[cfg(unix)]
        assert!(resolve_executable("sh").is_some());
        assert!(resolve_executable("definitely-not-a-real-binary-fm-tui").is_none());
        assert!(resolve_executable("").is_none());
    }

    #[test]
    fn workspace_root_prefers_marker_then_parent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        let nested = root.join("src").join("deep");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        let file = nested.join("main.rs");
        std::fs::write(&file, "").unwrap();

        assert_eq!(
            find_workspace_root(&file, &["Cargo.toml".to_string()]),
            root
        );
        // Unknown markers fall back to the file's directory.
        let plain = tempfile::tempdir().unwrap();
        let loose = plain.path().join("file.rs");
        std::fs::write(&loose, "").unwrap();
        assert_eq!(find_workspace_root(&loose, &[]), *plain.path());
    }

    #[test]
    fn global_local_precedence_is_per_language() {
        let global = LspConfig {
            servers: [
                (
                    "rust".to_string(),
                    ServerEntry {
                        argv: vec!["rust-analyzer".to_string()],
                        root_markers: vec![],
                    },
                ),
                (
                    "python".to_string(),
                    ServerEntry {
                        argv: vec!["pylsp".to_string()],
                        root_markers: vec![],
                    },
                ),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let local = LspConfig {
            servers: [(
                "rust".to_string(),
                ServerEntry {
                    argv: vec!["./vendor/lsp.sh".to_string()],
                    root_markers: vec![],
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let file = Path::new("/tmp/x.rs");
        let rust = resolve_server("rust", file, &global, &local).unwrap();
        assert_eq!(rust.spec.argv, vec!["./vendor/lsp.sh"]);
        assert_eq!(rust.spec.source, ConfigSource::ProjectLocal);
        let python = resolve_server("python", file, &global, &local).unwrap();
        assert_eq!(python.spec.argv, vec!["pylsp"]);
        assert_eq!(python.spec.source, ConfigSource::Global);
        assert!(resolve_server("go", file, &global, &local).is_none());
    }

    #[test]
    fn project_server_never_executes_without_explicit_trust() {
        let project_server = ResolvedServer {
            spec: spec(ConfigSource::ProjectLocal, &["./evil.sh"]),
            root: PathBuf::from("/tmp/proj"),
            executable: Some(PathBuf::from("/bin/sh")),
        };
        // Pinned assertions from the plan.
        assert!(!project_server.allowed_without_explicit_trust());
        let trust = TrustStore::default();
        assert_eq!(
            project_server.decide(&trust, true),
            SpawnDecision::NeedsTrust
        );
        // Headless startup defaults to refusal — never execution.
        assert_eq!(
            project_server.decide(&trust, false),
            SpawnDecision::DeniedHeadless
        );

        let global_server = ResolvedServer {
            spec: spec(ConfigSource::Global, &["rust-analyzer"]),
            root: PathBuf::from("/tmp/proj"),
            executable: Some(PathBuf::from("/bin/sh")),
        };
        assert!(global_server.allowed_without_explicit_trust());
        assert_eq!(global_server.decide(&trust, false), SpawnDecision::Allowed);
    }

    #[test]
    fn trust_binds_to_exact_root_and_argv_and_survives_nothing() {
        let root = tempfile::tempdir().unwrap();
        let argv = vec!["./lsp.sh".to_string()];
        let mut trust = TrustStore::default();
        assert!(!trust.allows(root.path(), &argv));
        trust.grant(root.path(), &argv);
        assert!(trust.allows(root.path(), &argv));
        // Different argv under the same root is NOT the granted command.
        assert!(!trust.allows(root.path(), &["./other.sh".to_string()]));
        // Same argv under a different root is NOT granted either.
        let other_root = tempfile::tempdir().unwrap();
        assert!(!trust.allows(other_root.path(), &argv));

        // A modified project argv falls back to NeedsTrust, not the old grant.
        let modified = ResolvedServer {
            spec: spec(ConfigSource::ProjectLocal, &["./other.sh"]),
            root: root.path().to_path_buf(),
            executable: Some(PathBuf::from("/bin/sh")),
        };
        assert_eq!(modified.decide(&trust, true), SpawnDecision::NeedsTrust);

        // Denial is remembered for the session: no re-asking loop.
        trust.deny(root.path(), &argv);
        assert!(trust.denied(root.path(), &argv));
        let denied = ResolvedServer {
            spec: spec(ConfigSource::ProjectLocal, &["./lsp.sh"]),
            root: root.path().to_path_buf(),
            executable: Some(PathBuf::from("/bin/sh")),
        };
        assert_eq!(denied.decide(&trust, true), SpawnDecision::DeniedByUser);
    }

    #[test]
    fn session_restore_never_grants_execution() {
        use crate::session::SessionRecord;
        let root = tempfile::tempdir().unwrap();
        let argv = vec!["./lsp.sh".to_string()];
        let mut trust = TrustStore::default();
        trust.grant(root.path(), &argv);
        assert!(trust.grants_execution(root.path(), &argv));

        // Even if a session record exists, restored trust is empty.
        let record = SessionRecord {
            schema: "fm-session".to_string(),
            version: 1,
            workspace_root: root.path().to_path_buf(),
            active_document_path: None,
            documents: vec![],
            layout: crate::session::SessionLayout {
                explorer_width: 24,
                terminal_height: 7,
                explorer_visible: true,
                terminal_visible: false,
            },
            recent_files: vec![],
        };
        let restored = TrustStore::restored_from_session(&record);
        assert!(!restored.grants_execution(root.path(), &argv));
        let restored_session_grants_execution = restored.grants_execution(root.path(), &argv);
        assert!(!restored_session_grants_execution);
    }

    #[test]
    fn config_grants_apply_from_global_only() {
        let root = tempfile::tempdir().unwrap();
        let mut trust = TrustStore::default();
        trust.apply_config_grants(&[
            TrustGrantEntry {
                root: root.path().to_string_lossy().into_owned(),
                argv: vec!["./lsp.sh".to_string()],
            },
            // Missing root → skipped, not widened.
            TrustGrantEntry {
                root: "/definitely/missing/root/fm-tui".to_string(),
                argv: vec!["x".to_string()],
            },
            // Invalid argv → skipped.
            TrustGrantEntry {
                root: root.path().to_string_lossy().into_owned(),
                argv: vec![],
            },
        ]);
        assert!(trust.allows(root.path(), &["./lsp.sh".to_string()]));
        assert!(!trust.allows(
            Path::new("/definitely/missing/root/fm-tui"),
            &["x".to_string()]
        ));
    }

    #[test]
    fn missing_executable_and_disabled_report_cleanly() {
        let missing = ResolvedServer {
            spec: spec(ConfigSource::Global, &["no-such-lsp-binary-fm"]),
            root: PathBuf::from("/tmp"),
            executable: None,
        };
        assert_eq!(
            missing.decide(&TrustStore::default(), true),
            SpawnDecision::MissingExecutable("no-such-lsp-binary-fm".to_string())
        );
        let invalid = ResolvedServer {
            spec: spec(ConfigSource::Global, &[]),
            root: PathBuf::from("/tmp"),
            executable: None,
        };
        assert!(matches!(
            invalid.decide(&TrustStore::default(), true),
            SpawnDecision::Disabled(_)
        ));
    }

    #[test]
    fn language_map_and_overrides() {
        assert_eq!(
            language_for_path(Path::new("a.rs"), &BTreeMap::new()),
            Some("rust".to_string())
        );
        assert_eq!(
            language_for_path(Path::new("a.py"), &BTreeMap::new()),
            Some("python".to_string())
        );
        assert!(language_for_path(Path::new("a.xyz"), &BTreeMap::new()).is_none());
        let overrides = [("xyz".to_string(), "custom-lang".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            language_for_path(Path::new("a.xyz"), &overrides),
            Some("custom-lang".to_string())
        );
    }

    #[test]
    fn lsp_config_merges_sections_per_key() {
        let global: LspConfig = toml::from_str(
            r#"
            enabled = true
            [servers.rust]
            argv = ["rust-analyzer"]
            [servers.python]
            argv = ["pylsp"]
            [languages]
            templ = "html"
            "#,
        )
        .unwrap();
        let local: LspConfig = toml::from_str(
            r#"
            [servers.rust]
            argv = ["./vendor/ra"]
            "#,
        )
        .unwrap();
        let merged = global.merge(&local);
        assert!(merged.enabled());
        assert_eq!(merged.servers["rust"].argv, vec!["./vendor/ra"]);
        assert_eq!(merged.servers["python"].argv, vec!["pylsp"]);
        assert_eq!(merged.languages["templ"], "html");
    }

    #[test]
    fn status_line_formats_every_state() {
        let server = ResolvedServer {
            spec: spec(ConfigSource::Global, &["rust-analyzer", "--stdio"]),
            root: PathBuf::from("/tmp"),
            executable: None,
        };
        assert_eq!(status_line(None, "off", ""), "LSP off");
        assert_eq!(
            status_line(Some(&server), "ready", "utf-16"),
            "LSP rust [ready] rust-analyzer --stdio — utf-16"
        );
        assert_eq!(
            status_line(Some(&server), "needs trust", ""),
            "LSP rust [needs trust] rust-analyzer --stdio"
        );
    }

    #[test]
    fn merge_languages_apply_from_the_higher_layer() {
        let global: LspConfig = toml::from_str("[languages]\ntempl = \"html\"\n").unwrap();
        let local: LspConfig =
            toml::from_str("[languages]\nvue = \"html\"\ntempl = \"vue\"\n").unwrap();
        let merged = global.merge(&local);
        assert_eq!(merged.languages["vue"], "html");
        assert_eq!(merged.languages["templ"], "vue");
    }

    #[test]
    fn granted_pair_decides_allowed_and_expand_home_resolves() {
        let root = tempfile::tempdir().unwrap();
        let mut trust = TrustStore::default();
        trust.grant(root.path(), &["ra".to_string()]);
        let granted = ResolvedServer {
            spec: spec(ConfigSource::ProjectLocal, &["ra"]),
            root: root.path().to_path_buf(),
            executable: Some(PathBuf::from("/bin/sh")),
        };
        assert_eq!(granted.decide(&trust, true), SpawnDecision::Allowed);

        // Tilde paths expand under the real home; others pass through.
        let home = expand_home(Path::new("~"));
        assert!(home.is_absolute());
        assert_eq!(expand_home(Path::new("~/x")), home.join("x"));
        assert_eq!(expand_home(Path::new("/abs/x")), PathBuf::from("/abs/x"));
    }

    #[test]
    fn workspace_root_parent_fallback_and_unconfigured_language_resolves_none() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.py");
        std::fs::write(&file, "x = 1\n").unwrap();
        // A dir argument is its own starting point; a file starts at parent.
        assert_eq!(
            find_workspace_root(dir.path(), &["missing-marker-xyz".to_string()]),
            dir.path()
        );
        assert_eq!(
            find_workspace_root(&file, &["missing-marker-xyz".to_string()]),
            dir.path()
        );
        // Unconfigured language resolves to nothing — a silent no-op.
        assert!(resolve_server(
            "brainfuck",
            &file,
            &LspConfig::default(),
            &LspConfig::default()
        )
        .is_none());
        // A configured-but-empty argv resolves to nothing either.
        let local = LspConfig {
            servers: [(
                "python".to_string(),
                ServerEntry {
                    argv: vec![],
                    root_markers: vec![],
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        assert!(resolve_server("python", &file, &LspConfig::default(), &local).is_none());
    }
}
