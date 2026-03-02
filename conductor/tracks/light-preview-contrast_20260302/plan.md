# Plan: Light Theme Preview Panel Contrast

## Phase 1: Theme-Aware Syntax Highlighting Default

- [ ] Task 1: Make `syntax_theme_name()` scheme-aware
  - Update `AppConfig::syntax_theme_name()` in `config.rs` to accept the
    current theme scheme and return `"InspiredGitHub"` for `"light"` when
    no user override is set.
  - Update call site in `app.rs` (`App::new()`) to pass the scheme.
  - Sub-tasks:
    - [ ] Update `syntax_theme_name()` signature to accept scheme param
    - [ ] Add light-theme default logic
    - [ ] Update `App::new()` call site

- [ ] Task 2: Handle syntect theme reload on scheme change
  - In `handler.rs`, when the user changes theme scheme via Settings, also
    reload `app.syntax_theme` using the new scheme's appropriate default.
  - Sub-tasks:
    - [ ] Find the scheme-change handler in `handler.rs`
    - [ ] Add `syntax_theme` reload after theme resolution
    - [ ] Verify preview re-renders with new syntax theme

- [ ] Task 3: Tests for syntax theme auto-switching
  - Add unit tests in `config.rs` for:
    - [ ] Default returns `"base16-ocean.dark"` for dark/unset scheme
    - [ ] Default returns `"InspiredGitHub"` for light scheme
    - [ ] User-configured `syntax_theme` overrides both

- [ ] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Theme-Aware Preview Content Colors

- [ ] Task 1: Update code preview functions
  - Add `&ThemeColors` param to `load_highlighted_content()`,
    `highlight_single_line()`, and `load_head_tail_content()`.
  - Replace `Color::DarkGray` line numbers with `theme.preview_line_nr_fg`.
  - Replace `Color::DarkGray` empty-file text with `theme.dim_fg`.
  - Replace `Color::Red` errors with `theme.error_fg`.
  - Replace `Color::Yellow` separator with `theme.warning_fg`.

- [ ] Task 2: Update metadata & directory functions
  - Add `&ThemeColors` param to `load_binary_metadata()`,
    `load_directory_summary()`, `load_directory_summary_shallow()`.
  - Replace `Color::Cyan` labels with `theme.info_fg`.
  - Replace `Color::White` values with `theme.preview_fg`.
  - Replace `Color::DarkGray` dim text with `theme.dim_fg`.
  - Replace `Color::Yellow` hints with `theme.warning_fg`.

- [ ] Task 3: Update notebook content function
  - Add `&ThemeColors` param to `load_notebook_content()`.
  - Replace `Color::Yellow` cell headers with `theme.warning_fg`.
  - Replace `Color::Green` output prefix with `theme.success_fg`.
  - Replace `Color::DarkGray` separators with `theme.dim_fg`.
  - Replace `Color::Red` error tracebacks with `theme.error_fg`.

- [ ] Task 4: Update all call sites in `app.rs`
  - Pass `&self.theme` to every preview content function call in
    `update_preview()` and related methods.

- [ ] Task 5: Update tests in `preview_content.rs`
  - Update all test functions to pass a `ThemeColors` instance.
  - Use `dark_theme()` as default test theme for backward compat.

- [ ] Task: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: Integration & Verification

- [ ] Task 1: Full quality check
  - Run `cargo test` — all tests pass.
  - Run `cargo clippy -- -D warnings` — no warnings.
  - Run `cargo fmt --check` — properly formatted.

- [ ] Task 2: Visual sanity check
  - Build and run with light theme — verify all preview content types
    (code, binary, directory, notebook) have proper contrast.
  - Build and run with dark theme — verify no visual regression.

- [ ] Task: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)
