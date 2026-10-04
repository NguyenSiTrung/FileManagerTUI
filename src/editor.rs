use crate::fs::save::{self, FileRevision, SaveError};
use crate::text::{self, TextPosition, TAB_WIDTH};
use std::path::PathBuf;
use std::time::Instant;

/// A single reversible edit action. All `col`/`start_col` fields are UTF-8 bytes.
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
    /// A byte-addressed text replacement retaining only the affected text.
    ReplaceText {
        start: TextPosition,
        removed: String,
        inserted: String,
        before_cursor: TextPosition,
        after_cursor: TextPosition,
    },
}

/// Represents a text selection range in the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Anchor position (where selection started): (line, UTF-8 byte offset).
    pub anchor_line: usize,
    pub anchor_col: usize,
    // The cursor end of the selection moves with Shift+Arrow.
    // The actual cursor_line/cursor_col in EditorState is the "active" end.
}

impl Selection {
    pub fn new(line: usize, col: usize) -> Self {
        Self {
            anchor_line: line,
            anchor_col: col,
        }
    }
}

/// State for the find/replace bar.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct EditorFind {
    /// Current search query.
    pub query: String,
    /// UTF-8 byte cursor position within the query string.
    pub query_cursor: usize,
    /// Replacement string (when in replace mode).
    pub replacement: String,
    /// UTF-8 byte cursor position within the replacement string.
    pub replacement_cursor: usize,
    /// All match positions as (line, UTF-8 byte offset) pairs.
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
    /// Current cursor UTF-8 byte offset (0-indexed).
    pub cursor_col: usize,
    /// Whether the buffer has been modified since the last save.
    pub modified: bool,
    /// Path to the file being edited.
    pub file_path: PathBuf,
    /// Absolute visual-row offset; equals logical-line index when unwrapped.
    pub scroll_offset: usize,
    /// Undo stack of edit actions.
    pub undo_stack: Vec<EditorAction>,
    /// Current position in the undo stack (for redo support).
    pub undo_index: usize,
    /// Editor-specific clipboard (separate from file manager clipboard).
    pub editor_clipboard: Vec<String>,
    clipboard_linewise: bool,
    /// Find/replace state.
    pub find_state: EditorFind,
    /// Visible height of the editor area (set during render).
    pub visible_height: usize,
    /// Code viewport width in display cells, excluding gutter and border.
    pub visible_width: usize,
    /// Logical display-cell offset, independent of vertical visual-row offset.
    pub horizontal_offset: usize,
    /// Optional visual-row wrapping; document coordinates remain UTF-8 bytes.
    pub line_wrap: bool,
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
    group_delete_backward: bool,
    /// Active text selection (None if no selection).
    pub selection: Option<Selection>,
    pub source_revision: Option<FileRevision>,
    pub line_ending: LineEnding,
    pub normalization_required: bool,
    saved_undo_revision: u64,
    saved_serialization_policy: (LineEnding, bool),
    undo_revisions: Vec<u64>,
    next_undo_revision: u64,
    preferred_display_col: Option<usize>,
    viewport_cursor: Option<TextPosition>,
    /// Changes for every buffer mutation, including unflushed grouped edits.
    content_revision: u64,
    viewport_content_revision: u64,
}

/// Serialization policy for ordinary edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
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
        let line_ending = if content.contains("\r\n") {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };
        let normalization_required = {
            let without_crlf = content.replace("\r\n", "");
            without_crlf.contains('\r')
                || (content.contains("\r\n") && without_crlf.contains('\n'))
                || content.contains('\0')
                || content.starts_with('\u{feff}')
        };
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
            clipboard_linewise: false,
            find_state: EditorFind::default(),
            visible_height: 24,
            visible_width: 80,
            horizontal_offset: 0,
            line_wrap: false,
            last_edit_time: None,
            grouping_active: false,
            current_group: String::new(),
            group_start_line: 0,
            group_start_col: 0,
            group_is_delete: false,
            group_delete_backward: false,
            selection: None,
            source_revision: None,
            line_ending,
            normalization_required,
            saved_serialization_policy: (line_ending, normalization_required),
            saved_undo_revision: 0,
            undo_revisions: vec![0],
            next_undo_revision: 1,
            preferred_display_col: None,
            viewport_cursor: None,
            content_revision: 0,
            viewport_content_revision: 0,
        }
    }

    /// Load editor state from a file path.
    pub fn from_file(path: &std::path::Path) -> std::io::Result<Self> {
        let (bytes, revision) = save::load_document(path).map_err(std::io::Error::other)?;
        let content = std::str::from_utf8(&bytes).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsupported encoding; conversion requires explicit confirmation",
            )
        })?;
        let mut editor = Self::new(content, path.to_path_buf());
        editor.source_revision = Some(revision);
        Ok(editor)
    }

    /// Mutation token, including undo/redo and unflushed edits (wraps at u64::MAX).
    /// Read-only consumers can observe edits without copying or hashing content.
    pub fn content_revision(&self) -> u64 {
        self.content_revision
    }

    pub fn cursor_position(&self) -> TextPosition {
        TextPosition {
            line: self.cursor_line,
            byte: self.cursor_col,
        }
    }

    /// Total number of lines in the buffer.
    pub fn line_count(&self) -> usize {
        self.buffer.len()
    }

    /// Width of the line number gutter (digits + space + separator).
    pub fn gutter_width(&self) -> u16 {
        let max_line = self.line_count();
        let digits = if max_line == 0 {
            1
        } else {
            (max_line as f64).log10().floor() as u16 + 1
        };
        digits + 2
    }

    /// Set the cursor to a specific line and column, clamping to valid bounds.
    pub fn set_cursor_position(&mut self, line: usize, col: usize) {
        self.flush_group();
        self.preferred_display_col = None;
        self.cursor_line = line.min(self.buffer.len().saturating_sub(1));
        let line_len = self
            .buffer
            .get(self.cursor_line)
            .map(|l| l.len())
            .unwrap_or(0);
        self.cursor_col =
            text::floor_grapheme_boundary(&self.buffer[self.cursor_line], col.min(line_len));
        self.selection = None;
        self.ensure_cursor_visible();
    }

    /// Set the cursor to a specific line and column without clearing the selection.
    /// Used for mouse drag selection where the anchor stays put.
    pub fn set_cursor_position_for_selection(&mut self, line: usize, col: usize) {
        self.flush_group();
        self.preferred_display_col = None;
        self.cursor_line = line.min(self.buffer.len().saturating_sub(1));
        let line_len = self
            .buffer
            .get(self.cursor_line)
            .map(|l| l.len())
            .unwrap_or(0);
        self.cursor_col =
            text::floor_grapheme_boundary(&self.buffer[self.cursor_line], col.min(line_len));
        self.ensure_cursor_visible();
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
        self.cursor_col = text::floor_grapheme_boundary(
            &self.buffer[self.cursor_line],
            self.cursor_col.min(line_len),
        );
    }

    /// Ensure the viewport scrolls to keep the cursor visible.
    pub fn ensure_cursor_visible(&mut self) {
        let col = text::byte_to_display_col(
            &self.buffer[self.cursor_line],
            self.cursor_col,
            text::TAB_WIDTH,
        );
        if self.line_wrap {
            self.horizontal_offset = 0;
        } else if self.visible_width > 0 {
            if col < self.horizontal_offset {
                self.horizontal_offset = col;
            } else if col >= self.horizontal_offset.saturating_add(self.visible_width) {
                self.horizontal_offset = col + 1 - self.visible_width;
            }
        } else {
            self.horizontal_offset = 0;
        }
        let cursor_row = self.cursor_visual_row();
        let margin = 2usize;
        self.viewport_cursor = Some(self.cursor_position());
        self.viewport_content_revision = self.content_revision;
        if self.visible_height == 0 {
            self.clamp_viewport();
            return;
        }
        // Scroll up if cursor is above the viewport
        let margin = margin.min(self.visible_height.saturating_sub(1) / 2);
        if cursor_row < self.scroll_offset + margin {
            self.scroll_offset = cursor_row.saturating_sub(margin);
        }
        // Scroll down if cursor is below the viewport
        let bottom = self.scroll_offset + self.visible_height;
        if cursor_row >= bottom.saturating_sub(margin) {
            self.scroll_offset =
                cursor_row.saturating_sub(self.visible_height.saturating_sub(margin + 1));
        }
        self.clamp_viewport();
    }

    /// Update code dimensions; follow cursor movement, edits/reflow, and resize.
    /// Unchanged content/cursor/size preserves intentional viewport scrolling.
    pub fn update_viewport(&mut self, width: usize, height: usize) {
        let changed = self.visible_width != width
            || self.visible_height != height
            || self.viewport_cursor != Some(self.cursor_position())
            || self.viewport_content_revision != self.content_revision;
        self.visible_width = width;
        self.visible_height = height;
        if changed {
            self.ensure_cursor_visible();
        } else {
            self.clamp_viewport();
        }
    }

    fn move_visual(&mut self, down: bool, amount: usize, select: bool) {
        if select {
            self.ensure_selection_anchor();
        } else {
            self.selection = None;
        }
        let current = self.cursor_visual_row();
        let (_, mapped) = self.visual_row(current);
        let col =
            text::byte_to_display_col(&self.buffer[self.cursor_line], self.cursor_col, TAB_WIDTH);
        let preferred = *self
            .preferred_display_col
            .get_or_insert(col.saturating_sub(mapped.start));
        let last = self.visual_row_count().saturating_sub(1);
        let mut target = if down {
            current.saturating_add(amount).min(last)
        } else {
            current.saturating_sub(amount)
        };
        while target != current {
            let (line, row) = self.visual_row(target);
            let content = &self.buffer[line];
            let eol = text::byte_to_display_col(content, content.len(), TAB_WIDTH);
            let right = if row.end == eol && row.end - row.start < self.visible_width {
                row.end
            } else {
                row.end.saturating_sub(1).max(row.start)
            };
            let mut byte =
                text::display_col_to_byte(content, (row.start + preferred).min(right), TAB_WIDTH);
            let mut canonical = self.position_visual_row(TextPosition { line, byte });
            if down && canonical <= current {
                // The target's preferred column can be a continuation while a
                // safe position later in that same row (including EOL) exists.
                // Try the glyph's right boundary before skipping the whole row.
                byte = text::next_grapheme_boundary(content, byte);
                canonical = self.position_visual_row(TextPosition { line, byte });
            }
            // Continuation cells have no safe byte cursor. Skip until snapping
            // advances in the requested visual direction, never into a grapheme.
            if (down && canonical > current) || (!down && canonical < current) {
                self.cursor_line = line;
                self.cursor_col = byte;
                break;
            }
            if (down && target == last) || (!down && target == 0) {
                break;
            }
            target = if down { target + 1 } else { target - 1 };
        }
        self.ensure_cursor_visible();
    }

    /// Number of visible find-bar rows, bounded by the actual inner height.
    pub fn find_bar_height(&self, height: usize) -> usize {
        if self.find_state.active {
            (if self.find_state.replace_mode { 2 } else { 1 }).min(height)
        } else {
            0
        }
    }

    /// Count visual rows without allocating document glyphs or row maps.
    pub fn visual_row_count(&self) -> usize {
        if !self.line_wrap || self.visible_width == 0 {
            return self.buffer.len();
        }
        self.buffer
            .iter()
            .map(|line| text::visual_rows(line, self.visible_width, true, true).count())
            .sum()
    }

    /// Map a vertical viewport row to a logical line and display-cell range.
    pub fn visual_row(&self, mut row: usize) -> (usize, text::VisualRow) {
        for (line, content) in self.buffer.iter().enumerate() {
            let mut rows = text::visual_rows(content, self.visible_width, self.line_wrap, true);
            if !self.line_wrap {
                if row == 0 {
                    return (line, rows.next().unwrap());
                }
                row -= 1;
            } else {
                for mapped in rows {
                    if row == 0 {
                        return (line, mapped);
                    }
                    row -= 1;
                }
            }
        }
        let line = self.buffer.len().saturating_sub(1);
        (
            line,
            text::visual_rows(&self.buffer[line], self.visible_width, self.line_wrap, true)
                .last()
                .unwrap(),
        )
    }

    /// Cursor's absolute visual row; exact-width EOL uses the extra cursor slot.
    pub fn cursor_visual_row(&self) -> usize {
        self.position_visual_row(self.cursor_position())
    }

    /// Canonical row of a grapheme-safe document position, independent of scroll.
    fn position_visual_row(&self, position: TextPosition) -> usize {
        if !self.line_wrap || self.visible_width == 0 {
            return position.line;
        }
        let preceding: usize = self.buffer[..position.line]
            .iter()
            .map(|line| text::visual_rows(line, self.visible_width, true, true).count())
            .sum();
        let content = &self.buffer[position.line];
        let col = text::byte_to_display_col(content, position.byte, text::TAB_WIDTH);
        let rows = text::visual_rows(content, self.visible_width, true, true);
        let mut local = 0;
        for (idx, row) in rows.enumerate() {
            local = idx;
            if col >= row.start
                && (col < row.end
                    || position.byte == content.len()
                        && row.end - row.start < self.visible_width
                        && col == row.end)
            {
                break;
            }
        }
        preceding + local
    }

    /// Clamp offsets after size or wrap changes without following the cursor.
    pub fn clamp_viewport(&mut self) {
        self.scroll_offset = self.scroll_offset.min(
            self.visual_row_count()
                .saturating_sub(self.visible_height.max(1)),
        );
        let content = &self.buffer[self.cursor_line];
        let max_col = text::byte_to_display_col(content, content.len(), text::TAB_WIDTH);
        self.horizontal_offset = if self.line_wrap || self.visible_width == 0 {
            0
        } else {
            self.horizontal_offset
                .min(max_col.saturating_add(1).saturating_sub(self.visible_width))
        };
    }

    /// Toggle optional wrapping while keeping the document cursor visible.
    pub fn toggle_wrap(&mut self) {
        self.line_wrap = !self.line_wrap;
        self.preferred_display_col = None;
        self.horizontal_offset = 0;
        self.scroll_offset = 0;
        self.ensure_cursor_visible();
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
        self.undo_revisions.truncate(self.undo_index + 1);
        self.undo_revisions.push(self.next_undo_revision);
        self.next_undo_revision += 1;
        self.undo_stack.push(action);
        self.undo_index = self.undo_stack.len();
        // Cap the undo stack
        if self.undo_stack.len() > MAX_UNDO_ENTRIES {
            let excess = self.undo_stack.len() - MAX_UNDO_ENTRIES;
            self.undo_stack.drain(..excess);
            self.undo_revisions.drain(..excess);
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
            && col == self.group_start_col + self.current_group.len()
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

    /// Record byte-addressed deletion of a complete grapheme.
    fn record_text_delete(&mut self, line: usize, byte: usize, deleted: &str, backward: bool) {
        let now = Instant::now();
        let contiguous = if backward {
            byte + deleted.len() == self.group_start_col
        } else {
            byte == self.group_start_col
        };
        let should_group = self.grouping_active
            && self.group_is_delete
            && self.group_delete_backward == backward
            && self.group_start_line == line
            && contiguous
            && self
                .last_edit_time
                .map(|t| now.duration_since(t).as_millis() < GROUPING_TIMEOUT_MS)
                .unwrap_or(false);
        if should_group {
            if backward {
                self.current_group.insert_str(0, deleted);
                self.group_start_col = byte;
            } else {
                self.current_group.push_str(deleted);
            }
        } else {
            self.flush_group();
            self.grouping_active = true;
            self.group_is_delete = true;
            self.group_delete_backward = backward;
            self.group_start_line = line;
            self.group_start_col = byte;
            self.current_group = deleted.to_string();
        }
        self.last_edit_time = Some(now);
    }

    pub fn record_char_delete(&mut self, line: usize, byte: usize, ch: char) {
        self.record_text_delete(line, byte, &ch.to_string(), true);
    }

    /// Invalidate layout independently of cursor coordinates and undo grouping.
    fn mark_content_changed(&mut self) {
        self.content_revision = self.content_revision.wrapping_add(1);
        self.modified = true;
    }

    // ── Buffer mutation methods ───────────────────────────────────────

    /// Insert literal pasted text without dispatching keys or auto-indenting.
    pub fn insert_text(&mut self, input: &str) -> Result<(), &'static str> {
        if input.len() > 1024 * 1024 {
            return Err("Paste exceeds the 1 MiB limit");
        }
        if input.is_empty() {
            return Ok(());
        }
        self.flush_group();
        self.preferred_display_col = None;
        let before_cursor = self.cursor_position();
        let ((sl, sc), (el, ec)) = self.selection_range().unwrap_or((
            (self.cursor_line, self.cursor_col),
            (self.cursor_line, self.cursor_col),
        ));
        let start = TextPosition { line: sl, byte: sc };
        let end = TextPosition { line: el, byte: ec };
        let removed = self.selected_text();
        let inserted = input.replace("\r\n", "\n");
        let after_cursor = self.replace_text_range(start, end, &inserted);
        self.selection = None;
        self.cursor_line = after_cursor.line;
        self.cursor_col = after_cursor.byte;
        self.record_action(EditorAction::Compound {
            actions: vec![EditorAction::ReplaceText {
                start,
                removed,
                inserted,
                before_cursor,
                after_cursor,
            }],
        });
        self.mark_content_changed();
        self.ensure_cursor_visible();
        Ok(())
    }

    fn text_end(start: TextPosition, input: &str) -> TextPosition {
        let mut lines = input.split('\n');
        let first = lines.next().unwrap_or("");
        let mut end = TextPosition {
            line: start.line,
            byte: start.byte + first.len(),
        };
        for line in lines {
            end.line += 1;
            end.byte = line.len();
        }
        end
    }

    /// Replace an internally validated UTF-8 range without recording history.
    fn replace_text_range(
        &mut self,
        start: TextPosition,
        end: TextPosition,
        input: &str,
    ) -> TextPosition {
        let prefix = self.buffer[start.line][..start.byte].to_string();
        let suffix = self.buffer[end.line][end.byte..].to_string();
        let mut lines: Vec<String> = input.split('\n').map(String::from).collect();
        lines[0].insert_str(0, &prefix);
        if let Some(last) = lines.last_mut() {
            last.push_str(&suffix);
        }
        self.buffer.splice(start.line..=end.line, lines);
        let mut cursor = Self::text_end(start, input);
        let line = &self.buffer[cursor.line];
        if text::floor_grapheme_boundary(line, cursor.byte) != cursor.byte {
            cursor.byte = text::next_grapheme_boundary(line, cursor.byte);
        }
        cursor
    }

    /// Insert a character at the current cursor position.
    /// If there is a selection, delete it first.
    pub fn insert_char(&mut self, ch: char) {
        self.preferred_display_col = None;
        if self.selection.is_some() {
            self.delete_selection();
        }
        self.record_char_insert(self.cursor_line, self.cursor_col, ch);
        if let Some(line) = self.buffer.get_mut(self.cursor_line) {
            // Cursor and action coordinates are UTF-8 byte offsets.
            let byte_idx = self.cursor_col;
            line.insert(byte_idx, ch);
            self.cursor_col += ch.len_utf8();
            if text::floor_grapheme_boundary(line, self.cursor_col) != self.cursor_col {
                self.cursor_col = text::next_grapheme_boundary(line, self.cursor_col);
            }
            self.mark_content_changed();
        }
    }

    /// Delete the character before the cursor (Backspace).
    /// If there is a selection, delete it instead.
    pub fn delete_char_before(&mut self) {
        self.preferred_display_col = None;
        if self.selection.is_some() {
            self.delete_selection();
            return;
        }
        if self.cursor_col > 0 {
            let start =
                text::previous_grapheme_boundary(&self.buffer[self.cursor_line], self.cursor_col);
            let deleted = self.buffer[self.cursor_line][start..self.cursor_col].to_string();
            self.record_text_delete(self.cursor_line, start, &deleted, true);
            self.buffer[self.cursor_line].replace_range(start..self.cursor_col, "");
            self.cursor_col = start;
            self.mark_content_changed();
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
            self.mark_content_changed();
        }
        self.clamp_cursor();
    }

    /// Delete the character at the cursor (Delete key).
    /// If there is a selection, delete it instead.
    pub fn delete_char_at(&mut self) {
        self.preferred_display_col = None;
        if self.selection.is_some() {
            self.delete_selection();
            return;
        }
        let line_len = self.current_line_len();
        if self.cursor_col < line_len {
            let end = text::next_grapheme_boundary(&self.buffer[self.cursor_line], self.cursor_col);
            let deleted = self.buffer[self.cursor_line][self.cursor_col..end].to_string();
            self.record_text_delete(self.cursor_line, self.cursor_col, &deleted, false);
            self.buffer[self.cursor_line].replace_range(self.cursor_col..end, "");
            self.mark_content_changed();
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
            self.mark_content_changed();
        }
        self.clamp_cursor();
    }

    /// Split the current line at the cursor position (Enter).
    /// Implements auto-indent: copies leading whitespace from the current line.
    /// If there is a selection, delete it first.
    pub fn insert_newline(&mut self) {
        self.preferred_display_col = None;
        if self.selection.is_some() {
            self.delete_selection();
        }
        self.flush_group();

        if let Some(line) = self.buffer.get(self.cursor_line) {
            // Detect leading whitespace for auto-indent
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            let byte_idx = self.cursor_col;
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
            self.mark_content_changed();
        }
    }

    // ── Navigation ────────────────────────────────────────────────────

    /// Move cursor up one line (clears selection).
    pub fn move_up(&mut self) {
        self.flush_group();
        if self.line_wrap && self.visible_width > 0 {
            self.move_visual(false, 1, false);
            return;
        }
        let display_col = *self.preferred_display_col.get_or_insert_with(|| {
            text::byte_to_display_col(&self.buffer[self.cursor_line], self.cursor_col, TAB_WIDTH)
        });
        self.selection = None;
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col =
                text::display_col_to_byte(&self.buffer[self.cursor_line], display_col, TAB_WIDTH);
            self.ensure_cursor_visible();
        }
    }

    /// Move cursor down one line (clears selection).
    pub fn move_down(&mut self) {
        self.flush_group();
        if self.line_wrap && self.visible_width > 0 {
            self.move_visual(true, 1, false);
            return;
        }
        let display_col = *self.preferred_display_col.get_or_insert_with(|| {
            text::byte_to_display_col(&self.buffer[self.cursor_line], self.cursor_col, TAB_WIDTH)
        });
        self.selection = None;
        if self.cursor_line + 1 < self.buffer.len() {
            self.cursor_line += 1;
            self.cursor_col =
                text::display_col_to_byte(&self.buffer[self.cursor_line], display_col, TAB_WIDTH);
            self.ensure_cursor_visible();
        }
    }

    /// Move cursor left one character (clears selection).
    pub fn move_left(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
        self.selection = None;
        if self.cursor_col > 0 {
            self.cursor_col =
                text::previous_grapheme_boundary(&self.buffer[self.cursor_line], self.cursor_col);
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.current_line_len();
            self.ensure_cursor_visible();
        }
        self.ensure_cursor_visible();
    }

    /// Move cursor right one character (clears selection).
    pub fn move_right(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
        self.selection = None;
        let line_len = self.current_line_len();
        if self.cursor_col < line_len {
            self.cursor_col =
                text::next_grapheme_boundary(&self.buffer[self.cursor_line], self.cursor_col);
        } else if self.cursor_line + 1 < self.buffer.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
            self.ensure_cursor_visible();
        }
        self.ensure_cursor_visible();
    }

    /// Move cursor to the start of the current line (clears selection).
    pub fn move_home(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
        self.selection = None;
        self.cursor_col = 0;
        self.ensure_cursor_visible();
    }

    /// Move cursor to the end of the current line (clears selection).
    pub fn move_end(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
        self.selection = None;
        self.cursor_col = self.current_line_len();
        self.ensure_cursor_visible();
    }

    /// Move cursor to the first line (clears selection).
    pub fn move_to_top(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
        self.selection = None;
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.ensure_cursor_visible();
    }

    /// Move cursor to the last line (clears selection).
    pub fn move_to_bottom(&mut self) {
        self.flush_group();
        let display_col = *self.preferred_display_col.get_or_insert_with(|| {
            text::byte_to_display_col(&self.buffer[self.cursor_line], self.cursor_col, TAB_WIDTH)
        });
        self.selection = None;
        self.cursor_line = self.buffer.len().saturating_sub(1);
        self.cursor_col =
            text::display_col_to_byte(&self.buffer[self.cursor_line], display_col, TAB_WIDTH);
        self.ensure_cursor_visible();
    }

    /// Move cursor up by one page (clears selection).
    pub fn page_up(&mut self) {
        self.flush_group();
        if self.line_wrap && self.visible_width > 0 {
            self.move_visual(false, self.visible_height.max(1), false);
            return;
        }
        let display_col = *self.preferred_display_col.get_or_insert_with(|| {
            text::byte_to_display_col(&self.buffer[self.cursor_line], self.cursor_col, TAB_WIDTH)
        });
        self.selection = None;
        let jump = self.visible_height.max(1);
        self.cursor_line = self.cursor_line.saturating_sub(jump);
        self.cursor_col =
            text::display_col_to_byte(&self.buffer[self.cursor_line], display_col, TAB_WIDTH);
        self.ensure_cursor_visible();
    }

    /// Move cursor down by one page (clears selection).
    pub fn page_down(&mut self) {
        self.flush_group();
        if self.line_wrap && self.visible_width > 0 {
            self.move_visual(true, self.visible_height.max(1), false);
            return;
        }
        let display_col = *self.preferred_display_col.get_or_insert_with(|| {
            text::byte_to_display_col(&self.buffer[self.cursor_line], self.cursor_col, TAB_WIDTH)
        });
        self.selection = None;
        let jump = self.visible_height.max(1);
        self.cursor_line = (self.cursor_line + jump).min(self.buffer.len().saturating_sub(1));
        self.cursor_col =
            text::display_col_to_byte(&self.buffer[self.cursor_line], display_col, TAB_WIDTH);
        self.ensure_cursor_visible();
    }

    // ── Selection-aware navigation (Shift+Arrow) ─────────────────────

    /// Ensure a selection anchor exists; if not, set it at the current cursor pos.
    fn ensure_selection_anchor(&mut self) {
        if self.selection.is_none() {
            self.selection = Some(Selection::new(self.cursor_line, self.cursor_col));
        }
    }

    /// Extend selection upward one line.
    pub fn select_up(&mut self) {
        self.flush_group();
        if self.line_wrap && self.visible_width > 0 {
            self.move_visual(false, 1, true);
            return;
        }
        let display_col = *self.preferred_display_col.get_or_insert_with(|| {
            text::byte_to_display_col(&self.buffer[self.cursor_line], self.cursor_col, TAB_WIDTH)
        });
        self.ensure_selection_anchor();
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col =
                text::display_col_to_byte(&self.buffer[self.cursor_line], display_col, TAB_WIDTH);
            self.ensure_cursor_visible();
        }
    }

    /// Extend selection downward one line.
    pub fn select_down(&mut self) {
        self.flush_group();
        if self.line_wrap && self.visible_width > 0 {
            self.move_visual(true, 1, true);
            return;
        }
        let display_col = *self.preferred_display_col.get_or_insert_with(|| {
            text::byte_to_display_col(&self.buffer[self.cursor_line], self.cursor_col, TAB_WIDTH)
        });
        self.ensure_selection_anchor();
        if self.cursor_line + 1 < self.buffer.len() {
            self.cursor_line += 1;
            self.cursor_col =
                text::display_col_to_byte(&self.buffer[self.cursor_line], display_col, TAB_WIDTH);
            self.ensure_cursor_visible();
        }
    }

    /// Extend selection left one character.
    pub fn select_left(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
        self.ensure_selection_anchor();
        if self.cursor_col > 0 {
            self.cursor_col =
                text::previous_grapheme_boundary(&self.buffer[self.cursor_line], self.cursor_col);
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.current_line_len();
            self.ensure_cursor_visible();
        }
        self.ensure_cursor_visible();
    }

    /// Extend selection right one character.
    pub fn select_right(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
        self.ensure_selection_anchor();
        let line_len = self.current_line_len();
        if self.cursor_col < line_len {
            self.cursor_col =
                text::next_grapheme_boundary(&self.buffer[self.cursor_line], self.cursor_col);
        } else if self.cursor_line + 1 < self.buffer.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
            self.ensure_cursor_visible();
        }
        self.ensure_cursor_visible();
    }

    /// Extend selection to start of current line.
    pub fn select_home(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
        self.ensure_selection_anchor();
        self.cursor_col = 0;
        self.ensure_cursor_visible();
    }

    /// Extend selection to end of current line.
    pub fn select_end(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
        self.ensure_selection_anchor();
        self.cursor_col = self.current_line_len();
        self.ensure_cursor_visible();
    }

    /// Extend selection to beginning of document.
    pub fn select_to_top(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
        self.ensure_selection_anchor();
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.ensure_cursor_visible();
    }

    /// Extend selection to end of document.
    pub fn select_to_bottom(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
        self.ensure_selection_anchor();
        self.cursor_line = self.buffer.len().saturating_sub(1);
        self.cursor_col = self.current_line_len();
        self.ensure_cursor_visible();
    }

    /// Extend selection up by one page.
    pub fn select_page_up(&mut self) {
        self.flush_group();
        if self.line_wrap && self.visible_width > 0 {
            self.move_visual(false, self.visible_height.max(1), true);
            return;
        }
        let display_col = *self.preferred_display_col.get_or_insert_with(|| {
            text::byte_to_display_col(&self.buffer[self.cursor_line], self.cursor_col, TAB_WIDTH)
        });
        self.ensure_selection_anchor();
        let jump = self.visible_height.max(1);
        self.cursor_line = self.cursor_line.saturating_sub(jump);
        self.cursor_col =
            text::display_col_to_byte(&self.buffer[self.cursor_line], display_col, TAB_WIDTH);
        self.ensure_cursor_visible();
    }

    /// Extend selection down by one page.
    pub fn select_page_down(&mut self) {
        self.flush_group();
        if self.line_wrap && self.visible_width > 0 {
            self.move_visual(true, self.visible_height.max(1), true);
            return;
        }
        let display_col = *self.preferred_display_col.get_or_insert_with(|| {
            text::byte_to_display_col(&self.buffer[self.cursor_line], self.cursor_col, TAB_WIDTH)
        });
        self.ensure_selection_anchor();
        let jump = self.visible_height.max(1);
        self.cursor_line = (self.cursor_line + jump).min(self.buffer.len().saturating_sub(1));
        self.cursor_col =
            text::display_col_to_byte(&self.buffer[self.cursor_line], display_col, TAB_WIDTH);
        self.ensure_cursor_visible();
    }

    /// Select all text in the buffer (Ctrl+A).
    pub fn select_all(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
        self.selection = Some(Selection::new(0, 0));
        self.cursor_line = self.buffer.len().saturating_sub(1);
        self.cursor_col = self.current_line_len();
        self.ensure_cursor_visible();
    }

    // ── Selection helpers ─────────────────────────────────────────────

    /// Get the ordered (start, end) of the current selection as ((line, col), (line, col)).
    /// Returns None if there is no selection.
    pub fn selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let sel = self.selection.as_ref()?;
        let a = (sel.anchor_line, sel.anchor_col);
        let b = (self.cursor_line, self.cursor_col);
        if a <= b {
            Some((a, b))
        } else {
            Some((b, a))
        }
    }

    /// Check if a character position (line, col) is within the current selection.
    pub fn is_selected(&self, line: usize, col: usize) -> bool {
        if let Some(((sl, sc), (el, ec))) = self.selection_range() {
            if line < sl || line > el {
                return false;
            }
            if line == sl && line == el {
                return col >= sc && col < ec;
            }
            if line == sl {
                return col >= sc;
            }
            if line == el {
                return col < ec;
            }
            true // line is strictly between start and end
        } else {
            false
        }
    }

    /// Get the selected text as a String. Returns empty string if no selection.
    pub fn selected_text(&self) -> String {
        let range = match self.selection_range() {
            Some(r) => r,
            None => return String::new(),
        };
        let ((sl, sc), (el, ec)) = range;
        if sl == el {
            // Single-line selection
            if let Some(line) = self.buffer.get(sl) {
                let start = sc;
                let end = ec;
                return line[start..end].to_string();
            }
            return String::new();
        }
        // Multi-line selection
        let mut result = String::new();
        for line_idx in sl..=el {
            if let Some(line) = self.buffer.get(line_idx) {
                if line_idx == sl {
                    let start = sc;
                    result.push_str(&line[start..]);
                    result.push('\n');
                } else if line_idx == el {
                    let end = ec;
                    result.push_str(&line[..end]);
                } else {
                    result.push_str(line);
                    result.push('\n');
                }
            }
        }
        result
    }

    /// Delete the currently selected text and position cursor at the start of selection.
    /// Records a compound undo action. Clears the selection afterwards.
    pub fn delete_selection(&mut self) {
        self.preferred_display_col = None;
        let range = match self.selection_range() {
            Some(r) => r,
            None => return,
        };
        let ((sl, sc), (el, ec)) = range;
        self.selection = None;
        self.flush_group();

        if sl == el {
            // Single-line deletion
            if let Some(line) = self.buffer.get(sl) {
                let start = sc;
                let end = ec;
                let deleted = line[start..end].to_string();
                self.buffer[sl].replace_range(start..end, "");
                self.record_action(EditorAction::DeleteGroup {
                    line: sl,
                    start_col: sc,
                    chars: deleted,
                });
            }
        } else {
            // Multi-line deletion: we'll build a compound action
            let mut actions = Vec::new();

            // Collect the tail of the end line (part after ec)
            let end_tail = if let Some(line) = self.buffer.get(el) {
                let end_byte = ec;
                line[end_byte..].to_string()
            } else {
                String::new()
            };

            // Remove lines from el down to sl+1 (in reverse to keep indices valid)
            for line_idx in (sl + 1..=el).rev() {
                if line_idx < self.buffer.len() {
                    let content = self.buffer.remove(line_idx);
                    actions.push(EditorAction::RemoveLine {
                        line: line_idx,
                        content,
                    });
                }
            }

            // Truncate the start line at sc, then append end_tail
            if let Some(line) = self.buffer.get_mut(sl) {
                let start_byte = sc;
                let deleted_part = line[start_byte..].to_string();
                line.truncate(start_byte);
                line.push_str(&end_tail);
                if !deleted_part.is_empty() {
                    actions.push(EditorAction::DeleteGroup {
                        line: sl,
                        start_col: sc,
                        chars: deleted_part,
                    });
                }
            }

            if !end_tail.is_empty() {
                actions.push(EditorAction::InsertGroup {
                    line: sl,
                    start_col: sc,
                    chars: end_tail,
                });
            }
            if !actions.is_empty() {
                self.record_action(EditorAction::Compound { actions });
            }
        }

        self.cursor_line = sl;
        self.cursor_col = sc;
        self.mark_content_changed();
        self.clamp_cursor();
        self.ensure_cursor_visible();
    }

    /// Apply a batch of byte-position replacements atomically: every range
    /// validates before anything is written (so failure leaves the buffer
    /// and undo stack untouched), then all edits land inside ONE `Compound`
    /// undo entry. `edits` may be unordered and are applied in descending
    /// document order so earlier coordinates stay valid; `primary` is the
    /// index whose post-insert position becomes the cursor.
    pub fn apply_edit_ranges(
        &mut self,
        edits: &[crate::lsp::features::RangeEdit],
        primary: usize,
    ) -> Result<(), &'static str> {
        if edits.is_empty() {
            return Ok(());
        }
        if primary >= edits.len() {
            return Err("primary edit index out of range");
        }
        for e in edits {
            if e.start.line >= self.buffer.len()
                || e.end.line >= self.buffer.len()
                || e.start > e.end
                || e.start.byte > self.buffer[e.start.line].len()
                || e.end.byte > self.buffer[e.end.line].len()
                || !self.buffer[e.start.line].is_char_boundary(e.start.byte)
                || !self.buffer[e.end.line].is_char_boundary(e.end.byte)
            {
                return Err("edit range is invalid");
            }
        }
        let mut order: Vec<usize> = (0..edits.len()).collect();
        order.sort_by_key(|&i| (edits[i].start.line, edits[i].start.byte));
        for pair in order.windows(2) {
            let (a, b) = (&edits[pair[0]], &edits[pair[1]]);
            if a.end > b.start {
                return Err("edits overlap");
            }
        }
        self.flush_group();
        let before_cursor = self.cursor_position();
        let mut actions = Vec::with_capacity(order.len());
        let mut after_cursor = before_cursor;
        for &i in order.iter().rev() {
            let e = &edits[i];
            let removed = self.text_in_range(e.start, e.end);
            let inserted = e.new_text.replace("\r\n", "\n");
            let end = self.replace_text_range(e.start, e.end, &inserted);
            if i == primary {
                after_cursor = end;
            }
            actions.push(EditorAction::ReplaceText {
                start: e.start,
                removed,
                inserted,
                before_cursor,
                after_cursor: end,
            });
        }
        actions.reverse();
        self.selection = None;
        self.cursor_line = after_cursor.line;
        self.cursor_col = after_cursor.byte;
        self.record_action(EditorAction::Compound { actions });
        self.mark_content_changed();
        self.clamp_cursor();
        self.ensure_cursor_visible();
        Ok(())
    }

    /// Byte-slice of the buffer between two positions (positions must be
    /// valid and ordered — validated by `apply_edit_ranges` beforehand).
    fn text_in_range(&self, start: TextPosition, end: TextPosition) -> String {
        if start.line == end.line {
            return self.buffer[start.line][start.byte..end.byte].to_string();
        }
        let mut out = self.buffer[start.line][start.byte..].to_string();
        for line in &self.buffer[start.line + 1..end.line] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.push_str(&self.buffer[end.line][..end.byte]);
        out
    }

    // ── Save ──────────────────────────────────────────────────────────

    /// Save only if the loaded/saved revision still matches disk.
    pub fn save(&mut self) -> Result<(), SaveError> {
        self.save_with_policy(None, false, 0)
    }

    /// Publish a new destination exclusively; never overwrite an existing name.
    pub fn save_as(&mut self, path: &std::path::Path) -> Result<(), SaveError> {
        self.save_with_policy(Some(path), false, 0)
    }

    /// Call only after the user explicitly confirms overwriting external changes.
    ///
    /// Captures the target revision under the default finite legacy budget. A
    /// caller that knows a byte budget should use
    /// [`Self::save_confirmed_overwrite_bounded`].
    pub fn save_confirmed_overwrite(&mut self) -> Result<(), SaveError> {
        self.save_confirmed_overwrite_bounded(save::DEFAULT_LEGACY_LOAD_BUDGET_BYTES)
    }

    /// Explicitly confirmed overwrite that captures the target revision under the
    /// explicit finite `max_bytes` budget. A target grown beyond the budget is a
    /// conflict; it is never clipped, truncated, or partially published, and the
    /// dirty buffer is retained.
    pub fn save_confirmed_overwrite_bounded(&mut self, max_bytes: usize) -> Result<(), SaveError> {
        self.save_with_policy(None, true, max_bytes)
    }

    /// Call only after warning and obtaining explicit normalization confirmation.
    pub fn confirm_normalization(&mut self, ending: LineEnding) {
        self.line_ending = ending;
        self.normalization_required = false;
        self.modified = true;
    }

    /// The exact bytes [`Self::save`] would write for the current buffer.
    ///
    /// Flushes any open undo group first so the result matches what a save
    /// publishes. Used to record a self-write's content identity from memory,
    /// without re-reading the file that was just written.
    pub fn serialized_content(&mut self) -> Vec<u8> {
        self.flush_group();
        let separator = match self.line_ending {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        };
        self.buffer.join(separator).into_bytes()
    }

    fn save_with_policy(
        &mut self,
        destination: Option<&std::path::Path>,
        overwrite: bool,
        overwrite_budget: usize,
    ) -> Result<(), SaveError> {
        self.flush_group();
        if self.normalization_required {
            return Err(SaveError::NormalizationRequired);
        }
        let separator = match self.line_ending {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        };
        let content = self.buffer.join(separator);
        let path = destination.unwrap_or(&self.file_path);
        let revision = if overwrite {
            save::overwrite_document_bounded(path, content.as_bytes(), overwrite_budget)?
        } else {
            save::save_document(
                path,
                content.as_bytes(),
                if destination.is_some() {
                    None
                } else {
                    self.source_revision.as_ref()
                },
            )?
        };
        if let Some(path) = destination {
            self.file_path = path.to_path_buf();
        }
        self.source_revision = Some(revision);
        self.saved_undo_revision = self.undo_revisions[self.undo_index];
        self.saved_serialization_policy = (self.line_ending, self.normalization_required);
        self.modified = false;
        Ok(())
    }

    // ── Undo/Redo ─────────────────────────────────────────────────────

    /// Undo the last action.
    pub fn undo(&mut self) {
        self.selection = None;
        self.preferred_display_col = None;
        self.flush_group();
        if self.undo_index == 0 {
            return;
        }
        self.undo_index -= 1;
        let action = self.undo_stack[self.undo_index].clone();
        self.apply_reverse(&action);
        self.content_revision = self.content_revision.wrapping_add(1);
        self.modified = self.undo_revisions[self.undo_index] != self.saved_undo_revision
            || self.saved_serialization_policy != (self.line_ending, self.normalization_required);
        self.clamp_cursor();
    }

    /// Redo the last undone action.
    pub fn redo(&mut self) {
        self.selection = None;
        self.preferred_display_col = None;
        self.flush_group();
        if self.undo_index >= self.undo_stack.len() {
            return;
        }
        let action = self.undo_stack[self.undo_index].clone();
        self.apply_forward(&action);
        self.content_revision = self.content_revision.wrapping_add(1);
        self.undo_index += 1;
        self.modified = self.undo_revisions[self.undo_index] != self.saved_undo_revision
            || self.saved_serialization_policy != (self.line_ending, self.normalization_required);
        self.clamp_cursor();
    }

    /// Apply an action in reverse (for undo).
    fn apply_reverse(&mut self, action: &EditorAction) {
        match action {
            EditorAction::ReplaceText {
                start,
                removed,
                inserted,
                before_cursor,
                ..
            } => {
                self.replace_text_range(*start, Self::text_end(*start, inserted), removed);
                self.cursor_line = before_cursor.line;
                self.cursor_col = before_cursor.byte;
            }
            EditorAction::InsertChar { line, col, .. } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let byte_idx = *col;
                    l.remove(byte_idx);
                }
                self.cursor_line = *line;
                self.cursor_col = *col;
            }
            EditorAction::DeleteChar { line, col, ch } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let byte_idx = *col;
                    l.insert(byte_idx, *ch);
                }
                self.cursor_line = *line;
                self.cursor_col = *col + ch.len_utf8();
            }
            EditorAction::SplitLine { line, col, indent } => {
                // Reverse of split: join lines line and line+1
                if *line + 1 < self.buffer.len() {
                    // Remove the indent from the next line before joining
                    let next = self.buffer.remove(*line + 1);
                    let remainder = next[indent.len()..].to_string();
                    let trunc_pos = *col;
                    self.buffer[*line].truncate(trunc_pos);
                    self.buffer[*line].push_str(&remainder);
                }
                self.cursor_line = *line;
                self.cursor_col = *col;
            }
            EditorAction::JoinLine { line, col } => {
                // Reverse of join: split line at col
                if let Some(l) = self.buffer.get(*line) {
                    let byte_idx = *col;
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
                    let start_byte = *start_col;
                    let end_byte = *start_col + chars.len();
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
                    let byte_idx = *start_col;
                    l.insert_str(byte_idx, chars);
                }
                self.cursor_line = *line;
                self.cursor_col = *start_col + chars.len();
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
            EditorAction::ReplaceText {
                start,
                removed,
                inserted,
                after_cursor,
                ..
            } => {
                self.replace_text_range(*start, Self::text_end(*start, removed), inserted);
                self.cursor_line = after_cursor.line;
                self.cursor_col = after_cursor.byte;
            }
            EditorAction::InsertChar { line, col, ch } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let byte_idx = *col;
                    l.insert(byte_idx, *ch);
                }
                self.cursor_line = *line;
                self.cursor_col = *col + ch.len_utf8();
                if text::floor_grapheme_boundary(&self.buffer[*line], self.cursor_col)
                    != self.cursor_col
                {
                    self.cursor_col =
                        text::next_grapheme_boundary(&self.buffer[*line], self.cursor_col);
                }
            }
            EditorAction::DeleteChar { line, col, .. } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let byte_idx = *col;
                    l.remove(byte_idx);
                }
                self.cursor_line = *line;
                self.cursor_col = *col;
            }
            EditorAction::SplitLine { line, col, indent } => {
                if let Some(l) = self.buffer.get(*line) {
                    let byte_idx = *col;
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
                    let byte_idx = *start_col;
                    l.insert_str(byte_idx, chars);
                }
                self.cursor_line = *line;
                self.cursor_col = *start_col + chars.len();
                if text::floor_grapheme_boundary(&self.buffer[*line], self.cursor_col)
                    != self.cursor_col
                {
                    self.cursor_col =
                        text::next_grapheme_boundary(&self.buffer[*line], self.cursor_col);
                }
            }
            EditorAction::DeleteGroup {
                line,
                start_col,
                chars,
            } => {
                if let Some(l) = self.buffer.get_mut(*line) {
                    let start_byte = *start_col;
                    let end_byte = *start_col + chars.len();
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

    /// Copy to editor clipboard. If there's a selection, copy the selected text;
    /// otherwise, copy the current line.
    pub fn copy_line(&mut self) {
        if self.selection.is_some() {
            let text = self.selected_text();
            self.editor_clipboard = text.split('\n').map(String::from).collect();
            self.clipboard_linewise = false;
        } else if let Some(line) = self.buffer.get(self.cursor_line) {
            self.editor_clipboard = vec![line.clone()];
            self.clipboard_linewise = true;
        }
    }

    /// Cut to editor clipboard. If there's a selection, cut the selected text;
    /// otherwise, cut the current line.
    pub fn cut_line(&mut self) {
        self.flush_group();
        if self.selection.is_some() {
            self.copy_line();
            self.delete_selection();
            return;
        }
        if self.buffer.len() <= 1 {
            // Don't remove the last line, just copy and clear it
            self.copy_line();
            let end = self.buffer[0].len();
            if end > 0 {
                self.selection = Some(Selection::new(0, 0));
                self.cursor_col = end;
                self.delete_selection();
            }
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
        self.mark_content_changed();
    }

    /// Paste clipboard content. If clipboard was from a selection (inline text),
    /// insert at cursor position. Otherwise, insert lines below cursor.
    pub fn paste(&mut self) {
        if self.editor_clipboard.is_empty() {
            return;
        }
        let text = self.clipboard_paste_text();
        if self.clipboard_linewise && self.selection.is_none() {
            let end = self.buffer[self.cursor_line].len();
            self.set_cursor_position(self.cursor_line, end);
        }
        let _ = self.insert_text(&text);
    }

    /// Produce literal insertion text, adding a separator for whole-line paste
    /// only when there is no selection to replace.
    pub fn clipboard_paste_text(&self) -> String {
        if self.editor_clipboard.is_empty() {
            return String::new();
        }
        let text = self.editor_clipboard.join("\n");
        if self.clipboard_linewise && self.selection.is_none() {
            format!("\n{text}")
        } else {
            text
        }
    }

    /// Exact copy payload; a whole line includes its line terminator.
    pub fn clipboard_text(&self) -> String {
        let mut text = self.editor_clipboard.join("\n");
        if self.clipboard_linewise && !self.editor_clipboard.is_empty() {
            text.push('\n');
        }
        text
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
    /// If there is a selection, delete it first.
    pub fn insert_tab(&mut self) {
        self.preferred_display_col = None;
        if self.selection.is_some() {
            self.delete_selection();
        }
        let indent = self.detect_indent();
        self.flush_group();
        if let Some(line) = self.buffer.get_mut(self.cursor_line) {
            let byte_idx = self.cursor_col;
            line.insert_str(byte_idx, &indent);
            let old_col = self.cursor_col;
            self.cursor_col += indent.len();
            if text::floor_grapheme_boundary(line, self.cursor_col) != self.cursor_col {
                self.cursor_col = text::next_grapheme_boundary(line, self.cursor_col);
            }
            self.record_action(EditorAction::InsertGroup {
                line: self.cursor_line,
                start_col: old_col,
                chars: indent,
            });
            self.mark_content_changed();
        }
    }

    /// Remove one indentation level from the beginning of the current line (Shift+Tab).
    pub fn dedent(&mut self) {
        self.preferred_display_col = None;
        let indent = self.detect_indent();
        let indent_len = indent.len();
        if let Some(line) = self.buffer.get_mut(self.cursor_line) {
            let leading_spaces: usize = line.chars().take_while(|c| c.is_whitespace()).count();
            if leading_spaces == 0 {
                return;
            }
            let remove_count = leading_spaces.min(indent_len);
            let removed: String = line.chars().take(remove_count).collect();
            let byte_end = removed.len();
            line.replace_range(..byte_end, "");
            self.cursor_col = self.cursor_col.saturating_sub(byte_end);
            self.flush_group();
            self.record_action(EditorAction::DeleteGroup {
                line: self.cursor_line,
                start_col: 0,
                chars: removed,
            });
            self.mark_content_changed();
        }
        self.clamp_cursor();
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
        self.flush_group();
        self.preferred_display_col = None;
        if self.find_state.matches.is_empty() {
            return;
        }
        self.find_state.current_match =
            (self.find_state.current_match + 1) % self.find_state.matches.len();
        let (line, col) = self.find_state.matches[self.find_state.current_match];
        self.cursor_line = line;
        self.cursor_col = text::floor_grapheme_boundary(&self.buffer[line], col);
        self.selection = None;
        self.ensure_cursor_visible();
    }

    /// Jump to the previous find match.
    pub fn find_previous(&mut self) {
        self.flush_group();
        self.preferred_display_col = None;
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
        self.cursor_col = text::floor_grapheme_boundary(&self.buffer[line], col);
        self.selection = None;
        self.ensure_cursor_visible();
    }

    /// Replace the current match and jump to the next.
    pub fn replace_current(&mut self) {
        self.preferred_display_col = None;
        if self.find_state.matches.is_empty() {
            return;
        }
        let (line, col) = self.find_state.matches[self.find_state.current_match];
        let query_len = self.find_state.query.len();
        let replacement = self.find_state.replacement.clone();

        if let Some(l) = self.buffer.get_mut(line) {
            let byte_start = col;
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
                self.mark_content_changed();
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
            self.cursor_col = text::floor_grapheme_boundary(&self.buffer[nl], nc);
            self.ensure_cursor_visible();
        }
        self.clamp_cursor();
    }

    /// Replace all matches at once. Returns the number of replacements.
    pub fn replace_all(&mut self) -> usize {
        self.preferred_display_col = None;
        if self.find_state.matches.is_empty() {
            return 0;
        }
        let query = self.find_state.query.clone();
        let replacement = self.find_state.replacement.clone();
        let mut total_count = 0;
        let mut actions = Vec::new();

        self.flush_group();
        for &(line, byte) in self.find_state.matches.iter().rev() {
            self.buffer[line].replace_range(byte..byte + query.len(), &replacement);
            actions.push(EditorAction::DeleteGroup {
                line,
                start_col: byte,
                chars: query.clone(),
            });
            actions.push(EditorAction::InsertGroup {
                line,
                start_col: byte,
                chars: replacement.clone(),
            });
            total_count += 1;
        }

        if total_count > 0 {
            self.flush_group();
            self.record_action(EditorAction::Compound { actions });
            self.mark_content_changed();
        }
        self.update_find_matches();
        self.clamp_cursor();
        total_count
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn read_only_content_revision_tracks_edits_undo_redo_not_cursor_or_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("revision");
        std::fs::write(&path, "abc").unwrap();
        let mut editor = super::EditorState::from_file(&path).unwrap();
        let initial = editor.content_revision();
        editor.move_right();
        assert_eq!(editor.content_revision(), initial);
        editor.insert_char('x');
        let edited = editor.content_revision();
        assert_ne!(edited, initial);
        editor.undo();
        let undone = editor.content_revision();
        assert_ne!(undone, edited);
        editor.redo();
        let redone = editor.content_revision();
        assert_ne!(redone, undone);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        editor.save().unwrap();
        assert_eq!(editor.content_revision(), redone);
    }

    use super::*;

    fn assert_split_glyph_directional_progress(content: &str, width: usize) {
        type Navigation = fn(&mut EditorState);
        let paths: [(&str, Navigation, Navigation, bool); 4] = [
            ("arrow", EditorState::move_down, EditorState::move_up, false),
            (
                "selection",
                EditorState::select_down,
                EditorState::select_up,
                true,
            ),
            ("page", EditorState::page_down, EditorState::page_up, false),
            (
                "selected page",
                EditorState::select_page_down,
                EditorState::select_page_up,
                true,
            ),
        ];
        for height in [1, 2] {
            for (name, down, up, selected) in paths {
                let mut editor = EditorState::new(content, "test.txt".into());
                editor.update_viewport(width, height);
                editor.toggle_wrap();
                let mut previous = editor.cursor_visual_row();
                down(&mut editor);
                assert!(
                    editor.cursor_visual_row() > previous,
                    "{content:?} {name} Down trapped at {:?}",
                    editor.cursor_position()
                );
                assert!(content.is_char_boundary(editor.cursor_col));
                assert_eq!(
                    text::floor_grapheme_boundary(content, editor.cursor_col),
                    editor.cursor_col
                );
                if selected {
                    assert!(
                        !editor.selected_text().is_empty(),
                        "{name} did not select beyond split glyph"
                    );
                }
                for _ in 0..content.len() + 2 {
                    if editor.cursor_visual_row() == editor.visual_row_count() - 1 {
                        break;
                    }
                    previous = editor.cursor_visual_row();
                    down(&mut editor);
                    assert!(
                        editor.cursor_visual_row() > previous,
                        "{name} repeated Down trapped"
                    );
                }
                assert_eq!(
                    editor.cursor_visual_row(),
                    editor.visual_row_count() - 1,
                    "{name} did not reach last visual row"
                );
                let bottom = editor.cursor_position();
                down(&mut editor);
                assert_eq!(editor.cursor_position(), bottom, "{name} moved past bottom");
                for _ in 0..content.len() + 2 {
                    if editor.cursor_col == 0 {
                        break;
                    }
                    previous = editor.cursor_visual_row();
                    up(&mut editor);
                    assert!(editor.cursor_visual_row() < previous, "{name} Up trapped");
                    assert_eq!(
                        text::floor_grapheme_boundary(content, editor.cursor_col),
                        editor.cursor_col
                    );
                }
                assert_eq!(editor.cursor_col, 0, "{name} did not return to split glyph");
                if selected {
                    assert_eq!(
                        editor.selected_text(),
                        "",
                        "{name} did not shrink selection symmetrically"
                    );
                }
                up(&mut editor);
                assert_eq!(editor.cursor_col, 0);
                editor.move_end();
                down(&mut editor);
                assert_eq!(editor.cursor_col, content.len());
            }
        }
    }

    #[test]
    fn wrapped_split_tab_navigation_skips_nonrepresentable_rows() {
        for width in [1, 2, 3] {
            assert_split_glyph_directional_progress("\tX", width);
            assert_split_glyph_directional_progress("\t", width);
        }
    }

    #[test]
    fn wrapped_oversized_grapheme_navigation_skips_nonrepresentable_rows() {
        for content in ["中X", "👩‍💻X"] {
            assert_split_glyph_directional_progress(content, 1);
        }
    }

    #[test]
    fn same_byte_delete_reflow_follows_cursor_on_next_viewport_update() {
        let mut editor = EditorState::new("abc中dddd\nmore\nmore", "test.txt".into());
        editor.update_viewport(4, 1);
        editor.toggle_wrap();
        editor.set_cursor_position(0, 3);
        assert_eq!(editor.cursor_visual_row(), 1);
        assert_eq!(editor.scroll_offset, 1);
        editor.delete_char_at();
        assert_eq!(editor.cursor_position(), TextPosition { line: 0, byte: 3 });
        assert_eq!(editor.cursor_visual_row(), 0);
        editor.update_viewport(4, 1);
        assert_eq!(editor.scroll_offset, 0);
        assert_eq!(editor.buffer[0], "abcdddd");
    }

    #[test]
    fn same_byte_grouped_edits_follow_but_unchanged_frames_keep_intentional_scroll() {
        let mut editor = EditorState::new("abcdef\nmore\nmore\nmore", "test.txt".into());
        editor.update_viewport(4, 1);
        editor.toggle_wrap();
        editor.set_cursor_position(0, 2);
        for expected in ["abdef", "abef"] {
            editor.scroll_offset = 3;
            editor.update_viewport(4, 1);
            assert_eq!(
                editor.scroll_offset, 3,
                "unchanged frame defeated intentional scroll"
            );
            editor.delete_char_at();
            assert_eq!(editor.cursor_position(), TextPosition { line: 0, byte: 2 });
            assert_eq!(editor.cursor_visual_row(), 0);
            editor.update_viewport(4, 1);
            assert_eq!(
                editor.scroll_offset, 0,
                "same-byte grouped edit was not followed"
            );
            assert_eq!(editor.buffer[0], expected);
        }
        editor.undo();
        editor.update_viewport(4, 1);
        assert_eq!(editor.buffer[0], "abcdef");
        assert!(editor.scroll_offset <= editor.cursor_visual_row());
        assert!(editor.cursor_visual_row() < editor.scroll_offset + editor.visible_height);
        editor.redo();
        editor.update_viewport(4, 1);
        assert_eq!(editor.buffer[0], "abef");
        assert!(editor.cursor_visual_row() < editor.scroll_offset + editor.visible_height);
        editor.scroll_offset = 2;
        editor.update_viewport(4, 1);
        assert_eq!(editor.scroll_offset, 2);
    }

    #[test]
    fn clipboard_linewise_cut_last_line_undo_and_repeated_copy() {
        let mut e = EditorState::new("only", "test.txt".into());
        e.cut_line();
        assert_eq!(e.buffer, vec![""]);
        e.undo();
        assert_eq!(e.buffer, vec!["only"]);
        e.copy_line();
        assert_eq!(e.clipboard_text(), "only\n");
        e.set_cursor_position(0, 2);
        e.paste();
        assert_eq!(e.buffer, vec!["only", "only"]);
        e.paste();
        assert_eq!(e.buffer, vec!["only", "only", "only"]);
        e.undo();
        assert_eq!(e.buffer, vec!["only", "only"]);
    }

    #[test]
    fn clipboard_empty_selection_does_not_reuse_old_payload() {
        let mut e = EditorState::new("old", "test.txt".into());
        e.copy_line();
        e.selection = Some(Selection::new(0, 0));
        e.copy_line();
        e.selection = None;
        e.paste();
        assert_eq!(e.buffer, vec!["old"]);
    }

    #[test]
    fn clipboard_inline_trailing_newline_repeated_paste_one_undo() {
        let mut e = EditorState::new("beta\nend", "text.txt".into());
        e.selection = Some(Selection::new(0, 0));
        e.set_cursor_position_for_selection(1, 0);
        e.copy_line();
        e.selection = None;
        e.buffer = vec!["alphaomega".into()];
        e.set_cursor_position(0, 5);
        e.paste();
        assert_eq!(e.buffer, vec!["alphabeta", "omega"]);
        e.undo();
        assert_eq!(e.buffer, vec!["alphaomega"]);
        e.redo();
        e.paste();
        assert_eq!(e.buffer, vec!["alphabeta", "beta", "omega"]);
    }

    #[test]
    fn clipboard_selection_replacement_is_one_undo() {
        let mut e = EditorState::new("中betaomega", "text.txt".into());
        e.selection = Some(Selection::new(0, 3));
        e.set_cursor_position_for_selection(0, 7);
        e.copy_line();
        e.selection = Some(Selection::new(0, 7));
        e.set_cursor_position_for_selection(0, 12);
        e.paste();
        assert_eq!(e.buffer[0], "中betabeta");
        e.undo();
        assert_eq!(e.buffer[0], "中betaomega");
    }

    #[test]
    fn multiline_paste_is_one_undoable_insert() {
        let mut editor = EditorState::new("", "config.yaml".into());
        editor.insert_text("training:\n  lr: 0.001\n").unwrap();
        assert_eq!(editor.buffer.join("\n"), "training:\n  lr: 0.001\n");
        editor.undo();
        assert_eq!(editor.buffer.join("\n"), "");
        assert!(!editor.modified);
        editor.redo();
        assert_eq!(editor.buffer.join("\n"), "training:\n  lr: 0.001\n");
    }

    #[test]
    fn paste_replaces_multiline_selection_and_undo_restores_exact_bytes() {
        let mut editor = EditorState::new("ab中\nold\n尾cd", "text.txt".into());
        editor.set_cursor_position(0, 2);
        editor.selection = Some(Selection::new(0, 2));
        editor.set_cursor_position_for_selection(2, "尾".len());
        editor.insert_text("😀\r\n  e\u{301}\r\n").unwrap();
        assert_eq!(editor.buffer.join("\n"), "ab😀\n  e\u{301}\ncd");
        assert_eq!(editor.cursor_position(), TextPosition { line: 2, byte: 0 });
        editor.undo();
        assert_eq!(editor.buffer.join("\n"), "ab中\nold\n尾cd");
        assert!(!editor.modified);
        editor.redo();
        assert_eq!(editor.buffer.join("\n"), "ab😀\n  e\u{301}\ncd");
    }

    #[test]
    fn paste_keeps_following_text_and_does_not_add_auto_indent() {
        let mut editor = EditorState::new("  beforeAFTER", "text.txt".into());
        editor.set_cursor_position(0, 8);
        editor.insert_text("q\nvalue:\n\n").unwrap();
        assert_eq!(editor.buffer.join("\n"), "  beforeq\nvalue:\n\nAFTER");
        editor.undo();
        assert_eq!(editor.buffer.join("\n"), "  beforeAFTER");
    }

    #[test]
    fn repeated_inline_paste_is_separate_from_typed_undo_group() {
        let mut editor = EditorState::new("omega", "text.txt".into());
        editor.insert_char('a');
        editor.insert_text("β").unwrap();
        editor.insert_text("中").unwrap();
        assert_eq!(editor.buffer[0], "aβ中omega");
        editor.undo();
        assert_eq!(editor.buffer[0], "aβomega");
        editor.undo();
        assert_eq!(editor.buffer[0], "aomega");
        editor.undo();
        assert_eq!(editor.buffer[0], "omega");
        assert!(!editor.modified);
    }

    #[test]
    fn empty_paste_does_not_delete_selection_or_add_history() {
        let mut editor = EditorState::new("keep", "text.txt".into());
        editor.select_all();
        editor.insert_text("").unwrap();
        assert_eq!(editor.selected_text(), "keep");
        assert_eq!(editor.undo_index, 0);
        assert!(!editor.modified);
    }

    #[test]
    fn oversized_paste_is_rejected_before_mutating_selection() {
        let mut editor = EditorState::new("keep", "text.txt".into());
        editor.select_all();
        assert!(editor.insert_text(&"x".repeat(1024 * 1024 + 1)).is_err());
        assert_eq!(editor.selected_text(), "keep");
        assert_eq!(editor.undo_index, 0);
        assert!(!editor.modified);
    }

    #[test]
    fn unicode_indent_insertion_before_combining_mark_keeps_boundary() {
        let mut e = EditorState::new("\u{301}z", "unused".into());
        e.insert_tab();
        assert_eq!(e.cursor_col, 6);
        e.undo();
        assert_eq!(e.buffer[0], "\u{301}z");
        e.redo();
        assert_eq!(e.cursor_col, 6);
        e.undo();
        e.insert_char('e');
        assert_eq!(e.cursor_col, 3);
        e.undo();
        assert_eq!(e.buffer[0], "\u{301}z");
    }

    #[test]
    fn unicode_undo_clears_stale_selection() {
        let mut e = EditorState::new("z", "unused".into());
        e.insert_char('中');
        e.select_left();
        e.undo();
        assert!(e.selection.is_none());
        assert_eq!(e.selected_text(), "");
        e.redo();
        assert_eq!(e.buffer[0], "中z");
    }

    #[test]
    fn unicode_newline_tab_indent_and_byte_undo() {
        let mut e = EditorState::new("\t中e\u{301} z", "unused".into());
        e.set_cursor_position(0, 7);
        e.insert_newline();
        assert_eq!(e.buffer, vec!["\t中e\u{301}", "\t z"]);
        assert_eq!(e.cursor_col, 1);
        e.undo();
        assert_eq!(e.buffer[0], "\t中e\u{301} z");
        e.redo();
        assert_eq!(e.buffer, vec!["\t中e\u{301}", "\t z"]);
        e.move_end();
        e.insert_tab();
        assert_eq!(e.cursor_col, 4);
        e.undo();
        assert_eq!(e.buffer[1], "\t z");
        e.dedent();
        assert_eq!(e.buffer[1], " z");
        e.undo();
        assert_eq!(e.buffer[1], "\t z");
    }

    #[test]
    fn unicode_cursor_setters_and_selection_snap_interior_bytes() {
        let mut e = EditorState::new("中e\u{301}🙂\nx", "unused".into());
        e.set_cursor_position(0, 2);
        assert_eq!(e.cursor_col, 0);
        e.set_cursor_position(0, 4);
        assert_eq!(e.cursor_col, 3);
        e.selection = Some(Selection::new(0, 3));
        e.set_cursor_position_for_selection(0, 9);
        assert_eq!(e.cursor_col, 6);
        assert_eq!(e.selected_text(), "e\u{301}");
        e.select_down();
        assert_eq!(e.cursor_col, 1);
        e.set_cursor_position(99, 99);
        assert_eq!(e.cursor_position(), TextPosition { line: 1, byte: 1 });
    }

    #[test]
    fn unicode_consecutive_grapheme_deletes_group_reversibly() {
        for backward in [false, true] {
            let mut e = EditorState::new("中e\u{301}👩‍💻", "unused".into());
            if backward {
                e.move_end();
                e.delete_char_before();
                e.delete_char_before();
            } else {
                e.delete_char_at();
                e.delete_char_at();
            }
            let after = e.buffer.clone();
            e.undo();
            assert_eq!(e.buffer[0], "中e\u{301}👩‍💻");
            e.redo();
            assert_eq!(e.buffer, after);
        }
    }
    #[test]
    fn unicode_joining_combining_cluster_keeps_cursor_on_boundary() {
        let mut e = EditorState::new("e\n\u{301}z", "unused".into());
        e.set_cursor_position(1, 0);
        e.delete_char_before();
        assert_eq!(e.buffer[0], "e\u{301}z");
        assert_eq!(e.cursor_col, 0);
        e.undo();
        assert_eq!(e.buffer, vec!["e", "\u{301}z"]);
    }
    #[test]
    fn unicode_find_cursor_snaps_to_grapheme_but_replacement_uses_exact_bytes() {
        let mut e = EditorState::new("e\u{301}z", "unused".into());
        e.find_state.query = "\u{301}".into();
        e.find_state.replacement = "x".into();
        e.update_find_matches();
        e.find_next();
        assert_eq!(e.cursor_col, 0);
        e.replace_current();
        assert_eq!(e.buffer[0], "exz");
        e.undo();
        assert_eq!(e.buffer[0], "e\u{301}z");
        assert_eq!(
            text::floor_grapheme_boundary(&e.buffer[0], e.cursor_col),
            e.cursor_col
        );
    }

    #[test]
    fn unicode_multiline_selection_undo_redo_preserves_tail() {
        let mut e = EditorState::new("中abc\n🙂tail\nz", "unused".into());
        e.set_cursor_position(0, 3);
        e.selection = Some(Selection::new(1, 4));
        assert_eq!(e.selected_text(), "abc\n🙂");
        e.delete_selection();
        assert_eq!(e.buffer, vec!["中tail", "z"]);
        e.undo();
        assert_eq!(e.buffer, vec!["中abc", "🙂tail", "z"]);
        e.redo();
        assert_eq!(e.buffer, vec!["中tail", "z"]);
    }

    #[test]
    fn unicode_vertical_movement_preserves_display_column() {
        let mut e = EditorState::new("中a\n\tb\ne\u{301}中z\n", "unused".into());
        e.move_end();
        e.move_down();
        assert_eq!(e.cursor_col, 0);
        e.move_end();
        e.move_up();
        assert_eq!(e.cursor_col, 4);
        e.move_down();
        e.move_down();
        assert_eq!(e.cursor_col, 7);
        e.move_down();
        assert_eq!(e.cursor_col, 0);
    }

    #[test]
    fn unicode_insert_group_uses_bytes() {
        let mut e = EditorState::new("中z", "unused".into());
        e.set_cursor_position(0, 3);
        e.insert_char('é');
        e.insert_char('🙂');
        assert_eq!(e.buffer[0], "中é🙂z");
        assert_eq!(e.cursor_col, 9);
        e.undo();
        assert_eq!(e.buffer[0], "中z");
        e.redo();
        assert_eq!(e.buffer[0], "中é🙂z");
        assert_eq!(e.cursor_col, 9);
    }

    #[test]
    fn unicode_grapheme_movement_selection_deletion() {
        let text = "中e\u{301}👩‍💻z";
        let mut e = EditorState::new(text, "unused".into());
        e.move_right();
        assert_eq!(e.cursor_col, 3);
        e.select_right();
        assert_eq!(e.selected_text(), "e\u{301}");
        e.delete_selection();
        assert_eq!(e.buffer[0], "中👩‍💻z");
        e.undo();
        assert_eq!(e.buffer[0], text);
        e.set_cursor_position(0, 6);
        e.delete_char_at();
        assert_eq!(e.buffer[0], "中e\u{301}z");
        e.undo();
        assert_eq!(e.buffer[0], text);
        e.move_end();
        e.move_left();
        e.delete_char_before();
        assert_eq!(e.buffer[0], "中e\u{301}z");
        e.undo();
        assert_eq!(e.buffer[0], text);
    }

    #[test]
    fn unicode_replace_all_different_lengths_is_reversible() {
        let mut e = EditorState::new("中é éé", "unused".into());
        e.find_state.query = "é".into();
        e.find_state.replacement = "🙂x".into();
        e.update_find_matches();
        assert_eq!(e.find_state.matches, vec![(0, 3), (0, 6), (0, 8)]);
        assert_eq!(e.replace_all(), 3);
        assert_eq!(e.buffer[0], "中🙂x 🙂x🙂x");
        e.undo();
        assert_eq!(e.buffer[0], "中é éé");
        e.redo();
        assert_eq!(e.buffer[0], "中🙂x 🙂x🙂x");
    }

    #[test]
    fn unicode_replace_current_byte_offset() {
        let mut e = EditorState::new("中éz", "unused".into());
        e.find_state.query = "é".into();
        e.find_state.replacement = "a".into();
        e.update_find_matches();
        e.replace_current();
        assert_eq!(e.buffer[0], "中az");
        e.undo();
        assert_eq!(e.buffer[0], "中éz");
    }

    #[test]
    fn normalization_policy_remains_dirty_after_undo() {
        let mut e = EditorState::new("a\r\n", "unused".into());
        e.insert_char('x');
        e.confirm_normalization(LineEnding::Lf);
        e.undo();
        assert!(
            e.modified,
            "undoing text cannot erase unsaved serialization changes"
        );
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_roundtrip_preserves_empty_lf_crlf_and_no_trailing_newline() {
        for bytes in [
            b"".as_slice(),
            b"a",
            b"a\nb",
            b"a\nb\n",
            b"a\r\nb",
            b"a\r\nb\r\n",
            b"\r\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("text");
            std::fs::write(&path, bytes).unwrap();
            let mut e = EditorState::from_file(&path).unwrap();
            e.save().unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            e.insert_char('x');
            e.save().unwrap();
            let mut expected = vec![b'x'];
            expected.extend(bytes);
            assert_eq!(std::fs::read(&path).unwrap(), expected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn save_failures_keep_dirty_buffer_and_saved_revision() {
        for stage in [
            save::Stage::Write,
            save::Stage::Flush,
            save::Stage::Sync,
            save::Stage::Permissions,
            save::Stage::Validate,
            save::Stage::Replace,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("text");
            std::fs::write(&path, "a").unwrap();
            let mut e = EditorState::from_file(&path).unwrap();
            let revision = e.source_revision.clone();
            e.insert_char('x');
            save::inject_failure(stage);
            assert!(e.save().is_err());
            assert!(e.modified);
            assert_eq!(e.buffer, vec!["xa"]);
            assert_eq!(e.source_revision, revision);
            assert_eq!(std::fs::read(&path).unwrap(), b"a");
            e.undo();
            assert!(!e.modified);
        }
    }

    #[test]
    fn mixed_endings_refuse_until_explicit_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text");
        std::fs::write(&path, b"a\r\nb\n").unwrap();
        let mut e = EditorState::from_file(&path).unwrap();
        e.insert_char('x');
        assert!(e.save().unwrap_err().requires_normalization());
        assert!(e.modified);
        assert_eq!(std::fs::read(&path).unwrap(), b"a\r\nb\n");
        e.confirm_normalization(LineEnding::Lf);
        e.save().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"xa\nb\n");
    }

    #[test]
    fn unsupported_encoding_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text");
        std::fs::write(&path, b"\xff\xfea\0").unwrap();
        assert_eq!(
            EditorState::from_file(&path).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"\xff\xfea\0");
    }

    #[cfg(unix)]
    #[test]
    fn saved_revision_survives_history_truncation_without_index_alias() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text");
        std::fs::write(&path, "a").unwrap();
        let mut e = EditorState::from_file(&path).unwrap();
        e.insert_char('x');
        e.save().unwrap();
        for _ in 0..MAX_UNDO_ENTRIES + 2 {
            e.insert_char('y');
            e.flush_group();
        }
        for _ in 0..MAX_UNDO_ENTRIES {
            e.undo();
        }
        assert!(e.modified);
        assert_eq!(e.undo_index, 0);
        e.insert_char('z');
        e.flush_group();
        assert!(e.modified);
    }

    #[cfg(unix)]
    #[test]
    fn save_as_exclusive_and_confirmed_overwrite_keep_path_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text");
        let dest = dir.path().join("dest");
        std::fs::write(&path, "a").unwrap();
        std::fs::write(&dest, "keep").unwrap();
        let mut e = EditorState::from_file(&path).unwrap();
        e.insert_char('x');
        assert!(matches!(e.save_as(&dest), Err(SaveError::AlreadyExists(_))));
        assert_eq!(e.file_path, path);
        assert!(e.modified);
        std::fs::write(&path, "external").unwrap();
        e.save_confirmed_overwrite().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"xa");
        assert_eq!(std::fs::read(&dest).unwrap(), b"keep");
        let new = dir.path().join("new");
        e.save_as(&new).unwrap();
        assert_eq!(e.file_path, new);
        e.insert_char('z');
        e.save().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn confirmed_overwrite_budget_refusal_retains_dirty_buffer_and_original_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text");
        std::fs::write(&path, "a").unwrap();
        let mut e = EditorState::from_file(&path).unwrap();
        e.insert_char('x');
        assert!(e.modified);
        std::fs::write(&path, "grown well beyond the tiny overwrite budget").unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(matches!(
            e.save_confirmed_overwrite_bounded(4),
            Err(SaveError::Conflict(_))
        ));
        assert!(e.modified, "dirty buffer must survive refusal");
        assert_eq!(e.buffer.join("\n"), "xa");
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn confirmed_overwrite_budget_refuses_symlink_and_directory_replacement() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text");
        std::fs::write(&path, "a").unwrap();
        let mut e = EditorState::from_file(&path).unwrap();
        e.insert_char('x');
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(matches!(
            e.save_confirmed_overwrite_bounded(64),
            Err(SaveError::UnsafeTarget { .. })
        ));
        assert!(e.modified);
        assert_eq!(e.buffer.join("\n"), "xa");
        assert!(path.is_dir());

        let dir2 = tempfile::tempdir().unwrap();
        let clobbered = dir2.path().join("text");
        let real = dir2.path().join("real");
        std::fs::write(&clobbered, "a").unwrap();
        let mut e = EditorState::from_file(&clobbered).unwrap();
        e.insert_char('x');
        std::fs::remove_file(&clobbered).unwrap();
        std::fs::write(&real, "keep").unwrap();
        symlink(&real, &clobbered).unwrap();
        assert!(matches!(
            e.save_confirmed_overwrite_bounded(64),
            Err(SaveError::UnsafeTarget { .. })
        ));
        assert!(e.modified);
        assert_eq!(std::fs::read(&real).unwrap(), b"keep");
        assert!(std::fs::symlink_metadata(&clobbered)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn save_preserves_crlf_and_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.txt");
        std::fs::write(&path, b"a\r\nb\r\n").unwrap();
        let mut editor = EditorState::from_file(&path).unwrap();
        editor.insert_char('x');
        editor.save().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"xa\r\nb\r\n");
    }

    #[test]
    fn saved_revision_undo_redo_and_branch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text");
        std::fs::write(&path, "a").unwrap();
        let mut e = EditorState::from_file(&path).unwrap();
        e.insert_char('x');
        e.save().unwrap();
        e.insert_char('y');
        e.undo();
        assert!(!e.modified);
        e.redo();
        assert!(e.modified);
        e.undo();
        e.undo();
        e.insert_char('z');
        e.flush_group();
        assert!(e.modified);
    }

    #[test]
    fn external_conflict_retains_dirty_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text");
        std::fs::write(&path, "a").unwrap();
        let mut e = EditorState::from_file(&path).unwrap();
        e.insert_char('x');
        std::fs::write(&path, "b").unwrap();
        assert!(e.save().is_err());
        assert!(e.modified);
        assert_eq!(e.buffer, vec!["xa"]);
        assert_eq!(std::fs::read(path).unwrap(), b"b");
    }

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
    fn test_byte_boundary_ascii() {
        assert_eq!(text::floor_grapheme_boundary("hello", 2), 2);
        assert_eq!(text::floor_grapheme_boundary("hello", 5), 5);
    }

    #[test]
    fn test_byte_boundary_past_end() {
        assert_eq!(text::floor_grapheme_boundary("hi", 10), 2);
    }

    // ── Batched ranged edits (LSP completion application) ────────────────

    #[test]
    fn apply_edit_ranges_empty_and_bad_primary_are_noops() {
        let mut e = EditorState::new("abc", "f".into());
        assert!(e.apply_edit_ranges(&[], 0).is_ok());
        let edit = crate::lsp::features::RangeEdit {
            start: TextPosition { line: 0, byte: 0 },
            end: TextPosition { line: 0, byte: 1 },
            new_text: "x".into(),
        };
        assert!(e.apply_edit_ranges(&[edit], 1).is_err());
        assert_eq!(e.buffer.join("\n"), "abc");
    }

    #[test]
    fn apply_edit_ranges_rejects_every_invalid_range() {
        let mut e = EditorState::new("ab\ncd", "f".into());
        let mk = |start: TextPosition, end: TextPosition| crate::lsp::features::RangeEdit {
            start,
            end,
            new_text: "x".into(),
        };
        // start > end
        assert!(e
            .apply_edit_ranges(
                &[mk(
                    TextPosition { line: 0, byte: 2 },
                    TextPosition { line: 0, byte: 1 },
                )],
                0,
            )
            .is_err());
        // line out of bounds
        assert!(e
            .apply_edit_ranges(
                &[mk(
                    TextPosition { line: 5, byte: 0 },
                    TextPosition { line: 5, byte: 1 },
                )],
                0,
            )
            .is_err());
        // byte beyond line end
        assert!(e
            .apply_edit_ranges(
                &[mk(
                    TextPosition { line: 0, byte: 0 },
                    TextPosition { line: 0, byte: 99 },
                )],
                0,
            )
            .is_err());
        // non-boundary byte (mid UTF-8 scalar)
        let mut e2 = EditorState::new("é", "f".into());
        assert!(e2
            .apply_edit_ranges(
                &[mk(
                    TextPosition { line: 0, byte: 0 },
                    TextPosition { line: 0, byte: 1 },
                )],
                0,
            )
            .is_err());
        assert_eq!(e.buffer.join("\n"), "ab\ncd");
        assert_eq!(e2.buffer.join("\n"), "é");
    }

    #[test]
    fn apply_edit_ranges_multiline_edit_undoes_exactly() {
        let mut e = EditorState::new("one\ntwo\nthree", "f".into());
        let before = e.buffer.join("\n");
        let edit = crate::lsp::features::RangeEdit {
            start: TextPosition { line: 0, byte: 2 },
            end: TextPosition { line: 2, byte: 1 },
            new_text: "X".into(),
        };
        e.apply_edit_ranges(&[edit], 0).unwrap();
        assert_eq!(e.buffer.join("\n"), "onXhree");
        e.undo();
        assert_eq!(e.buffer.join("\n"), before);
        // Disjoint edits in one batch also revert together.
        let edits = vec![
            crate::lsp::features::RangeEdit {
                start: TextPosition { line: 0, byte: 0 },
                end: TextPosition { line: 0, byte: 3 },
                new_text: "A".into(),
            },
            crate::lsp::features::RangeEdit {
                start: TextPosition { line: 2, byte: 0 },
                end: TextPosition { line: 2, byte: 5 },
                new_text: "B".into(),
            },
        ];
        e.apply_edit_ranges(&edits, 1).unwrap();
        assert_eq!(e.buffer.join("\n"), "A\ntwo\nB");
        e.undo();
        assert_eq!(e.buffer.join("\n"), before);
    }
}
