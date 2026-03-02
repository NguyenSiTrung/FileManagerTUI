# Spec: Non-Blocking Directory Preview with Shallow Counting

## Overview

When navigating to a parent folder containing a very large number of files/subdirectories,
the TUI blocks because `spawn_async_dir_summary` performs an unbounded recursive directory
walk to count all entries at all depth levels. This track fixes directory preview to use
**shallow (depth-1) counting** by default, with an optional user-triggered deep scan,
and adds timeout + cancellation safety nets to prevent UI blocking.

## Problem

1. **Recursive walk blocks preview**: `spawn_async_dir_summary` walks the entire tree
   recursively with no depth limit. For directories with millions of files across
   subdirectories, this takes minutes and floods the event channel with progress updates.

2. **No cancellation on navigation**: When the user moves to a different tree item,
   the previous directory scan continues running in the background, wasting resources.

3. **Sync fallback also blocks**: `load_directory_summary` (used in tests/no-event-tx
   contexts) has a 10K cap but still performs a blocking recursive walk.

## Functional Requirements

### FR1: Shallow Directory Summary (Depth-1)
- Default directory preview counts only **immediate children** (depth 1).
- Display: directory name, immediate file count, immediate subdirectory count.
- No recursive size computation in the default view.
- Show the first N (e.g., 20) child item names as a quick listing (like `ls` output).
- Indicate that counts are shallow: e.g., "5 files, 3 subdirectories (direct)".

### FR2: User-Triggered Deep Scan
- Display a hint in the preview panel: "[Press D for deep scan]".
- When the user presses `D` (or configured key) while a directory is previewed,
  launch the existing async recursive scan (`spawn_async_dir_summary`).
- Show progressive updates during the deep scan with a scanning indicator.
- Deep scan results replace the shallow preview with full recursive counts + total size.

### FR3: Scan Cancellation on Navigation
- When the user navigates to a different tree item, **immediately cancel** any
  in-flight directory scan (both shallow and deep).
- Use an `Arc<AtomicBool>` cancel token passed to the scanning task.
- The scanning task checks the token periodically and aborts if cancelled.

### FR4: Configurable Preview Timeout
- Add a `preview_timeout_ms` config option (default: 2000ms).
- If a directory scan (shallow or deep) does not complete within the timeout,
  abort the scan and display: "⚠ Scan timed out (directory too large)".
- For deep scans, show partial results collected before timeout.

### FR5: Loading Indicator
- Show a scanning indicator (e.g., "⏳ Scanning...") in the preview panel while
  any directory scan is in progress.
- Replace with final results or timeout message when complete.

## Non-Functional Requirements

- **Zero UI blocking**: All directory scanning must be async. The main event loop
  must never wait for a directory scan.
- **Resource cleanup**: Cancelled scans must release file handles and stop iteration
  promptly (within one iteration cycle).
- **Backward compatibility**: Existing config files without `preview_timeout_ms` use
  the default value seamlessly.

## Acceptance Criteria

1. Navigating to a directory with 100K+ entries shows preview within <100ms (shallow count).
2. Moving away from a directory while scanning cancels the scan within one tick.
3. Deep scan (D key) shows progressive updates and can be cancelled by navigating away.
4. Preview timeout triggers after configured duration, showing partial results.
5. Child item listing shows first 20 immediate children names.
6. All existing tests pass; new tests cover shallow scan, cancellation, and timeout.

## Out of Scope

- Configurable scan depth (fixed at depth 1 for shallow).
- Status bar scan indicator (preview panel handles its own status).
- Changes to tree expansion or pagination logic.
- Recursive size computation in shallow mode.
