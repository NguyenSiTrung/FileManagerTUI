# Track Learnings: preview-hardening_20260304

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- `load_highlighted_content` reads entire file with `fs::read` — no internal size guard (caller gates via `update_preview`)
- Binary detection: check known extensions first (fast), then null-byte scan in 8KB (fallback)
- `highlight_single_line` and `load_highlighted_content` share duplicate span-building logic
- `load_directory_summary` (sync deep scan) lacks `VisitedDirs` symlink-loop protection that `spawn_async_dir_summary` has
- `total_lines` in `load_head_tail_content` returns displayed line count, not actual file line count
- `read_head_lines` uses `map_while(Result::ok)` which silently drops I/O errors
- `line_wrap` field exists on `PreviewState` but is never read by `PreviewWidget::render`
- `Ctrl+W` handler toggles `line_wrap` state but has no visual effect — dead code
- `epoch_days_to_date` casts `u64` to `i64` without bounds checking
- Store SyntaxSet and Theme on App struct (expensive to load, reuse across previews)
- Use `last_previewed_index` to avoid re-loading preview on every render frame

---

<!-- Learnings from implementation will be appended below -->
