//! Pure adaptive workspace geometry. Application integration follows its tests.

use ratatui::layout::Rect;

pub const MIN_EXPLORER_WIDTH: u16 = 16;
pub const MAX_EXPLORER_WIDTH: u16 = 80;
pub const MIN_DOCUMENT_WIDTH: u16 = 24;
pub const MIN_DOCUMENT_HEIGHT: u16 = 5;
pub const MIN_TERMINAL_HEIGHT: u16 = 4;
pub const MAX_TERMINAL_HEIGHT: u16 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaximizedPane {
    Document,
    Terminal,
}

/// Saved sizes are absolute columns/rows, not percentages. Computing a smaller
/// layout never changes them. Maximization is an overlay over these preferences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutState {
    explorer_width: u16,
    terminal_height: u16,
    explorer_visible: bool,
    terminal_visible: bool,
    maximized: Option<MaximizedPane>,
}

impl Default for LayoutState {
    fn default() -> Self {
        Self {
            explorer_width: 24,
            terminal_height: 7,
            explorer_visible: true,
            terminal_visible: true,
            maximized: None,
        }
    }
}

impl LayoutState {
    pub fn explorer_width(&self) -> u16 {
        self.explorer_width
    }

    pub fn terminal_height(&self) -> u16 {
        self.terminal_height
    }

    pub fn explorer_visible(&self) -> bool {
        self.explorer_visible
    }

    pub fn terminal_visible(&self) -> bool {
        self.terminal_visible
    }

    pub fn maximized(&self) -> Option<MaximizedPane> {
        self.maximized
    }

    /// Rebuild an unmaximized layout from persisted absolute preferences.
    /// Widths/heights pass through the same clamps as interactive resizing, so
    /// a corrupt or out-of-range session value can never produce bad geometry.
    pub fn from_saved(
        explorer_width: u16,
        explorer_visible: bool,
        terminal_height: u16,
        terminal_visible: bool,
    ) -> Self {
        let mut state = LayoutState::default();
        state.set_explorer_width(explorer_width);
        state.set_terminal_height(terminal_height);
        state.explorer_visible = explorer_visible;
        state.terminal_visible = terminal_visible;
        state
    }

    /// Explicit saved sizing also works while hidden, but not while maximized.
    pub fn set_explorer_width(&mut self, width: u16) {
        if self.maximized.is_none() {
            self.explorer_width = width.clamp(MIN_EXPLORER_WIDTH, MAX_EXPLORER_WIDTH);
        }
    }

    pub fn set_terminal_height(&mut self, height: u16) {
        if self.maximized.is_none() {
            self.terminal_height = height.clamp(MIN_TERMINAL_HEIGHT, MAX_TERMINAL_HEIGHT);
        }
    }

    /// Pane toggles first exit maximization, then change the saved visibility.
    pub fn toggle_explorer(&mut self) {
        self.restore();
        self.explorer_visible = !self.explorer_visible;
    }

    pub fn toggle_terminal(&mut self) {
        self.restore();
        self.terminal_visible = !self.terminal_visible;
    }

    /// Maximizing a hidden terminal temporarily reveals it. Repeating the same
    /// maximize restores; switching targets retains the original preferences.
    pub fn maximize(&mut self, pane: MaximizedPane) {
        self.maximized = if self.maximized == Some(pane) {
            None
        } else {
            Some(pane)
        };
    }

    pub fn restore(&mut self) {
        self.maximized = None;
    }

    /// Positive deltas grow the pane. Resize from rendered geometry, preserving
    /// document minimums. Hidden/fallback/maximized panes and pinned boundaries
    /// are no-ops, including their saved preferences. Returns whether it changed.
    pub fn resize_explorer(&mut self, area: Rect, columns: i32) -> bool {
        let rects = compute_layout(area, self);
        if self.maximized.is_some() || rects.explorer.is_empty() {
            return false;
        }
        let maximum = MAX_EXPLORER_WIDTH.min(bounded(area).width - MIN_DOCUMENT_WIDTH - 1);
        let width = (i64::from(rects.explorer.width) + i64::from(columns))
            .clamp(i64::from(MIN_EXPLORER_WIDTH), i64::from(maximum)) as u16;
        if width == rects.explorer.width {
            return false;
        }
        self.explorer_width = width;
        true
    }

    /// The terminal size includes its header; positive deltas move the split up.
    pub fn resize_terminal(&mut self, area: Rect, rows: i32) -> bool {
        let rects = compute_layout(area, self);
        if self.maximized.is_some() || rects.terminal.is_empty() {
            return false;
        }
        let maximum = MAX_TERMINAL_HEIGHT.min(rects.center.height - MIN_DOCUMENT_HEIGHT - 1);
        let height = (i64::from(rects.terminal.height) + i64::from(rows))
            .clamp(i64::from(MIN_TERMINAL_HEIGHT), i64::from(maximum)) as u16;
        if height == rects.terminal.height {
            return false;
        }
        self.terminal_height = height;
        true
    }

    /// Absolute desired splitter column (not a content coordinate).
    pub fn drag_explorer(&mut self, area: Rect, column: u16) -> bool {
        let rects = compute_layout(area, self);
        self.resize_explorer(area, i32::from(column) - i32::from(rects.explorer_split.x))
    }

    /// Absolute desired splitter row. Clamps even if the pointer leaves `area`.
    pub fn drag_terminal(&mut self, area: Rect, row: u16) -> bool {
        let rects = compute_layout(area, self);
        self.resize_terminal(area, i32::from(rects.terminal_split.y) - i32::from(row))
    }
}

/// `center` contains breadcrumbs, tabs, document, terminal, and terminal split.
/// `terminal` contains `terminal_header` and `terminal_content`.
/// All other nonempty rectangles are disjoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkspaceRects {
    pub explorer: Rect,
    pub center: Rect,
    pub breadcrumbs: Rect,
    pub tabs: Rect,
    pub document: Rect,
    pub terminal: Rect,
    pub terminal_header: Rect,
    pub terminal_content: Rect,
    pub status: Rect,
    pub explorer_split: Rect,
    pub terminal_split: Rect,
    pub compact: bool,
}

impl WorkspaceRects {
    /// Uses widened edges rather than saturating `Rect::right/bottom`, so an
    /// overflowing rectangle cannot accidentally pass the containment check.
    /// Empty rectangles must also have an origin inside the closed area bounds.
    pub fn all_inside(&self, area: Rect) -> bool {
        let area = bounded(area);
        [
            self.explorer,
            self.center,
            self.breadcrumbs,
            self.tabs,
            self.document,
            self.terminal,
            self.terminal_header,
            self.terminal_content,
            self.status,
            self.explorer_split,
            self.terminal_split,
        ]
        .into_iter()
        .all(|rect| {
            rect.x >= area.x
                && rect.y >= area.y
                && u32::from(rect.x) + u32::from(rect.width)
                    <= u32::from(area.x) + u32::from(area.width)
                && u32::from(rect.y) + u32::from(rect.height)
                    <= u32::from(area.y) + u32::from(area.height)
        })
    }
}

/// One bottom status row is reserved first. At widths below 41 columns the
/// explorer falls away; below 11 rows the ordinary terminal falls away. Chrome
/// yields to at least one content row: tabs need two rows, breadcrumbs three.
/// `compact` flags either undersized axis, independent of pane visibility.
/// Input edges are clipped to u16::MAX, even for a directly constructed Rect.
pub fn compute_layout(area: Rect, state: &LayoutState) -> WorkspaceRects {
    let area = bounded(area);
    let empty = Rect::new(area.x, area.y, 0, 0);
    let mut rects = WorkspaceRects {
        explorer: empty,
        center: empty,
        breadcrumbs: empty,
        tabs: empty,
        document: empty,
        terminal: empty,
        terminal_header: empty,
        terminal_content: empty,
        status: empty,
        explorer_split: empty,
        terminal_split: empty,
        compact: area.width < MIN_EXPLORER_WIDTH + 1 + MIN_DOCUMENT_WIDTH
            || area.height < 1 + MIN_DOCUMENT_HEIGHT + 1 + MIN_TERMINAL_HEIGHT,
    };
    if area.is_empty() {
        return rects;
    }
    rects.status = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    let body_height = area.height - 1;
    if body_height == 0 {
        return rects;
    }
    if state.maximized == Some(MaximizedPane::Terminal) {
        rects.center = Rect::new(area.x, area.y, area.width, body_height);
        rects.terminal = rects.center;
        terminal_content(&mut rects);
        return rects;
    }
    let explorer_width = if state.maximized.is_none()
        && state.explorer_visible
        && area.width >= MIN_EXPLORER_WIDTH + 1 + MIN_DOCUMENT_WIDTH
    {
        state
            .explorer_width
            .min(area.width - MIN_DOCUMENT_WIDTH - 1)
    } else {
        0
    };
    let split_width = u16::from(explorer_width > 0);
    if explorer_width > 0 {
        rects.explorer = Rect::new(area.x, area.y, explorer_width, body_height);
        rects.explorer_split = Rect::new(area.x + explorer_width, area.y, split_width, body_height);
    }
    rects.center = Rect::new(
        area.x + explorer_width + split_width,
        area.y,
        area.width - explorer_width - split_width,
        body_height,
    );
    let terminal_height = if state.maximized.is_none()
        && state.terminal_visible
        && body_height >= MIN_DOCUMENT_HEIGHT + 1 + MIN_TERMINAL_HEIGHT
    {
        state
            .terminal_height
            .min(body_height - MIN_DOCUMENT_HEIGHT - 1)
    } else {
        0
    };
    let split_height = u16::from(terminal_height > 0);
    let document_height = body_height - terminal_height - split_height;
    if terminal_height > 0 {
        rects.terminal_split = Rect::new(
            rects.center.x,
            area.y + document_height,
            rects.center.width,
            1,
        );
        rects.terminal = Rect::new(
            rects.center.x,
            area.y + document_height + 1,
            rects.center.width,
            terminal_height,
        );
        terminal_content(&mut rects);
    }
    let breadcrumbs_height = u16::from(document_height >= 3);
    let tabs_height = u16::from(document_height >= 2);
    if breadcrumbs_height > 0 {
        rects.breadcrumbs = Rect::new(rects.center.x, area.y, rects.center.width, 1);
    }
    if tabs_height > 0 {
        rects.tabs = Rect::new(
            rects.center.x,
            area.y + breadcrumbs_height,
            rects.center.width,
            1,
        );
    }
    rects.document = Rect::new(
        rects.center.x,
        area.y + breadcrumbs_height + tabs_height,
        rects.center.width,
        document_height - breadcrumbs_height - tabs_height,
    );
    rects
}

fn bounded(area: Rect) -> Rect {
    Rect::new(area.x, area.y, area.width, area.height)
}

fn terminal_content(rects: &mut WorkspaceRects) {
    let terminal = rects.terminal;
    let header_height = u16::from(terminal.height >= 2);
    if header_height > 0 {
        rects.terminal_header = Rect::new(terminal.x, terminal.y, terminal.width, 1);
    }
    rects.terminal_content = Rect::new(
        terminal.x,
        terminal.y + header_height,
        terminal.width,
        terminal.height - header_height,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_keep_readable_document_and_auxiliary_panes() {
        for (width, height) in [(120, 40), (80, 24), (60, 20)] {
            let area = Rect::new(0, 0, width, height);
            let rects = compute_layout(area, &LayoutState::default());
            assert!(rects.all_inside(area));
            assert_eq!(rects.explorer.width, 24);
            assert_eq!(rects.terminal.height, 7);
            assert!(rects.document.width >= 24);
            assert!(rects.document.height >= 3);
            assert_eq!(rects.status.height, 1);
            assert_eq!(rects.breadcrumbs.height, 1);
            assert_eq!(rects.tabs.height, 1);
            assert_eq!(rects.explorer_split.width, 1);
            assert_eq!(rects.terminal_split.height, 1);
            assert!(!rects.compact);
        }
    }

    #[test]
    fn zero_tiny_and_offset_rectangles_stay_inside_without_overlap() {
        for (x, y, w, h) in [
            (0, 0, 1, 1),
            (0, 0, 0, 0),
            (12, 19, 0, 10),
            (12, 19, 10, 0),
            (12, 19, 60, 20),
            (12, 19, 20, 4),
            (65530, 65530, 30, 30),
            (65535, 65535, 1, 1),
        ] {
            let area = Rect::new(x, y, w, h);
            let rects = compute_layout(area, &LayoutState::default());
            assert!(rects.all_inside(area), "{area:?}: {rects:?}");
            assert_disjoint(&rects);
            if w == 0 || h == 0 {
                assert_eq!(rects.status.area(), 0);
                assert_eq!(rects.document.area(), 0);
            }
        }
        let tiny = compute_layout(Rect::new(0, 0, 1, 1), &LayoutState::default());
        assert_eq!(tiny.status, Rect::new(0, 0, 1, 1));
        assert_eq!(tiny.document.area(), 0);
        assert!(tiny.compact);
    }

    #[test]
    fn containment_rejects_overflowing_or_outside_rectangles() {
        let area = Rect::new(20, 30, 60, 20);
        let mut rects = compute_layout(area, &LayoutState::default());
        assert!(rects.all_inside(area));
        rects.tabs = Rect::new(19, 30, 1, 1);
        assert!(!rects.all_inside(area));
        rects.tabs = Rect::new(20, 49, 1, 2);
        assert!(!rects.all_inside(area));
        rects.tabs = Rect {
            x: 65530,
            y: 30,
            width: 30,
            height: 1,
        };
        assert!(!rects.all_inside(Rect::new(65530, 30, 5, 20)));
    }

    #[test]
    fn hidden_panes_release_space_and_repeated_toggles_restore_preferences() {
        let area = Rect::new(0, 0, 80, 24);
        let mut state = LayoutState::default();
        state.set_explorer_width(30);
        state.set_terminal_height(9);
        let original = state.clone();
        for _ in 0..4 {
            state.toggle_explorer();
            assert!(!state.explorer_visible());
            let hidden = compute_layout(area, &state);
            assert!(hidden.explorer.is_empty());
            assert!(hidden.explorer_split.is_empty());
            assert_eq!(hidden.document.width, 80);
            state.toggle_terminal();
            assert!(!state.terminal_visible());
            let hidden = compute_layout(area, &state);
            assert!(hidden.terminal.is_empty());
            assert!(hidden.terminal_split.is_empty());
            assert_eq!(hidden.document, Rect::new(0, 2, 80, 21));
            state.toggle_explorer();
            state.toggle_terminal();
            assert_eq!(state, original);
            let restored = compute_layout(area, &state);
            assert_eq!(restored.explorer.width, 30);
            assert_eq!(restored.terminal.height, 9);
        }
    }

    #[test]
    fn sizing_is_bounded_and_hidden_panes_remember_explicit_sizes() {
        let mut state = LayoutState::default();
        state.set_explorer_width(0);
        state.set_terminal_height(0);
        let rects = compute_layout(Rect::new(0, 0, 120, 40), &state);
        assert_eq!(rects.explorer.width, 16);
        assert_eq!(rects.terminal.height, 4);
        state.toggle_explorer();
        state.toggle_terminal();
        state.set_explorer_width(u16::MAX);
        state.set_terminal_height(u16::MAX);
        assert_eq!(state.explorer_width(), 80);
        assert_eq!(state.terminal_height(), 60);
        state.toggle_explorer();
        state.toggle_terminal();
        let rects = compute_layout(Rect::new(0, 0, 200, 100), &state);
        assert_eq!(rects.explorer.width, 80);
        assert_eq!(rects.terminal.height, 60);
        let saved = state.clone();
        let small = compute_layout(Rect::new(0, 0, 41, 11), &state);
        assert_eq!(small.explorer.width, 16);
        assert_eq!(small.document, Rect::new(17, 2, 24, 3));
        assert_eq!(small.terminal.height, 4);
        assert_eq!(state, saved);
    }

    #[test]
    fn maximize_switch_and_restore_preserve_hidden_pane_sizes() {
        let area = Rect::new(10, 20, 80, 24);
        let mut state = LayoutState::default();
        state.set_explorer_width(30);
        state.set_terminal_height(9);
        state.toggle_terminal();
        let saved = state.clone();
        state.maximize(MaximizedPane::Document);
        assert_eq!(state.maximized(), Some(MaximizedPane::Document));
        let document = compute_layout(area, &state);
        assert_eq!(document.document, Rect::new(10, 22, 80, 21));
        assert!(document.explorer.is_empty());
        assert!(document.terminal.is_empty());
        state.maximize(MaximizedPane::Terminal);
        let terminal = compute_layout(area, &state);
        assert_eq!(terminal.terminal, Rect::new(10, 20, 80, 23));
        assert_eq!(terminal.terminal_header, Rect::new(10, 20, 80, 1));
        assert_eq!(terminal.terminal_content, Rect::new(10, 21, 80, 22));
        assert!(terminal.document.is_empty());
        assert!(terminal.tabs.is_empty());
        assert!(terminal.breadcrumbs.is_empty());
        assert!(terminal.explorer_split.is_empty());
        assert!(terminal.terminal_split.is_empty());
        assert!(terminal.all_inside(area));
        assert_disjoint(&terminal);
        state.set_explorer_width(80);
        state.set_terminal_height(60);
        state.restore();
        assert_eq!(state, saved);
        state.restore();
        assert_eq!(state, saved);
    }

    #[test]
    fn maximizing_same_pane_toggles_back_and_pane_toggles_exit_maximize() {
        let mut state = LayoutState::default();
        let saved = state.clone();
        state.maximize(MaximizedPane::Terminal);
        state.maximize(MaximizedPane::Terminal);
        assert_eq!(state, saved);
        state.maximize(MaximizedPane::Document);
        state.toggle_explorer();
        assert_eq!(state.maximized(), None);
        assert!(!state.explorer_visible());
        state.maximize(MaximizedPane::Terminal);
        state.toggle_terminal();
        assert_eq!(state.maximized(), None);
        assert!(!state.terminal_visible());
    }

    #[test]
    fn keyboard_resize_clamps_to_pane_and_document_minimums() {
        let area = Rect::new(0, 0, 60, 20);
        let mut state = LayoutState::default();
        assert!(state.resize_explorer(area, 4));
        assert!(state.resize_terminal(area, 2));
        let rects = compute_layout(area, &state);
        assert_eq!(rects.explorer.width, 28);
        assert_eq!(rects.terminal.height, 9);
        assert!(state.resize_explorer(area, i32::MAX));
        assert!(state.resize_terminal(area, i32::MAX));
        let rects = compute_layout(area, &state);
        assert_eq!(rects.explorer.width, 35);
        assert_eq!(rects.document, Rect::new(36, 2, 24, 3));
        assert_eq!(rects.terminal.height, 13);
        assert!(!state.resize_explorer(area, 1));
        assert!(!state.resize_terminal(area, 1));
        assert!(state.resize_explorer(area, i32::MIN));
        assert!(state.resize_terminal(area, i32::MIN));
        let rects = compute_layout(area, &state);
        assert_eq!(rects.explorer.width, 16);
        assert_eq!(rects.terminal.height, 4);
        assert!(!state.resize_explorer(area, -1));
        assert!(!state.resize_terminal(area, -1));
        assert!(!state.resize_explorer(area, 0));
        assert!(!state.resize_terminal(area, 0));
    }

    #[test]
    fn drag_uses_absolute_split_coordinates_with_nonzero_origins() {
        let area = Rect::new(10, 20, 80, 24);
        let mut state = LayoutState::default();
        assert!(state.drag_explorer(area, 40));
        assert!(state.drag_terminal(area, 32));
        let rects = compute_layout(area, &state);
        assert_eq!(rects.explorer.width, 30);
        assert_eq!(rects.explorer_split, Rect::new(40, 20, 1, 23));
        assert_eq!(rects.terminal.height, 10);
        assert_eq!(rects.terminal_split, Rect::new(41, 32, 49, 1));
        assert!(!state.drag_explorer(area, 40));
        assert!(!state.drag_terminal(area, 32));
        assert!(state.drag_explorer(area, 0));
        assert!(state.drag_terminal(area, u16::MAX));
        assert_eq!(state.explorer_width(), 16);
        assert_eq!(state.terminal_height(), 4);
        assert!(state.drag_explorer(area, u16::MAX));
        assert!(state.drag_terminal(area, 0));
        assert_eq!(state.explorer_width(), 55);
        assert_eq!(state.terminal_height(), 17);
        assert!(compute_layout(area, &state).all_inside(area));
    }

    #[test]
    fn resizing_clamped_preferences_starts_from_actual_geometry() {
        let area = Rect::new(0, 0, 60, 20);
        let mut state = LayoutState::default();
        state.set_explorer_width(80);
        state.set_terminal_height(60);
        let saved = state.clone();
        assert!(!state.resize_explorer(area, 1));
        assert!(!state.resize_terminal(area, 1));
        assert_eq!(state, saved);
        assert!(state.resize_explorer(area, -1));
        assert!(state.resize_terminal(area, -1));
        assert_eq!(state.explorer_width(), 34);
        assert_eq!(state.terminal_height(), 12);
        let large = Rect::new(0, 0, 300, 100);
        assert!(state.resize_explorer(large, i32::MAX));
        assert!(state.resize_terminal(large, i32::MAX));
        assert_eq!(state.explorer_width(), 80);
        assert_eq!(state.terminal_height(), 60);
    }

    #[test]
    fn hidden_compact_empty_and_maximized_resize_attempts_preserve_saved_sizes() {
        let area = Rect::new(0, 0, 80, 24);
        let mut state = LayoutState::default();
        state.resize_explorer(area, 6);
        state.resize_terminal(area, 2);
        assert_eq!(state.explorer_width(), 30);
        assert_eq!(state.terminal_height(), 9);
        for pane in [MaximizedPane::Document, MaximizedPane::Terminal] {
            let saved = state.clone();
            state.maximize(pane);
            for size in [area, Rect::new(0, 0, 1, 1), Rect::new(9, 8, 200, 60)] {
                assert!(!state.resize_explorer(size, 9));
                assert!(!state.resize_terminal(size, -2));
                assert!(!state.drag_explorer(size, 0));
                assert!(!state.drag_terminal(size, 0));
                assert!(compute_layout(size, &state).all_inside(size));
            }
            state.restore();
            assert_eq!(state, saved);
        }
        state.toggle_explorer();
        state.toggle_terminal();
        let hidden = state.clone();
        assert!(!state.resize_explorer(area, 9));
        assert!(!state.resize_terminal(area, -2));
        assert!(!state.drag_explorer(area, 0));
        assert!(!state.drag_terminal(area, 0));
        assert_eq!(state, hidden);
        state.toggle_explorer();
        state.toggle_terminal();
        let saved = state.clone();
        for size in [
            Rect::new(10, 20, 0, 0),
            Rect::new(10, 20, 80, 1),
            Rect::new(10, 20, 40, 10),
        ] {
            assert!(!state.resize_explorer(size, 9));
            assert!(!state.resize_terminal(size, -2));
            assert!(!state.drag_explorer(size, 0));
            assert!(!state.drag_terminal(size, 0));
            assert_eq!(state, saved);
        }
        let constrained = Rect::new(0, 0, 41, 11);
        assert!(!state.resize_explorer(constrained, 9));
        assert!(!state.resize_terminal(constrained, -2));
        assert_eq!(state, saved);
    }

    #[test]
    fn compact_thresholds_keep_content_and_do_not_change_saved_preferences() {
        let mut state = LayoutState::default();
        state.set_explorer_width(35);
        state.set_terminal_height(13);
        let saved = state.clone();
        let regular = compute_layout(Rect::new(0, 0, 60, 20), &state);
        for (width, height, explorer_width, terminal_height, document) in [
            (40, 10, 0, 0, Rect::new(0, 2, 40, 7)),
            (41, 10, 16, 0, Rect::new(17, 2, 24, 7)),
            (40, 11, 0, 4, Rect::new(0, 2, 40, 3)),
            (41, 11, 16, 4, Rect::new(17, 2, 24, 3)),
            (1, 2, 0, 0, Rect::new(0, 0, 1, 1)),
            (1, 3, 0, 0, Rect::new(0, 1, 1, 1)),
            (1, 4, 0, 0, Rect::new(0, 2, 1, 1)),
        ] {
            let area = Rect::new(0, 0, width, height);
            let rects = compute_layout(area, &state);
            assert_eq!(rects.explorer.width, explorer_width);
            assert_eq!(rects.terminal.height, terminal_height);
            assert_eq!(rects.document, document);
            assert_eq!(rects.compact, width < 41 || height < 11);
            assert_eq!(rects.status, Rect::new(0, height - 1, width, 1));
            assert!(rects.all_inside(area));
            assert_disjoint(&rects);
            assert_eq!(state, saved);
        }
        assert_eq!(compute_layout(Rect::new(0, 0, 60, 20), &state), regular);
    }

    #[test]
    fn tiny_maximized_terminal_prioritizes_content_over_header() {
        let mut state = LayoutState::default();
        state.maximize(MaximizedPane::Terminal);
        let single_row = compute_layout(Rect::new(7, 9, 1, 2), &state);
        assert_eq!(single_row.terminal_content, Rect::new(7, 9, 1, 1));
        assert!(single_row.terminal_header.is_empty());
        assert_eq!(single_row.status, Rect::new(7, 10, 1, 1));
        let two_rows = compute_layout(Rect::new(7, 9, 1, 3), &state);
        assert_eq!(two_rows.terminal_header, Rect::new(7, 9, 1, 1));
        assert_eq!(two_rows.terminal_content, Rect::new(7, 10, 1, 1));
    }

    #[test]
    fn malformed_raw_area_edges_are_clipped_and_resize_without_overflow() {
        let area = Rect {
            x: 65435,
            y: 65495,
            width: u16::MAX,
            height: u16::MAX,
        };
        let mut state = LayoutState::default();
        let rects = compute_layout(area, &state);
        assert_eq!(rects.center, Rect::new(65460, 65495, 75, 39));
        assert_eq!(rects.status, Rect::new(65435, 65534, 100, 1));
        assert!(rects.all_inside(area));
        assert_disjoint(&rects);
        assert!(state.drag_explorer(area, u16::MAX));
        assert!(state.drag_terminal(area, 0));
        assert_eq!(state.explorer_width(), 75);
        assert_eq!(state.terminal_height(), 33);
        assert!(compute_layout(area, &state).all_inside(area));
        let mut invalid = rects;
        invalid.status = Rect {
            x: 65534,
            y: 65534,
            width: 2,
            height: 2,
        };
        assert!(!invalid.all_inside(area));
        invalid = rects;
        invalid.document = Rect::new(0, 0, 0, 0);
        assert!(!invalid.all_inside(area));
    }

    #[test]
    fn geometry_matrix_is_bounded_disjoint_and_nested_in_all_pane_modes() {
        for explorer_visible in [false, true] {
            for terminal_visible in [false, true] {
                for maximized in [
                    None,
                    Some(MaximizedPane::Document),
                    Some(MaximizedPane::Terminal),
                ] {
                    for (explorer_width, terminal_height) in [(16, 4), (24, 7), (80, 60)] {
                        let mut state = LayoutState::default();
                        state.set_explorer_width(explorer_width);
                        state.set_terminal_height(terminal_height);
                        if !explorer_visible {
                            state.toggle_explorer();
                        }
                        if !terminal_visible {
                            state.toggle_terminal();
                        }
                        if let Some(pane) = maximized {
                            state.maximize(pane);
                        }
                        let saved = state.clone();
                        for width in [0, 1, 2, 16, 24, 40, 41, 48, 60, 80, 120, 200, 65535] {
                            for height in [0, 1, 2, 3, 4, 10, 11, 20, 24, 40, 100, 65535] {
                                for (x, y) in [(0, 0), (7, 11), (65520, 65525)] {
                                    let area = Rect::new(x, y, width, height);
                                    let rects = compute_layout(area, &state);
                                    assert!(rects.all_inside(area), "{area:?}: {rects:?}");
                                    assert_disjoint(&rects);
                                    for child in [
                                        rects.breadcrumbs,
                                        rects.tabs,
                                        rects.document,
                                        rects.terminal,
                                        rects.terminal_split,
                                    ] {
                                        assert_nested(rects.center, child);
                                    }
                                    assert_nested(rects.terminal, rects.terminal_header);
                                    assert_nested(rects.terminal, rects.terminal_content);
                                    if !area.is_empty() {
                                        assert_eq!(rects.status.height, 1);
                                        assert_eq!(rects.status.width, area.width);
                                    }
                                    assert_eq!(state, saved);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn assert_nested(parent: Rect, child: Rect) {
        if !child.is_empty() {
            assert!(
                child.x >= parent.x
                    && child.y >= parent.y
                    && child.right() <= parent.right()
                    && child.bottom() <= parent.bottom(),
                "{child:?} is not inside {parent:?}"
            );
        }
    }

    fn assert_disjoint(rects: &WorkspaceRects) {
        let leaves = [
            rects.explorer,
            rects.breadcrumbs,
            rects.tabs,
            rects.document,
            rects.terminal_header,
            rects.terminal_content,
            rects.status,
            rects.explorer_split,
            rects.terminal_split,
        ];
        for (index, left) in leaves.iter().enumerate() {
            for right in &leaves[index + 1..] {
                assert!(
                    left.is_empty() || right.is_empty() || !left.intersects(*right),
                    "{left:?} overlaps {right:?}"
                );
            }
        }
    }
}
