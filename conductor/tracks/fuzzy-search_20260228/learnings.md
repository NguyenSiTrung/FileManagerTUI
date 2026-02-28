# Track Learnings: fuzzy-search_20260228

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Handler uses 3-level dispatch: global keys → panel-specific keys → dialog keys
- Use `Clear` widget + centered `Block` for modal overlays in ratatui
- Use iterative stack-based directory walk with entry cap (10K) to prevent hanging on huge trees
- `flatten()` must clear `multi_selected` since flat indices change on re-flatten
- `TreeState::reload_dir()` reloads a specific directory's children and re-flattens after file ops
- Store SyntaxSet and Theme on App struct (expensive to load, reuse across previews)
- crossterm event polling is blocking — must run in spawned tokio task with mpsc channel

---

## [2026-02-28] - Phase 1: Search Infrastructure
- **Implemented:** SearchState, AppMode::Search/Filter, fuzzy-matcher integration, path indexer, filter logic
- **Files changed:** app.rs, handler.rs, fs/tree.rs, Cargo.toml
- **Learnings:**
  - Patterns: SkimMatcherV2 from fuzzy-matcher returns (score, indices) — indices useful for character highlighting
  - Patterns: Iterative stack-based walk reused from preview_content for 10K cap path indexing
  - Gotchas: `flatten_node_filtered` must check children recursively before deciding to include a node — parent inclusion depends on child matches
  - Context: `fuzzy_matcher::FuzzyMatcher` trait must be imported for `fuzzy_indices` method

## [2026-02-28] - Phase 2: Fuzzy Finder Overlay UI
- **Implemented:** SearchWidget component, handler dispatch, UI integration, filter status bar
- **Files changed:** components/search.rs, components/mod.rs, ui.rs, handler.rs
- **Learnings:**
  - Patterns: Search results rendered with per-character styling by grouping consecutive same-style chars into Spans
  - Gotchas: Match guard `Char('j') if CONTROL` in Rust applies to entire OR pattern — plain 'j' correctly falls through to Char(c) arm
  - Gotchas: clippy enforces `.clamp(min, max)` over `.max(min).min(max)` pattern

## [2026-02-28] - Phase 3-4: Inline Filter + Integration
- **Implemented:** Tree filter tests, cache invalidation wiring, integration tests
- **Files changed:** fs/tree.rs, app.rs, handler.rs
- **Learnings:**
  - Patterns: invalidate_search_cache must be called after ALL tree mutations: create, rename, delete, expand, toggle_hidden, paste
  - Gotchas: File::create in tests creates 0-byte files — use fs::write for tests that need content
  - Context: Filter operates on loaded tree structure only — unexpanded directories won't have their children matched
