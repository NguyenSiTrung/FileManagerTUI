# Spec: Large File & Directory Scalability

## Overview
Harden the application against UI blocking when navigating folders with 1M+ files per subfolder, previewing files >10MB, or attempting to edit files with 100K+ lines. Every user-facing I/O operation must remain non-blocking.

## Functional Requirements

### FR1: Streaming Head+Tail Preview
- `load_head_tail_content()` must NOT read the entire file into memory
- Read only the first N lines (head) via sequential read, then `seek()` to end of file and scan backward in chunks for last M lines (tail)
- Memory usage bounded to `head_lines + tail_lines` regardless of file size
- `fast_line_count()` remains as-is (64KB chunked byte scan is already efficient)

### FR2: Streaming Full Preview for <1MB Files
- `load_highlighted_content()` currently reads entire file — acceptable for <1MB
- No change needed (1MB cap already enforced by `DEFAULT_MAX_FULL_PREVIEW_BYTES`)

### FR3: Hard Block Editor for Large Files
- Refuse to open files exceeding a size threshold (default: 10MB) or line count threshold (default: 100K lines)
- Show clear status message: "File too large for editing (X MB / Y lines). Use an external editor."
- Add configurable constants: `MAX_EDITOR_FILE_BYTES`, `MAX_EDITOR_LINE_COUNT`

### FR4: Threshold-Based Async Directory Expansion
- Directories with entries ≤ page_size: expand synchronously (instant, current behavior)
- Directories with entries > page_size: use `spawn_blocking` + event channel
- Show a "Loading..." virtual node in tree while async scan runs
- On completion, replace "Loading..." with actual children + "Load more..." pagination node
- Use existing `spawn_async_dir_scan` / `DirScanComplete` event pattern

### FR5: Async Directory Summary in Preview
- `update_preview()` for directories currently calls `load_directory_summary()` synchronously (walks up to 10K entries)
- Switch to `spawn_async_dir_summary()` which already exists with progress events
- Show incremental "Scanning... (X files, Y dirs)" while running

### FR6: Sync `load_directory_summary` Removal
- After FR5, the sync `load_directory_summary` in preview path is dead code — remove or gate behind non-preview callers only

### FR7: `load_head_tail_content` ViewMode::HeadOnly / TailOnly
- HeadOnly: read only first N lines (no seek needed)
- TailOnly: seek to end, read last M lines
- Both must use streaming, not full-file load

### FR8: Preview Loading Timeout / Cancel
- If user navigates away while preview is loading (changes selected file), cancel/discard the stale result
- Use `last_previewed_index` comparison on event arrival

## Non-Functional Requirements
- No UI frame drops during any file/directory operation
- Memory usage for preview bounded to O(head_lines + tail_lines), not O(file_size)
- Directory expansion latency for <1000 entries: <50ms (sync path)
- All existing tests must pass; new tests for streaming + async paths

## Acceptance Criteria
1. Navigate a folder tree where a subfolder contains 1M+ files — UI never freezes
2. Select a 100MB text file — preview shows head+tail instantly without loading full file
3. Press `e` on a 50MB file — editor refuses with clear message
4. Expand a dir with 500K files — "Loading..." appears, then paginated results
5. Select a directory with 1M files — preview shows incremental scan progress

## Out of Scope
- Async syntax highlighting (syntect runs fast enough for head+tail line counts)
- Virtual/infinite scrolling in editor
- Memory-mapped file I/O
- External editor integration (just block with message)
