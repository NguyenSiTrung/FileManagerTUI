# Spec: AWS S3 Browse Mode

## Overview

Add a read-only S3 browsing mode to `fm-tui` that lets users navigate and preview
S3 bucket contents directly from the TUI, using the AWS CLI (`aws`) as the backend.
This targets Kubeflow environments where AWS credentials and the CLI are already
configured.

**Activation:** Passing an `s3://` path as the starting argument:
```
fm s3://my-bucket/experiments/ --aws-profile mfa
```

**Backend:** Shell out to `aws s3 ls` / `aws s3 cp` commands — no AWS SDK
dependency; leverages existing CLI authentication (profiles, IRSA, env vars).

## Functional Requirements

### FR-1: CLI Argument Parsing
- Detect when the PATH argument starts with `s3://`
- Accept `--aws-profile <name>` CLI flag (optional; omit for default profile)
- Parse the S3 URI into bucket name and prefix components
- Validate that the `aws` CLI is available on `$PATH`; show clear error if missing

### FR-2: S3 Tree Navigation
- List S3 prefixes ("directories") and objects ("files") by running
  `aws [--profile <p>] s3 ls s3://bucket/prefix/`
- Parse the CLI output to extract:
  - `PRE <name>/` → directory node
  - `<date> <time> <size> <name>` → file node with size and modified date
- Build `TreeNode` entries from parsed output (virtual — no local path)
- Support expand/collapse of S3 prefix directories (lazy-load on expand)
- Support pagination for prefixes with many objects
- Disable write operations in S3 mode: create (`a`/`A`), rename (`r`),
  delete (`d`), cut (`x`), paste (`p`) — show "Not available in S3 mode" message

### FR-3: S3 File Preview (On-Demand)
- When navigating to an S3 object, show metadata preview (no download needed):
  - File size (human-readable)
  - Last modified date/time
  - Full S3 URI (key path)
  - Prompt: "Press [Enter] to download and preview"
- On explicit Enter/`p` keypress:
  - Download to temp cache: `aws [--profile <p>] s3 cp s3://... /tmp/fm-s3-cache/...`
  - Show full syntax-highlighted preview using existing preview pipeline
  - Cache downloaded files for the session (avoid re-download on revisit)
- `y` key copies the full `s3://` URI to clipboard (system or OSC 52 fallback)

### FR-4: S3-Specific UX Indicators
- **Status bar badge:** Show `☁ S3 | s3://bucket` in status bar when in S3 mode
- **Distinct tree colors:** Use a configurable S3-specific color (default: orange/
  amber) for S3 entry names to visually distinguish from local files
- **Loading indicators:** Show `⏳ Loading...` during `aws` CLI calls (listing,
  downloading) since they have noticeable latency (~100-500ms per call)
- **Disabled features:** Grey out inapplicable keybindings in the help overlay
  and status bar hints; show "Not available in S3 mode" toast on attempt

### FR-5: Error Handling
- Handle `aws` CLI not found: show actionable error at startup
- Handle authentication failures: display stderr from `aws` CLI as error dialog
- Handle network/timeout errors: show error message, allow retry
- Handle empty prefixes: show "Empty directory" in preview (same as local)

## Non-Functional Requirements

### NFR-1: No New Binary Dependencies
- Shell out to `aws` CLI — no `aws-sdk-s3` crate
- No increase in binary size beyond the new Rust code itself

### NFR-2: Latency Tolerance
- All `aws` CLI calls must be async (spawned via `tokio::process::Command`)
- UI must remain responsive during S3 operations (loading indicators)
- Tree listing should not block the event loop

### NFR-3: Configuration
- `--aws-profile` CLI flag
- Future: TOML config for default profile, cache directory, timeout settings

### NFR-4: Graceful Degradation
- Features that don't apply in S3 mode (watcher, inline editor, terminal `cd`)
  should be silently disabled — not crash
- Fuzzy search disabled in S3 mode (would require full prefix enumeration)

## Acceptance Criteria

1. `fm s3://bucket/prefix/ --aws-profile mfa` opens the TUI showing S3 contents
2. Expanding an S3 "directory" lists its children via `aws s3 ls`
3. Selecting an S3 file shows metadata (size, date, URI) without downloading
4. Pressing Enter on an S3 file downloads it and shows syntax-highlighted preview
5. `y` on an S3 file copies the `s3://` URI to clipboard
6. Write operations (create, rename, delete, cut, paste) show disabled message
7. Status bar shows `☁ S3` badge; tree entries use distinct color
8. Loading indicators appear during network operations
9. Help overlay reflects S3-mode keybindings (write ops greyed out)
10. Error messages are clear when `aws` CLI is missing or auth fails

## Out of Scope

- Write operations (create, rename, delete, move) on S3
- AWS SDK integration (using CLI shelling only)
- In-app switching between local and S3 modes
- Dual-pane local+S3 view
- S3 event notifications / live watching
- Cross-region or cross-account browsing
- File editing on S3 objects
- Fuzzy search across S3 contents
