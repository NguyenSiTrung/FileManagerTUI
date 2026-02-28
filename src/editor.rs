use std::path::PathBuf;
use std::time::Instant;

/// A single reversible edit action in the editor.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum EditorAction {
    /// A single character was inserted at (line, col).
    InsertChar { line: usize, col: usize, ch: char },
    /// A single character was deleted at (line, col).
    DeleteChar { line: usize, col: usize, ch: char },
    /// A line was split at (line, col) — Enter key.
    SplitLine {
        line: usize,
        col: usize,
        indent: String,
    },
    /// Two lines were joined (line+1 was appended to line).
    JoinLine { line: usize, col: usize },
    /// A group of consecutive character inserts (for undo grouping).
    InsertGroup {
        line: usize,
        start_col: usize,
        chars: String,
    },
    /// A group of consecutive character deletes (for undo grouping).
    DeleteGroup {
        line: usize,
        start_col: usize,
        chars: String,
    },
    /// A line was inserted (from paste or other operation).
    InsertLine { line: usize, content: String },
    /// A line was removed (from cut or other operation).
    RemoveLine { line: usize, content: String },
    /// A compound action (multiple sub-actions treated as one undo step).
    Compound { actions: Vec<EditorAction> },
}

/// State for the find/replace bar.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct EditorFind {
    /// Current search query.
    pub query: String,
    /// Cursor position within the query string.
    pub query_cursor: usize,
    /// Replacement string (when in replace mode).
    pub replacement: String,
    /// Cursor position within the replacement string.
    pub replacement_cursor: usize,
    /// All match positions as (line, col) pairs.
    pub matches: Vec<(usize, usize)>,
    /// Index of the current match in `matches`.
    pub current_match: usize,
    /// Whether the find bar is active.
    pub active: bool,
    /// Whether replace mode is active (Ctrl+H).
    pub replace_mode: bool,
    /// Whether the cursor is in the replacement field (vs find field).
    pub in_replace_field: bool,
}

/// Full state for the text editor.
#[derive(Debug)]
#[allow(dead_code)]
pub struct EditorState {
    /// Lines of text in the buffer.
    pub buffer: Vec<String>,
    /// Current cursor line (0-indexed).
    pub cursor_line: usize,
    /// Current cursor column (0-indexed).
    pub cursor_col: usize,
    /// Whether the buffer has been modified since the last save.
    pub modified: bool,
    /// Path to the file being edited.
    pub file_path: PathBuf,
    /// Vertical scroll offset (line index of topmost visible line).
    pub scroll_offset: usize,
    /// Undo stack of edit actions.
    pub undo_stack: Vec<EditorAction>,
    /// Current position in the undo stack (for redo support).
    pub undo_index: usize,
    /// Editor-specific clipboard (separate from file manager clipboard).
    pub editor_clipboard: Vec<String>,
    /// Find/replace state.
    pub find_state: EditorFind,
    /// Visible height of the editor area (set during render).
    pub visible_height: usize,
    /// Timestamp of the last character insert/delete (for grouping).
    pub last_edit_time: Option<Instant>,
    /// Whether we are currently building a group for undo.
    pub grouping_active: bool,
    /// The chars accumulated in the current group.
    pub current_group: String,
    /// Line where the current group started.
    pub group_start_line: usize,
    /// Column where the current group started.
    pub group_start_col: usize,
    /// Whether the current group is a deletion group (vs insert).
    pub group_is_delete: bool,
}

/// Maximum entries in the undo stack.
#[allow(dead_code)]
const MAX_UNDO_ENTRIES: usize = 1000;

/// Grouping timeout: consecutive edits within this duration are grouped.
#[allow(dead_code)]
const GROUPING_TIMEOUT_MS: u128 = 500;

#[allow(dead_code)]
impl EditorState {
    /// Create a new EditorState from raw file content and path.
    pub fn new(content: &str, file_path: PathBuf) -> Self {
        let buffer: Vec<String> = if content.is_empty() {
            vec![String::new()]
        } else {
            content.lines().map(String::from).collect()
        };
        // If the content ends with a newline, add an empty trailing line
        // (this preserves the trailing newline on save).
        let buffer = if !content.is_empty() && content.ends_with('\n') && !buffer.is_empty() {
            let mut b = buffer;
            b.push(String::new());
            b
        } else if buffer.is_empty() {
            vec![String::new()]
        } else {
            buffer
        };

        Self {
            buffer,
            cursor_line: 0,
            cursor_col: 0,
            modified: false,
            file_path,
            scroll_offset: 0,
            undo_stack: Vec::new(),
            undo_index: 0,
            editor_clipboard: Vec::new(),
            find_state: EditorFind::default(),
            visible_height: 24,
            last_edit_time: None,
            grouping_active: false,
            current_group: String::new(),
            group_start_line: 0,
            group_start_col: 0,
            group_is_delete: false,
        }
    }

    /// Load editor state from a file path.
    pub fn from_file(path: &std::path::Path) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::new(&content, path.to_path_buf()))
    }

    /// Total number of lines in the buffer.
    pub fn line_count(&self) -> usize {
        self.buffer.len()
    }

    /// Get the length of the current line.
    pub fn current_line_len(&self) -> usize {
        self.buffer
            .get(self.cursor_line)
            .map(|l| l.len())
            .unwrap_or(0)
    }

    /// Clamp cursor position to valid bounds.
    pub fn clamp_cursor(&mut self) {
        if self.cursor_line >= self.buffer.len() {
            self.cursor_line = self.buffer.len().saturating_sub(1);
        }
        let line_len = self.current_line_len();
        if self.cursor_col > line_len {
            self.cursor_col = line_len;
        }
    }

    /// Ensure the viewport scrolls to keep the cursor visible.
    pub fn ensure_cursor_visible(&mut self) {
        let margin = 2usize;
        if self.visible_height == 0 {
            return;
        }
        // Scroll up if cursor is above the viewport
        if self.cursor_line < self.scroll_offset + margin {
            self.scroll_offset = self.cursor_line.saturating_sub(margin);
        }
        // Scroll down if cursor is below the viewport
        let bottom = self.scroll_offset + self.visible_height;
        if self.cursor_line >= bottom.saturating_sub(margin) {
            self.scroll_offset = self
                .cursor_line
                .saturating_sub(self.visible_height.saturating_sub(margin + 1));
        }
    }

    // ── Undo/Redo infrastructure ──────────────────────────────────────

    /// Flush any pending character group before recording a non-char action.
    pub fn flush_group(&mut self) {
        if self.grouping_active && !self.current_group.is_empty() {
            let action = if self.group_is_delete {
                EditorAction::DeleteGroup {
                    line: self.group_start_line,
                    start_col: self.group_start_col,
                    chars: self.current_group.clone(),
                }
            } else {
                EditorAction::InsertGroup {
                    line: self.group_start_line,
                    start_col: self.group_start_col,
                    chars: self.current_group.clone(),
                }
            };
            self.push_undo_action(action);
        }
        self.grouping_active = false;
        self.current_group.clear();
    }

    /// Push an action onto the undo stack, truncating any redo history.
    fn push_undo_action(&mut self, action: EditorAction) {
        // Truncate redo history
        self.undo_stack.truncate(self.undo_index);
        self.undo_stack.push(action);
        self.undo_index = self.undo_stack.len();
        // Cap the undo stack
        if self.undo_stack.len() > MAX_UNDO_ENTRIES {
            let excess = self.undo_stack.len() - MAX_UNDO_ENTRIES;
            self.undo_stack.drain(..excess);
            self.undo_index = self.undo_stack.len();
        }
    }

    /// Record a single action (non-grouped) in the undo stack.
    pub fn record_action(&mut self, action: EditorAction) {
        self.flush_group();
        self.push_undo_action(action);
    }

    /// Attempt to group a character insert with previous inserts.
    pub fn record_char_insert(&mut self, line: usize, col: usize, ch: char) {
        let now = Instant::now();
        let should_group = self.grouping_active
            && !self.group_is_delete
            && self.group_start_line == line
            && self
                .last_edit_time
                .map(|t| now.duration_since(t).as_millis() < GROUPING_TIMEOUT_MS)
                .unwrap_or(false);

        if should_group {
            self.current_group.push(ch);
        } else {
            self.flush_group();
            self.grouping_active = true;
            self.group_is_delete = false;
            self.group_start_line = line;
            self.group_start_col = col;
            self.current_group = ch.to_string();
        }
        self.last_edit_time = Some(now);
    }

    /// Attempt to group a character delete with previous deletes.
    pub fn record_char_delete(&mut self, line: usize, col: usize, ch: char) {
        let now = Instant::now();
        let should_group = self.grouping_active
            && self.group_is_delete
            && self.group_start_line == line
            && self
                .last_edit_time
                .map(|t| now.duration_since(t).as_millis() < GROUPING_TIMEOUT_MS)
                .unwrap_or(false);

        if should_group {
            // For backspace, chars accumulate in reverse order
            self.current_group.insert(0, ch);
            self.group_start_col = col;
        } else {
            self.flush_group();
            self.grouping_active = true;
            self.group_is_delete = true;
            self.group_start_line = line;
            self.group_start_col = col;
            self.current_group = ch.to_string();
        }
        self.last_edit_time = Some(now);
    }

    // ── Buffer mutation methods ───────────────────────────────────────

    /// Insert a character at the current cursor position.
    pub fn insert_char(&mut self, ch: char) {
        self.record_char_insert(self.cursor_line, self.cursor_col, ch);
        if let Some(line) = self.buffer.get_mut(self.cursor_line) {
            // Find byte index from char column
            let byte_idx = char_to_byte_index(line, self.cursor_col);
            line.insert(byte_idx, ch);
            self.cursor_col += 1;
            self.modified = true;
        }
    }

    /// Delete the character before the cursor (Backspace).
    pub fn delete_char_before(&mut self) {
        if self.cursor_col > 0 {
            // Extract char info before mutating
            let cur_line = self.cursor_line;
            let cur_col = self.cursor_col;
            let (prev_byte_idx, deleted_ch) = {
                let line = &self.buffer[cur_line];
                let byte_idx = char_to_byte_index(line, cur_col);
                let prev_byte_idx = char_to_byte_index(line, cur_col - 1);
                let ch = line[prev_byte_idx..byte_idx].chars().next().unwrap_or(' ');
                (prev_byte_idx, ch)
            };
            self.record_char_delete(cur_line, cur_col - 1, deleted_ch);
            self.buffer[cur_line].remove(prev_byte_idx);
            self.cursor_col -= 1;
            self.modified = true;
        } else if self.cursor_line > 0 {
            // Join with the previous line
            self.flush_group();
            let current_line = self.buffer.remove(self.cursor_line);
            self.cursor_line -= 1;
            let join_col = self.buffer[self.cursor_line].len();
            self.buffer[self.cursor_line].push_str(&current_line);
            self.cursor_col = join_col;
            self.record_action(EditorAction::JoinLine {
                line: self.cursor_line,
                col: join_col,
            });
            self.modified = true;
        }
    }

    /// Delete the character at the cursor (Delete key).
    pub fn delete_char_at(&mut self) {
        let line_len = self.current_line_len();
        if self.cursor_col < line_len {
            // Extract info before mutating
            let cur_line = self.cursor_line;
            let cur_col = self.cursor_col;
            let (byte_idx, ch) = {
                let line = &self.buffer[cur_line];
                let bi = char_to_byte_index(line, cur_col);
                let c = line[bi..].chars().next().unwrap_or(' ');
                (bi, c)
            };
            self.record_char_delete(cur_line, cur_col, ch);
            self.buffer[cur_line].remove(byte_idx);
            self.modified = true;
        } else if self.cursor_line + 1 < self.buffer.len() {
            // Join next line with current
            self.flush_group();
            let next_line = self.buffer.remove(self.cursor_line + 1);
            let join_col = self.buffer[self.cursor_line].len();
            self.buffer[self.cursor_line].push_str(&next_line);
            self.record_action(EditorAction::JoinLine {
                line: self.cursor_line,
                col: join_col,
            });
            self.modified = true;
        }
    }

    /// Split the current line at the cursor position (Enter).
    /// Implements auto-indent: copies leading whitespace from the current line.
    pub fn insert_newline(&mut self) {
        self.flush_group();

        if let Some(line) = self.buffer.get(self.cursor_line) {
            // Detect leading whitespace for auto-indent
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            let byte_idx = char_to_byte_index(line, self.cursor_col);
            let remainder = line[byte_idx..].to_string();
            let new_line = format!("{}{}", indent, remainder);

            self.buffer[self.cursor_line].truncate(byte_idx);
            self.buffer.insert(self.cursor_line + 1, new_line);

            self.record_action(EditorAction::SplitLine {
                line: self.cursor_line,
                col: self.cursor_col,
                indent: indent.clone(),
            });

            self.cursor_line += 1;
            self.cursor_col = indent.len();
            self.modified = true;
        }
    }

    // ── Navigation ────────────────────────────────────────────────────

    /// Move cursor up one line.
    pub fn move_up(&mut self) {
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.clamp_cursor();
            self.ensure_cursor_visible();
        }
    }

    /// Move cursor down one line.
    pub fn move_down(&mut self) {
        if self.cursor_line + 1 < self.buffer.len() {
            self.cursor_line += 1;
            self.clamp_cursor();
            self.ensure_cursor_visible();
        }
    }

    /// Move cursor left one character.
    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.current_line_len();
            self.ensure_cursor_visible();
        }
    }

    /// Move cursor right one character.
    pub fn move_right(&mut self) {
        let line_len = self.current_line_len();
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_line + 1 < self.buffer.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
            self.ensure_cursor_visible();
        }
    }

    /// Move cursor to the start of the current line.
    pub fn move_home(&mut self) {
        self.cursor_col = 0;
    }

    /// Move cursor to the end of the current line.
    pub fn move_end(&mut self) {
        self.cursor_col = self.current_line_len();
    }

    /// Move cursor to the first line.
    pub fn move_to_top(&mut self) {
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.ensure_cursor_visible();
    }

    /// Move cursor to the last line.
    pub fn move_to_bottom(&mut self) {
        self.cursor_line = self.buffer.len().saturating_sub(1);
        self.clamp_cursor();
        self.ensure_cursor_visible();
    }

    /// Move cursor up by one page.
    pub fn page_up(&mut self) {
        let jump = self.visible_height.max(1);
        self.cursor_line = self.cursor_line.saturating_sub(jump);
        self.clamp_cursor();
        self.ensure_cursor_visible();
    }

    /// Move cursor down by one page.
    pub fn page_down(&mut self) {
        let jump = self.visible_height.max(1);
        self.cursor_line = (self.cursor_line + jump).min(self.buffer.len().saturating_sub(1));
        self.clamp_cursor();
        self.ensure_cursor_visible();
    }

    // ── Save ──────────────────────────────────────────────────────────

    /// Save the buffer to disk.
    pub fn save(&mut self) -> std::io::Result<()> {
        let content = self.buffer.join("\n");
        // Only add trailing newline if original file had one
        // (we detect this by checking if last line is empty)
        let content = if self.buffer.last().is_some_and(|l| l.is_empty()) && self.buffer.len() > 1 {
            // The empty last line represents the trailing newline
            // join already puts \n between lines, so the empty last line
            // will produce a trailing \n
            content
        } else {
            content
        };
        std::fs::write(&self.file_path, &content)?;
        self.modified = false;
        Ok(())
    }

    // ── Undo/Redo ─────────────────────────────────────────────────────

    /// Undo the last action.
    pub fn undo(&mut self) {
        self.flush_group();
        if self.undo_index == 0 {
            return;
        }
        self.undo_index -= 1;
        let action = self.undo_stack[self.undo_index].clone();
        self.apply_reverse(&action);
        self.modified = true;
    }

    /// Redo the last undone action.
    pub fn redo(&mut self) {
        self.flush_group();
        if self.undo_index >= self.undo_stack.len() {
            return;
        }
        let action = self.undo_stack[self.undo_index].clone();
        self.apply_forward(&action);
        self.undo_index += 1;
        self.modified = true;
    }

    /// Apply an action in reverse (for undo).
    fn apply_reverse(&mut self, action: &EditorAction) {
        match action {
            EditorAction::InsertChar { line, col, .. } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let byte_idx = char_to_byte_index(l, *col);
                    l.remove(byte_idx);
                }
                self.cursor_line = *line;
                self.cursor_col = *col;
            }
            EditorAction::DeleteChar { line, col, ch } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let byte_idx = char_to_byte_index(l, *col);
                    l.insert(byte_idx, *ch);
                }
                self.cursor_line = *line;
                self.cursor_col = *col + 1;
            }
            EditorAction::SplitLine { line, col, .. } => {
                // Reverse of split: join lines line and line+1
                if *line + 1 < self.buffer.len() {
                    // Remove the indent from the next line before joining
                    let next = self.buffer.remove(*line + 1);
                    let indent_len = next.chars().take_while(|c| c.is_whitespace()).count();
                    let remainder = next[char_to_byte_index(&next, indent_len)..].to_string();
                    let trunc_pos = char_to_byte_index(&self.buffer[*line], *col);
                    self.buffer[*line].truncate(trunc_pos);
                    self.buffer[*line].push_str(&remainder);
                }
                self.cursor_line = *line;
                self.cursor_col = *col;
            }
            EditorAction::JoinLine { line, col } => {
                // Reverse of join: split line at col
                if let Some(l) = self.buffer.get(*line) {
                    let byte_idx = char_to_byte_index(l, *col);
                    let rest = l[byte_idx..].to_string();
                    self.buffer[*line].truncate(byte_idx);
                    self.buffer.insert(*line + 1, rest);
                }
                self.cursor_line = *line + 1;
                self.cursor_col = 0;
            }
            EditorAction::InsertGroup {
                line,
                start_col,
                chars,
            } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let start_byte = char_to_byte_index(l, *start_col);
                    let end_byte = char_to_byte_index(l, *start_col + chars.chars().count());
                    l.replace_range(start_byte..end_byte, "");
                }
                self.cursor_line = *line;
                self.cursor_col = *start_col;
            }
            EditorAction::DeleteGroup {
                line,
                start_col,
                chars,
            } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let byte_idx = char_to_byte_index(l, *start_col);
                    l.insert_str(byte_idx, chars);
                }
                self.cursor_line = *line;
                self.cursor_col = *start_col + chars.chars().count();
            }
            EditorAction::InsertLine { line, .. } => {
                if *line < self.buffer.len() {
                    self.buffer.remove(*line);
                }
                self.cursor_line = line.saturating_sub(1);
                self.clamp_cursor();
            }
            EditorAction::RemoveLine { line, content } => {
                self.buffer.insert(*line, content.clone());
                self.cursor_line = *line;
                self.cursor_col = 0;
            }
            EditorAction::Compound { actions } => {
                for a in actions.iter().rev() {
                    self.apply_reverse(a);
                }
            }
        }
        self.ensure_cursor_visible();
    }

    /// Apply an action forward (for redo).
    fn apply_forward(&mut self, action: &EditorAction) {
        match action {
            EditorAction::InsertChar { line, col, ch } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let byte_idx = char_to_byte_index(l, *col);
                    l.insert(byte_idx, *ch);
                }
                self.cursor_line = *line;
                self.cursor_col = *col + 1;
            }
            EditorAction::DeleteChar { line, col, .. } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let byte_idx = char_to_byte_index(l, *col);
                    l.remove(byte_idx);
                }
                self.cursor_line = *line;
                self.cursor_col = *col;
            }
            EditorAction::SplitLine { line, col, indent } => {
                if let Some(l) = self.buffer.get(*line) {
                    let byte_idx = char_to_byte_index(l, *col);
                    let remainder = l[byte_idx..].to_string();
                    let new_line = format!("{}{}", indent, remainder);
                    self.buffer[*line].truncate(byte_idx);
                    self.buffer.insert(*line + 1, new_line);
                }
                self.cursor_line = *line + 1;
                self.cursor_col = indent.len();
            }
            EditorAction::JoinLine { line, col } => {
                if *line + 1 < self.buffer.len() {
                    let next = self.buffer.remove(*line + 1);
                    self.buffer[*line].push_str(&next);
                }
                self.cursor_line = *line;
                self.cursor_col = *col;
            }
            EditorAction::InsertGroup {
                line,
                start_col,
                chars,
            } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let byte_idx = char_to_byte_index(l, *start_col);
                    l.insert_str(byte_idx, chars);
                }
                self.cursor_line = *line;
                self.cursor_col = *start_col + chars.chars().count();
            }
            EditorAction::DeleteGroup {
                line,
                start_col,
                chars,
            } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let start_byte = char_to_byte_index(l, *start_col);
                    let end_byte = char_to_byte_index(l, *start_col + chars.chars().count());
                    l.replace_range(start_byte..end_byte, "");
                }
                self.cursor_line = *line;
                self.cursor_col = *start_col;
            }
            EditorAction::InsertLine { line, content } => {
                self.buffer.insert(*line, content.clone());
                self.cursor_line = *line;
                self.cursor_col = 0;
            }
            EditorAction::RemoveLine { line, .. } => {
                if *line < self.buffer.len() {
                    self.buffer.remove(*line);
                }
                self.cursor_line = line.saturating_sub(1);
                self.clamp_cursor();
            }
            EditorAction::Compound { actions } => {
                for a in actions {
                    self.apply_forward(a);
                }
            }
        }
        self.ensure_cursor_visible();
    }

    // ── Clipboard ─────────────────────────────────────────────────────

    /// Copy the current line to the editor clipboard.
    pub fn copy_line(&mut self) {
        if let Some(line) = self.buffer.get(self.cursor_line) {
            self.editor_clipboard = vec![line.clone()];
        }
    }

    /// Cut the current line (copy + remove).
    pub fn cut_line(&mut self) {
        if self.buffer.len() <= 1 {
            // Don't remove the last line, just copy and clear it
            self.copy_line();
            if let Some(line) = self.buffer.get_mut(self.cursor_line) {
                let content = line.clone();
                line.clear();
                self.record_action(EditorAction::RemoveLine {
                    line: self.cursor_line,
                    content,
                });
            }
            self.cursor_col = 0;
            self.modified = true;
            return;
        }
        self.copy_line();
        let content = self.buffer.remove(self.cursor_line);
        self.record_action(EditorAction::RemoveLine {
            line: self.cursor_line,
            content,
        });
        if self.cursor_line >= self.buffer.len() {
            self.cursor_line = self.buffer.len().saturating_sub(1);
        }
        self.clamp_cursor();
        self.modified = true;
    }

    /// Paste clipboard content at the cursor position (inserts lines below cursor).
    pub fn paste(&mut self) {
        if self.editor_clipboard.is_empty() {
            return;
        }
        self.flush_group();
        let mut actions = Vec::new();
        for (i, line_content) in self.editor_clipboard.clone().iter().enumerate() {
            let insert_at = self.cursor_line + 1 + i;
            self.buffer.insert(insert_at, line_content.clone());
            actions.push(EditorAction::InsertLine {
                line: insert_at,
                content: line_content.clone(),
            });
        }
        self.record_action(EditorAction::Compound { actions });
        self.cursor_line += self.editor_clipboard.len();
        self.clamp_cursor();
        self.modified = true;
    }

    // ── Tab / Indent ──────────────────────────────────────────────────

    /// Detect the indent unit used in the buffer (default: 4 spaces).
    pub fn detect_indent(&self) -> String {
        // Check first few lines for tab vs spaces
        for line in self.buffer.iter().take(50) {
            if line.starts_with('\t') {
                return "\t".to_string();
            }
            // Count leading spaces
            let spaces: usize = line.chars().take_while(|c| *c == ' ').count();
            if spaces >= 2 {
                // Common indents: 2, 4
                if spaces <= 4 {
                    return " ".repeat(spaces);
                }
                return "    ".to_string(); // default 4 spaces
            }
        }
        "    ".to_string() // default: 4 spaces
    }

    /// Insert one indentation unit at cursor position.
    pub fn insert_tab(&mut self) {
        let indent = self.detect_indent();
        self.flush_group();
        if let Some(line) = self.buffer.get_mut(self.cursor_line) {
            let byte_idx = char_to_byte_index(line, self.cursor_col);
            line.insert_str(byte_idx, &indent);
            let old_col = self.cursor_col;
            self.cursor_col += indent.chars().count();
            self.record_action(EditorAction::InsertGroup {
                line: self.cursor_line,
                start_col: old_col,
                chars: indent,
            });
            self.modified = true;
        }
    }

    /// Remove one indentation level from the beginning of the current line (Shift+Tab).
    pub fn dedent(&mut self) {
        let indent = self.detect_indent();
        let indent_len = indent.len();
        if let Some(line) = self.buffer.get_mut(self.cursor_line) {
            let leading_spaces: usize = line.chars().take_while(|c| c.is_whitespace()).count();
            if leading_spaces == 0 {
                return;
            }
            let remove_count = leading_spaces.min(indent_len);
            let removed: String = line.chars().take(remove_count).collect();
            let byte_end = char_to_byte_index(line, remove_count);
            line.replace_range(..byte_end, "");
            self.cursor_col = self.cursor_col.saturating_sub(remove_count);
            self.flush_group();
            self.record_action(EditorAction::DeleteGroup {
                line: self.cursor_line,
                start_col: 0,
                chars: removed,
            });
            self.modified = true;
        }
    }

    // ── Find ──────────────────────────────────────────────────────────

    /// Open the find bar.
    pub fn open_find(&mut self) {
        self.find_state.active = true;
        self.find_state.replace_mode = false;
        self.find_state.in_replace_field = false;
        self.find_state.query.clear();
        self.find_state.query_cursor = 0;
        self.find_state.matches.clear();
        self.find_state.current_match = 0;
    }

    /// Open the find+replace bar.
    pub fn open_find_replace(&mut self) {
        self.find_state.active = true;
        self.find_state.replace_mode = true;
        self.find_state.in_replace_field = false;
        self.find_state.query.clear();
        self.find_state.query_cursor = 0;
        self.find_state.replacement.clear();
        self.find_state.replacement_cursor = 0;
        self.find_state.matches.clear();
        self.find_state.current_match = 0;
    }

    /// Close the find/replace bar.
    pub fn close_find(&mut self) {
        self.find_state.active = false;
    }

    /// Update search matches based on the current query.
    pub fn update_find_matches(&mut self) {
        self.find_state.matches.clear();
        if self.find_state.query.is_empty() {
            return;
        }
        let query = &self.find_state.query;
        for (line_idx, line) in self.buffer.iter().enumerate() {
            let mut start = 0;
            while let Some(pos) = line[start..].find(query.as_str()) {
                self.find_state.matches.push((line_idx, start + pos));
                start += pos + query.len().max(1);
            }
        }
        if !self.find_state.matches.is_empty()
            && self.find_state.current_match >= self.find_state.matches.len()
        {
            self.find_state.current_match = 0;
        }
    }

    /// Jump to the next find match.
    pub fn find_next(&mut self) {
        if self.find_state.matches.is_empty() {
            return;
        }
        self.find_state.current_match =
            (self.find_state.current_match + 1) % self.find_state.matches.len();
        let (line, col) = self.find_state.matches[self.find_state.current_match];
        self.cursor_line = line;
        self.cursor_col = col;
        self.ensure_cursor_visible();
    }

    /// Jump to the previous find match.
    pub fn find_previous(&mut self) {
        if self.find_state.matches.is_empty() {
            return;
        }
        if self.find_state.current_match == 0 {
            self.find_state.current_match = self.find_state.matches.len() - 1;
        } else {
            self.find_state.current_match -= 1;
        }
        let (line, col) = self.find_state.matches[self.find_state.current_match];
        self.cursor_line = line;
        self.cursor_col = col;
        self.ensure_cursor_visible();
    }

    /// Replace the current match and jump to the next.
    pub fn replace_current(&mut self) {
        if self.find_state.matches.is_empty() {
            return;
        }
        let (line, col) = self.find_state.matches[self.find_state.current_match];
        let query_len = self.find_state.query.len();
        let replacement = self.find_state.replacement.clone();

        if let Some(l) = self.buffer.get_mut(line) {
            let byte_start = char_to_byte_index(l, col);
            let byte_end = byte_start + query_len;
            if byte_end <= l.len() {
                let old = l[byte_start..byte_end].to_string();
                l.replace_range(byte_start..byte_end, &replacement);
                self.flush_group();
                self.record_action(EditorAction::Compound {
                    actions: vec![
                        EditorAction::DeleteGroup {
                            line,
                            start_col: col,
                            chars: old,
                        },
                        EditorAction::InsertGroup {
                            line,
                            start_col: col,
                            chars: replacement,
                        },
                    ],
                });
                self.modified = true;
            }
        }
        self.update_find_matches();
        if !self.find_state.matches.is_empty() {
            // Clamp current_match
            if self.find_state.current_match >= self.find_state.matches.len() {
                self.find_state.current_match = 0;
            }
            let (nl, nc) = self.find_state.matches[self.find_state.current_match];
            self.cursor_line = nl;
            self.cursor_col = nc;
            self.ensure_cursor_visible();
        }
    }

    /// Replace all matches at once. Returns the number of replacements.
    pub fn replace_all(&mut self) -> usize {
        if self.find_state.matches.is_empty() {
            return 0;
        }
        let query = self.find_state.query.clone();
        let replacement = self.find_state.replacement.clone();
        let mut total_count = 0;
        let mut actions = Vec::new();

        for line_idx in 0..self.buffer.len() {
            let line = self.buffer[line_idx].clone();
            let mut start = 0;
            let mut new_line = String::new();
            let mut line_count = 0;
            while let Some(pos) = line[start..].find(query.as_str()) {
                new_line.push_str(&line[start..start + pos]);
                new_line.push_str(&replacement);
                actions.push(EditorAction::DeleteGroup {
                    line: line_idx,
                    start_col: start + pos,
                    chars: query.clone(),
                });
                actions.push(EditorAction::InsertGroup {
                    line: line_idx,
                    start_col: start + pos,
                    chars: replacement.clone(),
                });
                start += pos + query.len().max(1);
                line_count += 1;
            }
            if line_count > 0 {
                new_line.push_str(&line[start..]);
                self.buffer[line_idx] = new_line;
                total_count += line_count;
            }
        }

        if total_count > 0 {
            self.flush_group();
            self.record_action(EditorAction::Compound { actions });
            self.modified = true;
        }
        self.update_find_matches();
        total_count
    }
}

/// Utility: Convert a char-based column index to a byte index in a string.
#[allow(dead_code)]
fn char_to_byte_index(s: &str, char_col: usize) -> usize {
    s.char_indices()
        .nth(char_col)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_empty_content() {
        let state = EditorState::new("", PathBuf::from("/tmp/test.txt"));
        assert_eq!(state.buffer, vec![""]);
        assert_eq!(state.cursor_line, 0);
        assert_eq!(state.cursor_col, 0);
        assert!(!state.modified);
    }

    #[test]
    fn test_new_with_content() {
        let state = EditorState::new("hello\nworld\n", PathBuf::from("/tmp/test.txt"));
        assert_eq!(state.buffer, vec!["hello", "world", ""]);
        assert!(!state.modified);
    }

    #[test]
    fn test_new_without_trailing_newline() {
        let state = EditorState::new("hello\nworld", PathBuf::from("/tmp/test.txt"));
        assert_eq!(state.buffer, vec!["hello", "world"]);
    }

    #[test]
    fn test_from_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "line1\nline2\n").unwrap();
        let state = EditorState::from_file(&path).unwrap();
        assert_eq!(state.buffer, vec!["line1", "line2", ""]);
        assert_eq!(state.file_path, path);
    }

    #[test]
    fn test_insert_char() {
        let mut state = EditorState::new("hello", PathBuf::from("/tmp/test.txt"));
        state.cursor_col = 5;
        state.insert_char('!');
        assert_eq!(state.buffer[0], "hello!");
        assert_eq!(state.cursor_col, 6);
        assert!(state.modified);
    }

    #[test]
    fn test_insert_char_middle() {
        let mut state = EditorState::new("hllo", PathBuf::from("/tmp/test.txt"));
        state.cursor_col = 1;
        state.insert_char('e');
        assert_eq!(state.buffer[0], "hello");
        assert_eq!(state.cursor_col, 2);
    }

    #[test]
    fn test_delete_char_before() {
        let mut state = EditorState::new("hello", PathBuf::from("/tmp/test.txt"));
        state.cursor_col = 5;
        state.delete_char_before();
        assert_eq!(state.buffer[0], "hell");
        assert_eq!(state.cursor_col, 4);
    }

    #[test]
    fn test_delete_char_before_at_line_start_joins() {
        let mut state = EditorState::new("hello\nworld", PathBuf::from("/tmp/test.txt"));
        state.cursor_line = 1;
        state.cursor_col = 0;
        state.delete_char_before();
        assert_eq!(state.buffer, vec!["helloworld"]);
        assert_eq!(state.cursor_line, 0);
        assert_eq!(state.cursor_col, 5);
    }

    #[test]
    fn test_delete_char_at() {
        let mut state = EditorState::new("hello", PathBuf::from("/tmp/test.txt"));
        state.cursor_col = 0;
        state.delete_char_at();
        assert_eq!(state.buffer[0], "ello");
    }

    #[test]
    fn test_delete_char_at_end_joins() {
        let mut state = EditorState::new("hello\nworld", PathBuf::from("/tmp/test.txt"));
        state.cursor_col = 5;
        state.delete_char_at();
        assert_eq!(state.buffer, vec!["helloworld"]);
    }

    #[test]
    fn test_insert_newline() {
        let mut state = EditorState::new("hello world", PathBuf::from("/tmp/test.txt"));
        state.cursor_col = 5;
        state.insert_newline();
        assert_eq!(state.buffer, vec!["hello", " world"]);
        assert_eq!(state.cursor_line, 1);
        assert_eq!(state.cursor_col, 0);
    }

    #[test]
    fn test_insert_newline_auto_indent() {
        let mut state = EditorState::new("    hello", PathBuf::from("/tmp/test.txt"));
        state.cursor_col = 9;
        state.insert_newline();
        assert_eq!(state.buffer[0], "    hello");
        assert_eq!(state.buffer[1], "    ");
        assert_eq!(state.cursor_col, 4);
    }

    #[test]
    fn test_navigation() {
        let mut state = EditorState::new("line1\nline2\nline3", PathBuf::from("/tmp/test.txt"));
        state.move_down();
        assert_eq!(state.cursor_line, 1);
        state.move_down();
        assert_eq!(state.cursor_line, 2);
        state.move_down();
        assert_eq!(state.cursor_line, 2); // Can't go past last line
        state.move_up();
        assert_eq!(state.cursor_line, 1);

        state.cursor_col = 3;
        state.move_right();
        assert_eq!(state.cursor_col, 4);
        state.move_left();
        assert_eq!(state.cursor_col, 3);

        state.move_home();
        assert_eq!(state.cursor_col, 0);
        state.move_end();
        assert_eq!(state.cursor_col, 5);
    }

    #[test]
    fn test_cursor_clamp_on_line_change() {
        let mut state = EditorState::new("longline\nhi", PathBuf::from("/tmp/test.txt"));
        state.cursor_col = 8;
        state.move_down();
        // Column should clamp to length of "hi" = 2
        assert_eq!(state.cursor_col, 2);
    }

    #[test]
    fn test_page_up_down() {
        let mut state = EditorState::new(
            &(0..50)
                .map(|i| format!("line{}", i))
                .collect::<Vec<_>>()
                .join("\n"),
            PathBuf::from("/tmp/test.txt"),
        );
        state.visible_height = 10;
        state.page_down();
        assert_eq!(state.cursor_line, 10);
        state.page_down();
        assert_eq!(state.cursor_line, 20);
        state.page_up();
        assert_eq!(state.cursor_line, 10);
    }

    #[test]
    fn test_undo_redo_insert_char() {
        let mut state = EditorState::new("hello", PathBuf::from("/tmp/test.txt"));
        state.cursor_col = 5;
        state.insert_char('!');
        assert_eq!(state.buffer[0], "hello!");
        state.flush_group(); // Flush the grouping
        state.undo();
        assert_eq!(state.buffer[0], "hello");
        state.redo();
        assert_eq!(state.buffer[0], "hello!");
    }

    #[test]
    fn test_undo_newline() {
        let mut state = EditorState::new("helloworld", PathBuf::from("/tmp/test.txt"));
        state.cursor_col = 5;
        state.insert_newline();
        assert_eq!(state.buffer, vec!["hello", "world"]);
        state.undo();
        assert_eq!(state.buffer, vec!["helloworld"]);
    }

    #[test]
    fn test_undo_join_line() {
        let mut state = EditorState::new("hello\nworld", PathBuf::from("/tmp/test.txt"));
        state.cursor_line = 1;
        state.cursor_col = 0;
        state.delete_char_before();
        assert_eq!(state.buffer, vec!["helloworld"]);
        state.undo();
        assert_eq!(state.buffer, vec!["hello", "world"]);
    }

    #[test]
    fn test_copy_paste() {
        let mut state = EditorState::new("line1\nline2\nline3", PathBuf::from("/tmp/test.txt"));
        state.cursor_line = 1;
        state.copy_line();
        assert_eq!(state.editor_clipboard, vec!["line2"]);
        state.cursor_line = 2;
        state.paste();
        assert_eq!(state.buffer, vec!["line1", "line2", "line3", "line2"]);
    }

    #[test]
    fn test_cut_paste() {
        let mut state = EditorState::new("line1\nline2\nline3", PathBuf::from("/tmp/test.txt"));
        state.cursor_line = 1;
        state.cut_line();
        assert_eq!(state.buffer, vec!["line1", "line3"]);
        assert_eq!(state.editor_clipboard, vec!["line2"]);
        state.cursor_line = 0;
        state.paste();
        assert_eq!(state.buffer, vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn test_tab_insert() {
        let mut state = EditorState::new("hello", PathBuf::from("/tmp/test.txt"));
        state.cursor_col = 0;
        state.insert_tab();
        assert_eq!(state.buffer[0], "    hello");
        assert_eq!(state.cursor_col, 4);
    }

    #[test]
    fn test_dedent() {
        let mut state = EditorState::new("    hello", PathBuf::from("/tmp/test.txt"));
        state.cursor_col = 4;
        state.dedent();
        assert_eq!(state.buffer[0], "hello");
        assert_eq!(state.cursor_col, 0);
    }

    #[test]
    fn test_save_to_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test_save.txt");
        std::fs::write(&path, "original").unwrap();
        let mut state = EditorState::from_file(&path).unwrap();
        state.cursor_col = 8;
        state.insert_char('!');
        state.save().unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "original!");
        assert!(!state.modified);
    }

    #[test]
    fn test_find_matches() {
        let mut state = EditorState::new("hello world\nhello rust", PathBuf::from("/tmp/test.txt"));
        state.find_state.query = "hello".to_string();
        state.update_find_matches();
        assert_eq!(state.find_state.matches.len(), 2);
        assert_eq!(state.find_state.matches[0], (0, 0));
        assert_eq!(state.find_state.matches[1], (1, 0));
    }

    #[test]
    fn test_find_next_wraps() {
        let mut state = EditorState::new("a b a", PathBuf::from("/tmp/test.txt"));
        state.find_state.query = "a".to_string();
        state.update_find_matches();
        assert_eq!(state.find_state.matches.len(), 2);
        state.find_next(); // Go to second match
        assert_eq!(state.find_state.current_match, 1);
        state.find_next(); // Wrap to first
        assert_eq!(state.find_state.current_match, 0);
    }

    #[test]
    fn test_replace_current() {
        let mut state = EditorState::new("hello world", PathBuf::from("/tmp/test.txt"));
        state.find_state.query = "world".to_string();
        state.find_state.replacement = "rust".to_string();
        state.update_find_matches();
        state.replace_current();
        assert_eq!(state.buffer[0], "hello rust");
    }

    #[test]
    fn test_replace_all() {
        let mut state = EditorState::new("hello hello hello", PathBuf::from("/tmp/test.txt"));
        state.find_state.query = "hello".to_string();
        state.find_state.replacement = "hi".to_string();
        state.update_find_matches();
        let count = state.replace_all();
        assert_eq!(count, 3);
        assert_eq!(state.buffer[0], "hi hi hi");
    }

    #[test]
    fn test_ensure_cursor_visible() {
        let mut state = EditorState::new(
            &(0..50)
                .map(|i| format!("line{}", i))
                .collect::<Vec<_>>()
                .join("\n"),
            PathBuf::from("/tmp/test.txt"),
        );
        state.visible_height = 10;
        state.cursor_line = 30;
        state.ensure_cursor_visible();
        // Scroll should have moved so cursor is visible
        assert!(state.scroll_offset <= state.cursor_line);
        assert!(state.cursor_line < state.scroll_offset + state.visible_height);
    }

    #[test]
    fn test_detect_indent_spaces() {
        let state = EditorState::new("def foo():\n    pass", PathBuf::from("/tmp/test.py"));
        assert_eq!(state.detect_indent(), "    ");
    }

    #[test]
    fn test_detect_indent_tabs() {
        let state = EditorState::new("def foo():\n\tpass", PathBuf::from("/tmp/test.py"));
        assert_eq!(state.detect_indent(), "\t");
    }

    #[test]
    fn test_char_to_byte_index_ascii() {
        assert_eq!(char_to_byte_index("hello", 2), 2);
        assert_eq!(char_to_byte_index("hello", 5), 5);
    }

    #[test]
    fn test_char_to_byte_index_past_end() {
        assert_eq!(char_to_byte_index("hi", 10), 2);
    }
}
