//! S3 backend using bounded, owned argv-only AWS CLI children.
//!
//! No AWS SDK dependency — all operations are performed by spawning
//! `aws s3api list-objects-v2` / `aws s3 cp` subprocesses.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use super::parser;
use super::types::{S3Config, S3Entry, S3Page, S3Path};

/// The S3 API never returns more than this many keys per request.
const MAX_PAGE_KEYS: usize = 1000;
/// Stdout bound for one page: at most 1000 keys of at most 1 KiB each plus JSON framing.
const PAGE_STDOUT_BYTES: usize = 2 * 1024 * 1024;

/// The S3 backend that manages CLI interactions and caching.
#[derive(Debug, Clone)]
pub struct S3Backend {
    /// Optional AWS profile name.
    profile: Option<String>,
    /// Cache directory for downloaded files.
    #[allow(dead_code)]
    cache_dir: PathBuf,
    /// Map of S3 key → local cache path for already-downloaded files.
    #[allow(dead_code)]
    download_cache: HashMap<String, PathBuf>,
}

impl S3Backend {
    /// Create a new S3Backend from config.
    pub fn new(config: &S3Config) -> Self {
        let pid = std::process::id();
        let cache_dir = PathBuf::from(format!("/tmp/fm-s3-cache-{}", pid));
        Self {
            profile: config.profile.clone(),
            cache_dir,
            download_cache: HashMap::new(),
        }
    }

    /// Check if the `aws` CLI is available on $PATH.
    ///
    /// Returns `Ok(())` if found, `Err` with actionable message if not.
    pub async fn check_cli() -> Result<(), String> {
        if std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|path| {
                std::fs::metadata(path.join("aws")).is_ok_and(|metadata| {
                    if !metadata.is_file() {
                        return false;
                    }
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        metadata.permissions().mode() & 0o111 != 0
                    }
                    #[cfg(not(unix))]
                    {
                        true
                    }
                })
            })
        }) {
            Ok(())
        } else {
            Err("AWS CLI (`aws`) not found on $PATH.\n\
                 Install it: https://docs.aws.amazon.com/cli/latest/userguide/install-cliv2.html\n\
                 Or: pip install awscli"
                .to_string())
        }
    }

    /// List one page of an S3 prefix.
    ///
    /// Spawns a single `aws s3api list-objects-v2 --no-paginate` request for at
    /// most `page_size` keys (clamped to the API maximum) and resumes from
    /// `token` when given. Output, memory and time are bounded; any limit,
    /// cancel or CLI failure is an `Err`, never a partial page.
    pub(crate) fn list_page_bounded(
        &self,
        s3_path: &S3Path,
        token: Option<&str>,
        page_size: usize,
        bytes: usize,
        stopped: impl Fn() -> bool,
    ) -> Result<S3Page, &'static str> {
        let prefix = if s3_path.key.is_empty() || s3_path.key.ends_with('/') {
            s3_path.key.clone()
        } else {
            format!("{}/", s3_path.key)
        };
        let mut cmd = self.base_command();
        // `--opt=value` keeps bucket/prefix/token values from ever parsing as flags.
        cmd.args([
            "s3api",
            "list-objects-v2",
            "--no-paginate",
            "--output",
            "json",
            "--delimiter",
            "/",
        ])
        .arg(format!("--bucket={}", s3_path.bucket))
        .arg(format!("--max-keys={}", page_size.clamp(1, MAX_PAGE_KEYS)));
        if !prefix.is_empty() {
            cmd.arg(format!("--prefix={prefix}"));
        }
        if let Some(token) = token {
            cmd.arg(format!("--continuation-token={token}"));
        }
        let output = capture_owned(cmd, PAGE_STDOUT_BYTES, None, &stopped)?;
        let page = parser::parse_list_objects(&output, &prefix)?;
        let retained = page.entries.iter().fold(
            page.entries.capacity() * std::mem::size_of::<S3Entry>()
                + page.next_token.as_ref().map_or(0, String::capacity),
            |total, entry| total + entry.name.capacity() + entry.modified.capacity(),
        );
        if retained > bytes {
            return Err("S3 listing Incomplete: parser/result budget");
        }
        Ok(page)
    }

    /// Download an S3 object to the local cache directory.
    ///
    /// Returns the local path to the downloaded file.
    /// Skips download if already cached for this session.
    #[allow(dead_code)]
    pub async fn download_to_cache(&mut self, s3_path: &S3Path) -> Result<PathBuf, String> {
        let cache_key = format!("{}/{}", s3_path.bucket, s3_path.key);

        // Check if already cached
        if let Some(cached) = self.download_cache.get(&cache_key) {
            if cached.exists() {
                return Ok(cached.clone());
            }
        }

        // Ensure cache directory exists
        if let Err(e) = std::fs::create_dir_all(&self.cache_dir) {
            return Err(format!("Failed to create cache dir: {}", e));
        }

        // Create local path preserving S3 key structure
        let local_path = self.cache_dir.join(&s3_path.bucket).join(&s3_path.key);
        if let Some(parent) = local_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Err(format!("Failed to create cache subdirectory: {}", e));
            }
        }

        let s3_uri = s3_path.to_uri();
        let mut cmd = Command::new("aws");
        if let Some(ref profile) = self.profile {
            cmd.arg("--profile").arg(profile);
        }
        cmd.arg("s3")
            .arg("cp")
            .arg(&s3_uri)
            .arg(local_path.to_string_lossy().as_ref());

        capture_owned(cmd, 64 * 1024, None, || false).map_err(str::to_owned)?;

        self.download_cache.insert(cache_key, local_path.clone());
        Ok(local_path)
    }

    /// Check if an S3 object is already cached locally.
    #[allow(dead_code)]
    pub fn is_cached(&self, s3_path: &S3Path) -> Option<&PathBuf> {
        let cache_key = format!("{}/{}", s3_path.bucket, s3_path.key);
        self.download_cache.get(&cache_key).filter(|p| p.exists())
    }

    /// Get the cache directory path.
    #[allow(dead_code)]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Stream the first N lines of an S3 object without downloading the full file.
    ///
    /// No shell/pipeline; the owned reader enforces line and byte limits.
    /// Returns `Err` with a user-friendly message on failure.
    #[allow(dead_code)]
    pub async fn stream_head(&self, s3_path: &S3Path, n_lines: usize) -> Result<String, String> {
        self.head_bounded(s3_path, n_lines, || false)
            .map_err(str::to_owned)
    }

    pub(crate) fn head_bounded(
        &self,
        s3_path: &S3Path,
        n_lines: usize,
        stopped: impl Fn() -> bool,
    ) -> Result<String, &'static str> {
        if n_lines == 0 {
            return Ok(String::new());
        }
        let mut cmd = self.base_command();
        cmd.args(["s3", "cp"]).arg(s3_path.to_uri()).arg("-");
        let output = capture_owned(cmd, 16 * 1024, Some(n_lines.min(4096)), stopped)?;
        if output[..output.len().min(8192)].contains(&0) {
            return Err("Binary file — cannot display head preview");
        }
        Ok(String::from_utf8_lossy(&output).into_owned())
    }

    /// Clean up the cache directory.
    pub fn cleanup_cache(&self) {
        let _ = std::fs::remove_dir_all(&self.cache_dir);
    }

    /// Get the configured AWS profile.
    #[allow(dead_code)]
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// Build the base aws command with profile flag if set.
    #[allow(dead_code)]
    fn base_command(&self) -> Command {
        let mut cmd = Command::new("aws");
        if let Some(ref profile) = self.profile {
            cmd.arg("--profile").arg(profile);
        }
        cmd
    }
}

/// Two bounded pipe pumps per running worker, joined before returning. Native
/// child exit/reap (never an aborted async wrapper) precedes worker slot reuse.
fn capture_owned(
    mut command: Command,
    stdout_bytes: usize,
    lines: Option<usize>,
    stopped: impl Fn() -> bool,
) -> Result<Vec<u8>, &'static str> {
    if stopped() {
        return Err("S3 Incomplete: cancelled/deadline before spawn");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "S3 failed: AWS CLI unavailable")?;
    let limit = Arc::new(AtomicBool::new(false));
    let enough = Arc::new(AtomicBool::new(false));
    fn pump(
        mut pipe: impl Read + Send + 'static,
        bytes: usize,
        lines: Option<usize>,
        limit: Arc<AtomicBool>,
        enough: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut output = Vec::with_capacity(bytes.min(64 * 1024));
            let mut buffer = [0u8; 1024];
            let mut count = 0;
            loop {
                let read = match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => {
                        limit.store(true, Ordering::Release);
                        break;
                    }
                };
                for byte in &buffer[..read] {
                    if output.len() == bytes {
                        limit.store(true, Ordering::Release);
                        return output;
                    }
                    output.push(*byte);
                    if *byte == b'\n' {
                        count += 1;
                        if lines.is_some_and(|lines| count == lines) {
                            enough.store(true, Ordering::Release);
                            return output;
                        }
                    }
                }
            }
            output
        })
    }
    let out = pump(
        child.stdout.take().unwrap(),
        stdout_bytes,
        lines,
        limit.clone(),
        enough.clone(),
    );
    let err = pump(
        child.stderr.take().unwrap(),
        8192,
        None,
        limit.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut interrupted = false;
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => {}
            Err(_) => {
                interrupted = true;
                break false;
            }
        }
        if stopped()
            || Instant::now() >= deadline
            || limit.load(Ordering::Acquire)
            || enough.load(Ordering::Acquire)
        {
            interrupted = !enough.load(Ordering::Acquire);
            break false;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    // Kill only this command's owned process group, including descendants that
    // might retain pipe descriptors. Readers cannot outlive worker retirement.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    let output = out
        .join()
        .map_err(|_| "S3 failed: stdout reader panicked")?;
    let stderr = err
        .join()
        .map_err(|_| "S3 failed: stderr reader panicked")?;
    if interrupted || stopped() || limit.load(Ordering::Acquire) {
        return Err("S3 Incomplete: output limit/cancel/deadline");
    }
    if !success && !enough.load(Ordering::Acquire) {
        let diagnostic = parser::parse_error_output(&String::from_utf8_lossy(&stderr));
        return Err(if diagnostic.contains("expired") {
            "AWS credentials have expired. Please refresh your credentials."
        } else if diagnostic.contains("denied") {
            "S3 failed: access denied"
        } else {
            "S3 failed: AWS CLI error"
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn app_jobs_final_s3_cli_probe_rejects_non_executable() {
        if std::env::var_os("FM_S3_PROBE_CHILD").is_none() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let fake = dir.path().join("aws");
            std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o600)).unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "s3::backend::tests::app_jobs_final_s3_cli_probe_rejects_non_executable",
                    "--nocapture",
                ])
                .env("FM_S3_PROBE_CHILD", "1")
                .env("PATH", dir.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        assert!(
            S3Backend::check_cli().await.is_err(),
            "non-executable aws file advertised as installed CLI"
        );
    }

    #[cfg(unix)]
    #[test]
    fn app_jobs_final_s3_fake_cli_matrix_and_actual_pool() {
        if std::env::var_os("FM_S3_MATRIX_CHILD").is_none() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let fake = dir.path().join("aws");
            std::fs::write(
                &fake,
                r#"#!/usr/bin/python3
import os, sys, signal, json
args = sys.argv[1:]
if "--profile" in args:
    assert args[:2] == ["--profile", "literal '; profile"], args
def opt(name):
    for arg in args:
        if arg.startswith("--" + name + "="):
            return arg.split("=", 1)[1]
if "s3api" in args:
    for flag in ("--no-paginate", "--delimiter"):
        assert flag in args, args
    assert opt("max-keys"), args
    prefix = opt("prefix") or ""
    uri = "s3://" + opt("bucket") + "/" + prefix
else:
    uri = args[-2]
if "cancel" in uri or "deadline" in uri:
    marker = os.environ["PEER_PID"]
    with open(marker + ".pending", "w") as output:
        output.write(str(os.getpid()))
    os.rename(marker + ".pending", marker)
    signal.pause()
elif "stderr" in uri:
    sys.stderr.write("z" * 20000)
    sys.exit(1)
elif "expired" in uri:
    sys.stderr.write("ExpiredToken")
    sys.exit(1)
elif "denied" in uri:
    sys.stderr.write("AccessDenied")
    sys.exit(1)
elif "failure" in uri:
    sys.exit(1)
elif "longline" in uri:
    sys.stdout.write("PRE " + "x" * 5000)
elif "giantlist" in uri:
    sys.stdout.write("x" * 3145728)
elif "binary" in uri:
    sys.stdout.buffer.write(b"a\0b")
elif "cp" in args:
    assert uri == "s3://fake-bucket/literal '; object", args
    sys.stdout.write("one\ntwo\nthree\n")
elif "paged" in uri:
    if opt("continuation-token") == "tok2":
        page = {"Contents": [{"Key": prefix + "c.txt", "Size": 3}]}
    else:
        assert opt("continuation-token") is None, args
        page = {
            "CommonPrefixes": [{"Prefix": prefix + "d/"}],
            "Contents": [{"Key": prefix + "a.txt", "Size": 1}],
            "NextContinuationToken": "tok2",
        }
    sys.stdout.write(json.dumps(page))
else:
    page = {
        "Contents": [
            {"Key": prefix, "Size": 0},
            {"Key": prefix + "file.txt", "Size": 7, "LastModified": "2026-03-10T12:34:56+00:00"},
        ],
        "CommonPrefixes": [{"Prefix": prefix + "child/"}]
            + ([{"Prefix": "paged/"}] if prefix == "" else []),
    }
    sys.stdout.write(json.dumps(page))
"#,
            )
            .unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "s3::backend::tests::app_jobs_final_s3_fake_cli_matrix_and_actual_pool",
                    "--nocapture",
                ])
                .env("FM_S3_MATRIX_CHILD", "1")
                .env("PEER_PID", dir.path().join("peer.pid"))
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        dir.path().display(),
                        std::env::var("PATH").unwrap()
                    ),
                )
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let path = S3Path::parse("s3://fake-bucket/").unwrap();
        let backend = S3Backend::new(&S3Config {
            path: path.clone(),
            profile: Some("literal '; profile".into()),
        });
        let page = backend
            .list_page_bounded(&path, None, 1000, 8192, || false)
            .unwrap();
        assert!(page.next_token.is_none());
        let entries = page.entries;
        assert_eq!(entries.len(), 3);
        assert_eq!((&*entries[0].name, entries[0].is_dir), ("child/", true));
        assert_eq!((&*entries[1].name, entries[1].is_dir), ("paged/", true));
        assert_eq!((&*entries[2].name, entries[2].size), ("file.txt", 7));
        assert_eq!(entries[2].modified, "2026-03-10 12:34:56");
        let paged = S3Path::parse("s3://fake-bucket/paged/").unwrap();
        let first = backend
            .list_page_bounded(&paged, None, 2, 8192, || false)
            .unwrap();
        assert_eq!(first.entries.len(), 2);
        assert_eq!(first.next_token.as_deref(), Some("tok2"));
        let second = backend
            .list_page_bounded(&paged, first.next_token.as_deref(), 2, 8192, || false)
            .unwrap();
        assert_eq!(second.entries.len(), 1);
        assert!(second.next_token.is_none());
        let head = S3Path::parse("s3://fake-bucket/literal '; object").unwrap();
        assert_eq!(
            backend.head_bounded(&head, 2, || false).unwrap(),
            "one\ntwo\n"
        );
        assert_eq!(backend.head_bounded(&head, 0, || false).unwrap(), "");
        assert!(backend.head_bounded(&head, 2, || true).is_err());
        for name in [
            "longline",
            "giantlist",
            "stderr",
            "expired",
            "denied",
            "failure",
        ] {
            assert!(
                backend
                    .list_page_bounded(
                        &S3Path::parse(&format!("s3://fake-bucket/{name}")).unwrap(),
                        None,
                        1000,
                        8192,
                        || false
                    )
                    .is_err(),
                "{name}"
            );
        }
        assert!(backend
            .head_bounded(
                &S3Path::parse("s3://fake-bucket/binary").unwrap(),
                2,
                || false
            )
            .is_err());
        assert!(backend
            .list_page_bounded(&path, None, 1000, 1, || false)
            .is_err());
        // Readiness comes from the actual peer's PID publication, not a sleep.
        let pid_file = PathBuf::from(std::env::var_os("PEER_PID").unwrap());
        assert!(backend
            .head_bounded(
                &S3Path::parse("s3://fake-bucket/cancel").unwrap(),
                2,
                || pid_file.exists()
            )
            .is_err());
        let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "cancelled child was not reaped"
        );
        std::fs::remove_file(&pid_file).unwrap();
        assert!(backend
            .head_bounded(
                &S3Path::parse("s3://fake-bucket/deadline").unwrap(),
                2,
                || false
            )
            .is_err());
        let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "deadline child was not reaped"
        );
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            assert!(S3Backend::check_cli().await.is_ok());
            let dir = tempfile::tempdir().unwrap();
            let mut app =
                crate::app::App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
            app.init_s3_mode(S3Config {
                path: path.clone(),
                profile: None,
            });
            let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
                slots: 1,
                ..Default::default()
            });
            tx.send(crate::event::Event::Resize(1, 1)).await.unwrap();
            app.spawn_s3_initial_load(&tx);
            let result = app.next_background().await.unwrap();
            app.apply_background(result);
            assert_eq!(app.tree_state.root.children.as_ref().unwrap().len(), 3);
            assert!(!app.tree_state.root.is_loading);
            app.spawn_s3_expand("s3://fake-bucket/child/".into(), &tx);
            let result = app.next_background().await.unwrap();
            app.apply_background(result);
            let child = crate::fs::tree::TreeState::find_node_mut_pub(
                &mut app.tree_state.root,
                &PathBuf::from("s3://fake-bucket/child/"),
            )
            .unwrap();
            assert_eq!(child.children.as_ref().unwrap().len(), 2);
            assert!(!child.is_loading);
            app.spawn_s3_expand("s3://fake-bucket/paged/".into(), &tx);
            let result = app.next_background().await.unwrap();
            app.apply_background(result);
            let paged_path = PathBuf::from("s3://fake-bucket/paged/");
            let paged = crate::fs::tree::TreeState::find_node_mut_pub(
                &mut app.tree_state.root,
                &paged_path,
            )
            .unwrap();
            assert_eq!(paged.children.as_ref().unwrap().len(), 2);
            assert!(paged.has_more_children);
            assert_eq!(paged.s3_next_token.as_deref(), Some("tok2"));
            assert_eq!(paged.total_child_count, None);
            app.load_more(&paged_path);
            let result = app.next_background().await.unwrap();
            app.apply_background(result);
            let paged = crate::fs::tree::TreeState::find_node_mut_pub(
                &mut app.tree_state.root,
                &paged_path,
            )
            .unwrap();
            let names: Vec<_> = paged
                .children
                .as_ref()
                .unwrap()
                .iter()
                .map(|child| child.name.as_str())
                .collect();
            assert_eq!(names, ["d/", "a.txt", "c.txt"]);
            assert!(!paged.has_more_children && paged.s3_next_token.is_none());
            assert_eq!(paged.total_child_count, Some(3));
            app.handle_s3_listing_complete(
                "s3://fake-bucket/",
                vec![S3Entry {
                    name: "literal '; object".into(),
                    is_dir: false,
                    size: 1,
                    modified: String::new(),
                }]
                .into(),
            );
            app.tree_state.selected_index = 1;
            app.spawn_s3_head(&tx);
            let result = app.next_background().await.unwrap();
            app.apply_background(result);
            assert!(!app.s3_head_loading);
            assert!(app
                .preview_state
                .content_lines
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.content.contains("one")));
            rx.close();
            app.shutdown_background().await;
        });
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn app_jobs_final_s3_giant_no_newline_head_is_bounded() {
        // A subprocess-local PATH avoids races or real AWS in the full suite.
        if std::env::var_os("FM_FAKE_CLI_CHILD").is_none() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let fake = dir.path().join("aws");
            std::fs::write(
                &fake,
                "#!/usr/bin/python3\nimport sys\nsys.stdout.write('x' * 2097152)\n",
            )
            .unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "s3::backend::tests::app_jobs_final_s3_giant_no_newline_head_is_bounded",
                    "--nocapture",
                ])
                .env("FM_FAKE_CLI_CHILD", "1")
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        dir.path().display(),
                        std::env::var("PATH").unwrap()
                    ),
                )
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            return;
        }
        let backend = S3Backend::new(&S3Config {
            path: S3Path::parse("s3://fake-bucket/giant").unwrap(),
            profile: None,
        });
        let output = backend
            .stream_head(&S3Path::parse("s3://fake-bucket/giant").unwrap(), 100)
            .await;
        assert!(
            output.as_ref().map_or(true, |text| text.len() <= 65_536),
            "no-newline head retained more than its byte envelope"
        );
    }

    #[test]
    fn test_backend_creation() {
        let config = S3Config {
            path: S3Path::parse("s3://test-bucket/prefix/").unwrap(),
            profile: Some("mfa".to_string()),
        };
        let backend = S3Backend::new(&config);
        assert_eq!(backend.profile(), Some("mfa"));
        assert!(backend
            .cache_dir()
            .to_string_lossy()
            .contains("fm-s3-cache-"));
    }

    #[test]
    fn test_backend_no_profile() {
        let config = S3Config {
            path: S3Path::parse("s3://bucket").unwrap(),
            profile: None,
        };
        let backend = S3Backend::new(&config);
        assert_eq!(backend.profile(), None);
    }

    #[test]
    fn test_stream_head_binary_detection() {
        // Binary content should be detected by null byte check
        let content = "hello\0world";
        let check_len = content.len().min(8192);
        assert!(content.as_bytes()[..check_len].contains(&0));
    }

    #[test]
    fn test_cache_key_lookup() {
        let config = S3Config {
            path: S3Path::parse("s3://bucket").unwrap(),
            profile: None,
        };
        let backend = S3Backend::new(&config);
        let path = S3Path::parse("s3://bucket/key.txt").unwrap();
        assert!(backend.is_cached(&path).is_none());
    }
}
