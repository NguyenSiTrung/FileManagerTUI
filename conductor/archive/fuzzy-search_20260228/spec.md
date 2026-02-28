# Spec: Fuzzy Finder + Search

## Overview

Add two complementary search capabilities to FileManagerTUI:
1. **Fuzzy Finder Overlay** (`Ctrl+P`) — A centered modal popup for quickly locating files by partial name matching across the entire tree.
2. **Inline Tree Filter** (`/`) — Filter-as-you-type that hides non-matching items from the tree view, preserving parent directories for context.

## Functional Requirements

### Fuzzy Finder Overlay (`Ctrl+P`)
- **FR-1**: `Ctrl+P` opens a centered modal overlay (consistent with existing dialog pattern using `Clear` + centered `Block`).
- **FR-2**: Modal contains a text input at top and a scrollable results list below.
- **FR-3**: File path index is built lazily on first `Ctrl+P` press, then cached. Cache invalidates on tree changes (expand, file ops, etc.).
- **FR-4**: Index walks all expanded and unexpanded directories recursively (up to 10K entry cap per existing pattern).
- **FR-5**: Uses `fuzzy-matcher` crate (already in Cargo.toml) to score and rank results. Higher-scored matches appear first.
- **FR-6**: Matched characters in each result are highlighted (bold or colored).
- **FR-7**: Results show relative path from root.
- **FR-8**: `Enter` on a selected result navigates the tree: expands parent directories as needed, selects the matched file, and closes the overlay.
- **FR-9**: `Esc` closes the overlay without navigation.
- **FR-10**: `j`/`k` or `↑`/`↓` navigate the results list while the input remains editable.
- **FR-11**: Results update in real-time as the user types.

### Inline Tree Filter (`/`)
- **FR-12**: `/` activates filter mode — a text input appears in the status bar area.
- **FR-13**: As the user types, non-matching tree items are hidden. Parent directories of matching items remain visible for context.
- **FR-14**: Matching is case-insensitive substring match on filename.
- **FR-15**: `Esc` clears the filter and restores the full tree.
- **FR-16**: `Enter` accepts the filter and returns to normal navigation within the filtered view.
- **FR-17**: Navigation keys (`j`/`k`) work on the filtered tree.

### New AppMode
- **FR-18**: Add `AppMode::Search` variant for the fuzzy finder overlay state.
- **FR-19**: Add `AppMode::Filter` variant (or integrate filter state into `TreeState`) for inline filtering.

## Non-Functional Requirements
- **NFR-1**: Index building must handle directories with 10K+ files without freezing the UI (use existing iterative stack-based walk with cap).
- **NFR-2**: Fuzzy matching must feel instant (<50ms) for typical project sizes (up to 10K files).
- **NFR-3**: Filter updates must re-render within a single frame (~16ms).

## Acceptance Criteria
- [ ] `Ctrl+P` opens centered fuzzy finder modal, typing filters results in real-time
- [ ] Matched characters are visually highlighted in results
- [ ] `Enter` navigates tree to selected result (expanding parents as needed)
- [ ] `Esc` closes fuzzy finder without side effects
- [ ] `/` activates inline filter; non-matching items hidden; parents preserved
- [ ] `Esc` from filter restores full tree
- [ ] Index is lazily built and cached; invalidated on tree mutations
- [ ] All existing tests pass; new unit tests for search/filter logic
- [ ] Works in KubeFlow/Jupyter web terminals

## Out of Scope
- Regex-based search
- File content search (grep)
- Search history / recent files
- Async/background indexing (synchronous with cap is sufficient)
