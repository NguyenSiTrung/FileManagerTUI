# Plan: Fuzzy Finder + Search

## Phase 1: Search Infrastructure + AppMode Extensions

- [ ] Task 1: Add `AppMode::Search` and `AppMode::Filter` variants to `app.rs`
  - [ ] Add SearchState struct (query string, cursor position, results, selected_index, cached_paths)
  - [ ] Add FilterState to TreeState (filter_query, is_filtering flag)
  - [ ] Add methods: `open_search()`, `close_search()`, `start_filter()`, `clear_filter()`
  - [ ] Tests: mode transitions, state initialization/cleanup

- [ ] Task 2: Build lazy file path indexer
  - [ ] Implement `build_path_index()` — iterative stack-based walk (reuse 10K cap pattern)
  - [ ] Store as `Option<Vec<PathBuf>>` on App (None = not yet built)
  - [ ] Invalidate cache on tree mutations (expand, file ops, reload)
  - [ ] Tests: index building, cache invalidation, cap enforcement

- [ ] Task 3: Integrate `fuzzy-matcher` scoring and ranking
  - [ ] Use `SkimMatcherV2` from `fuzzy-matcher` to score paths against query
  - [ ] Return sorted results with match indices for highlighting
  - [ ] Limit displayed results (e.g., top 50)
  - [ ] Tests: scoring correctness, ranking order, empty query, no matches

## Phase 2: Fuzzy Finder Overlay UI

- [ ] Task 1: Create `components/search.rs` — SearchWidget
  - [ ] Centered modal using `Clear` + `Block` pattern (existing dialog pattern)
  - [ ] Text input at top with cursor
  - [ ] Scrollable results list below with highlighted match characters
  - [ ] Show relative paths from root
  - [ ] Visual selection indicator on current result

- [ ] Task 2: Wire handler dispatch for `AppMode::Search`
  - [ ] `Ctrl+P` in normal mode → `open_search()`
  - [ ] Character input updates query, re-scores results in real-time
  - [ ] `↑`/`↓`/`k`/`j` navigate results list (not the tree)
  - [ ] `Enter` → navigate tree to selected result, close overlay
  - [ ] `Esc` → close overlay, return to normal mode
  - [ ] Tests: key dispatch, query updates, navigation, enter/esc behavior

- [ ] Task 3: Implement tree navigation to search result
  - [ ] `navigate_to_path()` on TreeState: expand all ancestor directories, select target
  - [ ] Re-flatten tree after expansion
  - [ ] Scroll to bring selected item into view
  - [ ] Tests: navigate to nested file, navigate to root-level file

- [ ] Task 4: Register SearchWidget in `ui.rs` render pipeline
  - [ ] Render overlay on top of existing layout when `AppMode::Search`
  - [ ] Update `components/mod.rs` to export search module

## Phase 3: Inline Tree Filter

- [ ] Task 1: Implement filter logic in `TreeState`
  - [ ] `apply_filter(query)` — rebuild `flat_items` showing only matches + ancestor dirs
  - [ ] Case-insensitive substring match on filename
  - [ ] Preserve tree structure (parent dirs of matches remain visible)
  - [ ] `clear_filter()` — restore full tree, preserve selection if possible
  - [ ] Tests: filter matches, parent preservation, clear restore, empty query

- [ ] Task 2: Wire handler dispatch for filter mode
  - [ ] `/` in normal mode → activate filter, show input in status bar
  - [ ] Character input updates filter query, re-filters tree in real-time
  - [ ] `Esc` → clear filter, restore full tree
  - [ ] `Enter` → accept filter, return to normal navigation within filtered view
  - [ ] Backspace → delete last char, update filter
  - [ ] Tests: key dispatch, filter activation, esc/enter behavior

- [ ] Task 3: Update status bar to show filter input
  - [ ] When filtering: display `Filter: <query>_` in status bar area
  - [ ] When not filtering: show normal status bar content
  - [ ] Tests: status bar content in filter vs normal mode

- [ ] Task: Conductor - User Manual Verification 'Inline Tree Filter' (Protocol in workflow.md)

## Phase 4: Integration + Polish

- [ ] Task 1: Integration testing
  - [ ] End-to-end: open search → type query → select result → verify tree selection
  - [ ] End-to-end: activate filter → type → verify tree filtering → clear
  - [ ] Verify no regressions in existing tree navigation, dialogs, clipboard ops
  - [ ] Test in large directory scenarios (many files)

- [ ] Task 2: Edge cases and robustness
  - [ ] Handle empty directories gracefully in search
  - [ ] Handle special characters in filenames
  - [ ] Handle rapid typing (debounce or immediate — verify performance)
  - [ ] Ensure search index invalidation works after file ops (create, rename, delete, paste)

- [ ] Task: Conductor - User Manual Verification 'Integration + Polish' (Protocol in workflow.md)
