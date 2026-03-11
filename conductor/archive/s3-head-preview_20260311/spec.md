# S3 Head Preview

## Overview

Add the ability to preview the first N lines of an S3 file directly in the
preview panel without downloading the entire file. Triggered by pressing "H"
when an S3 file (object) is selected, the feature streams content via
`aws s3 cp <uri> - | head -n <N>` and renders it with syntax highlighting
in the preview panel.

## Functional Requirements

### FR-1: Head Preview Keybinding
- When in S3 mode and an S3 **file** (not prefix/directory) is selected,
  pressing `H` streams the first N lines of the file and displays them
  in the preview panel.
- Pressing `H` again toggles back to the S3 metadata card.
- `H` has no effect when a directory/prefix is selected, or when not in
  S3 mode.

### FR-2: Streaming Backend
- Add `S3Backend::stream_head(s3_path, n_lines, profile)` that runs:
  `aws s3 cp <s3_uri> - | head -n <N>`
- Must respect the configured AWS `--profile` flag.
- Returns the streamed text content as a `String` (or error).
- Operation is **async** — spawned on tokio so it doesn't block the TUI.

### FR-3: Preview Rendering
- Streamed content is syntax-highlighted using `syntect`, with file type
  detected from the S3 key extension (e.g. `.py`, `.rs`, `.json`).
- Falls back to plain text when:
  - Extension is unrecognized by `syntect`
  - Content appears binary (contains null bytes)
- Preview panel header shows: `"☁ S3 Head ({N} lines) — {filename}"`
- Content is scrollable within the preview panel.
- Line numbers are displayed alongside content.

### FR-4: Loading State
- While streaming is in progress, show a loading indicator in the preview
  panel: `"☁ Loading head preview..."` (or similar).
- If the stream fails (e.g. permission denied, network error), display the
  error message in the preview panel and revert to metadata view.

### FR-5: Configuration
- **Config file** (`config.toml`): New field `s3_head_lines` under
  `[preview]` section. Default: `100`.
- **CLI flag**: `--s3-head-lines <N>` overrides the config file value.
- CLI flag takes precedence over config file value.

## Non-Functional Requirements

- Head preview fetch should complete within a few seconds for typical
  text files (the head+pipe approach terminates quickly).
- No temporary files written to disk — content streamed in memory.
- Must handle large files gracefully (only first N lines are fetched,
  regardless of total file size).

## Acceptance Criteria

1. Pressing `H` on an S3 file shows the first 100 (default) lines with
   syntax highlighting in the preview panel.
2. Pressing `H` again toggles back to the S3 metadata card.
3. `H` is ignored on S3 directories and in local mode.
4. A loading indicator appears during the fetch.
5. Errors (permission denied, network failure) show an error message.
6. `s3_head_lines` in `config.toml` changes the default line count.
7. `--s3-head-lines 50` CLI flag overrides the config value.
8. Syntax highlighting works for common extensions (.rs, .py, .json, .yaml, .log, .txt).
9. Binary files fall back to plain text or show "Binary file" message.

## Out of Scope

- Downloading the full S3 file for editing
- Tail preview (showing last N lines)
- Byte-range requests (`--range`)
- S3 write operations
- Caching of head preview content across selections
