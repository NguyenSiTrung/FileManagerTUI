# Implementation Plan: Search Action Menu

## Phase 1: State & Data Model

- [ ] Task 1: Add SearchActionState and AppMode::SearchAction
  - Add `SearchActionState` struct with `path: PathBuf`, `display: String`, `is_directory: bool`, `is_binary: bool`
  - Add `AppMode::SearchAction` variant to `AppMode` enum
  - Add `search_action_state: Option<SearchActionState>` field to `App`
  - Add helper method `App::detect_file_type(path) -> (is_directory, is_binary)` using existing `NodeType` check + `is_binary_file()`
  - Add unit tests for `SearchActionState` creation and file type detection

- [ ] Task 2: Modify search_confirm to transition to SearchAction mode
  - Change `App::search_confirm()` to populate `SearchActionState` and set `AppMode::SearchAction` instead of navigating
  - Add `App::search_action_back()` to return to `AppMode::Search` with query/results preserved
  - Add `App::close_search_action()` to close both overlays and return to Normal mode
  - Add unit tests: Enter in search → mode is SearchAction; Esc in SearchAction → mode is Search

## Phase 2: Action Execution Methods

- [ ] Task 1: Implement action methods on App
  - `search_action_navigate()` — navigate_to_path + close
  - `search_action_preview()` — navigate_to_path + focus Preview + close
  - `search_action_edit()` — navigate_to_path + focus Preview + enter_edit_mode + close
  - `search_action_copy_path()` — copy absolute path string + status message + close
  - `search_action_rename()` — navigate_to_path + open_dialog(Rename) + close search action
  - `search_action_delete()` — navigate_to_path + open_dialog(DeleteConfirm) + close search action
  - `search_action_copy_clipboard()` — set clipboard with path + Copy op + status + close
  - `search_action_cut_clipboard()` — set clipboard with path + Cut op + status + close
  - `search_action_open_terminal(event_tx)` — ensure terminal visible + send `cd <parent>\n` to PTY + close
  - Add unit tests for each action method

## Phase 3: Handler & Input Dispatch

- [ ] Task 1: Add handle_search_action_mode to handler.rs
  - Add `AppMode::SearchAction => handle_search_action_mode(app, key, event_tx)` in main dispatch
  - Map keys: Enter→navigate, p→preview, e→edit, y→copy_path, r→rename, d→delete, c→copy, x→cut, t→terminal
  - Map Esc→search_action_back (return to search)
  - Guard: skip "e" if `is_binary` or `is_directory`, skip "p" if `is_directory`
  - Add handler unit tests for key dispatch and context-aware guards

## Phase 4: Action Menu Widget (UI)

- [ ] Task 1: Create SearchActionWidget component
  - Create `src/components/search_action.rs`
  - Implement widget struct with `SearchActionState` + `ThemeColors` references
  - Build action list dynamically based on `is_directory` and `is_binary` flags
  - Render: file name header, separator, action rows with `[key]  Description` format
  - Use `Clear` + centered `Block` overlay pattern (same as search/dialog)
  - Register module in `src/components/mod.rs`

- [ ] Task 2: Integrate widget into ui.rs
  - Add rendering branch for `AppMode::SearchAction` in the main render function
  - Use existing overlay rendering pattern (render after main layout, on top)
  - Add widget unit tests for rendering with different file types

## Phase 5: Polish & Documentation

- [ ] Task 1: Update help overlay and documentation
  - Add search action menu keybindings to HelpOverlay data
  - Update README.md keybinding table
  - Verify all actions produce appropriate status messages
  - End-to-end test: full flow from Ctrl+P → search → Enter → action → result
