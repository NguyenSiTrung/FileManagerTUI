use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Widget},
};

use crate::fs::tree::{FlatItem, NodeType, TreeState};
use crate::theme::ThemeColors;

/// Read-only Git decorations for the tree: work-tree root plus the parsed
/// snapshot issued for that root. The widget never runs Git; callers supply the
/// generation-validated snapshot from the background refresh.
#[derive(Clone, Copy)]
pub struct TreeGit<'a> {
    /// Repository work-tree root the snapshot paths are relative to.
    pub root: &'a std::path::Path,
    /// Parsed read-only status snapshot.
    pub snapshot: &'a crate::git::GitSnapshot,
}

/// One color-independent Git marker glyph.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GitMark {
    Conflicted,
    Modified,
    Staged,
    Untracked,
}

impl GitMark {
    /// ASCII glyph, rendered regardless of theme or icon mode so a state can
    /// never be hidden by a missing color.
    fn glyph(self) -> &'static str {
        match self {
            Self::Conflicted => "U",
            Self::Modified => "M",
            Self::Staged => "S",
            Self::Untracked => "?",
        }
    }

    /// Aggregation severity: conflicted outranks modified, staged, untracked.
    fn severity(self) -> u8 {
        match self {
            Self::Conflicted => 0,
            Self::Modified => 1,
            Self::Staged => 2,
            Self::Untracked => 3,
        }
    }

    fn color(self, theme: &ThemeColors) -> ratatui::style::Color {
        match self {
            Self::Conflicted => theme.git_conflicted_fg,
            Self::Modified => theme.git_modified_fg,
            Self::Staged => theme.git_staged_fg,
            Self::Untracked => theme.git_untracked_fg,
        }
    }
}

/// Classify one Git entry into a marker, or `None` for ignored/clean entries.
fn entry_mark(entry: &crate::git::GitEntry) -> Option<GitMark> {
    if entry.is_ignored() {
        return None;
    }
    if entry.is_conflicted() {
        return Some(GitMark::Conflicted);
    }
    if entry.is_unstaged() {
        return Some(GitMark::Modified);
    }
    if entry.is_staged() {
        return Some(GitMark::Staged);
    }
    if entry.is_untracked() {
        return Some(GitMark::Untracked);
    }
    None
}

/// Path of `item` relative to the repository root, `/`-separated to match Git.
fn repo_relative(root: &std::path::Path, item: &std::path::Path) -> Option<String> {
    let relative = item.strip_prefix(root).ok()?;
    let mut text = String::new();
    for component in relative.components() {
        if !text.is_empty() {
            text.push('/');
        }
        text.push_str(&component.as_os_str().to_string_lossy());
    }
    Some(text)
}

/// Pure Git marker classifier. Files match their exact entry; directories
/// aggregate the highest-severity descendant state from the same snapshot.
fn git_mark(git: TreeGit<'_>, path: &std::path::Path, node_type: &NodeType) -> Option<GitMark> {
    let relative = repo_relative(git.root, path)?;
    match node_type {
        NodeType::Directory => {
            let prefix = format!("{relative}/");
            git.snapshot
                .entries
                .iter()
                .filter(|entry| relative.is_empty() || entry.path.starts_with(&prefix))
                .filter_map(entry_mark)
                .min_by_key(|mark| mark.severity())
        }
        NodeType::File | NodeType::Symlink => git
            .snapshot
            .entries
            .iter()
            .find(|entry| entry.path == relative)
            .and_then(entry_mark),
        NodeType::LoadMore | NodeType::Loading => None,
    }
}

/// Tree widget that renders the file tree with box-drawing characters.
pub struct TreeWidget<'a> {
    tree_state: &'a TreeState,
    theme: &'a ThemeColors,
    use_icons: bool,
    s3_mode: bool,
    git: Option<TreeGit<'a>>,
    block: Option<Block<'a>>,
}

impl<'a> TreeWidget<'a> {
    pub fn new(tree_state: &'a TreeState, theme: &'a ThemeColors, use_icons: bool) -> Self {
        Self {
            tree_state,
            theme,
            use_icons,
            s3_mode: false,
            git: None,
            block: None,
        }
    }

    pub fn s3_mode(mut self, s3: bool) -> Self {
        self.s3_mode = s3;
        self
    }

    /// Attach generation-validated read-only Git decorations.
    pub fn git(mut self, root: &'a std::path::Path, snapshot: &'a crate::git::GitSnapshot) -> Self {
        self.git = Some(TreeGit { root, snapshot });
        self
    }

    /// The marker for an item: exact entry for files, aggregated descendants
    /// for directories (from the same snapshot, never a per-directory process).
    fn item_mark(&self, item: &FlatItem) -> Option<GitMark> {
        git_mark(self.git?, &item.path, &item.node_type)
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = block.into();
        self
    }

    /// Build the prefix string for tree indentation using box-drawing characters.
    ///
    /// We need to know the ancestor chain to draw continuation lines correctly.
    fn build_prefix(item: &FlatItem, items: &[FlatItem], item_index: usize) -> String {
        if item.depth == 0 {
            return String::new();
        }

        // Build prefix from left to right for each depth level
        let mut parts: Vec<&str> = Vec::new();

        // For each ancestor level (1..depth), determine if it's the last sibling at that level
        // We need to look backwards through ancestors to figure this out
        for d in 1..item.depth {
            // Find the ancestor at depth d that contains this item
            let mut ancestor_is_last = false;
            // Walk backwards from current item to find the ancestor at depth d
            for j in (0..item_index).rev() {
                if items[j].depth == d {
                    ancestor_is_last = items[j].is_last_sibling;
                    break;
                }
                if items[j].depth < d {
                    break;
                }
            }
            if ancestor_is_last {
                parts.push("   ");
            } else {
                parts.push("│  ");
            }
        }

        // The connector for this item
        if item.is_last_sibling {
            parts.push("└──");
        } else {
            parts.push("├──");
        }

        parts.join("")
    }

    /// Get the directory/file indicator.
    fn item_indicator(&self, item: &FlatItem) -> &'static str {
        if self.s3_mode {
            // S3 mode: use cloud/package icons
            return match item.node_type {
                NodeType::Directory => "\u{2601}  ",
                NodeType::File => "\u{1f4e6} ",
                NodeType::Loading => "\u{23f3} ",
                _ => "",
            };
        }
        if self.use_icons {
            match item.node_type {
                NodeType::Directory if item.is_expanded => " ",
                NodeType::Directory => " ",
                NodeType::Symlink => " ",
                NodeType::File => Self::file_icon_by_ext(&item.name),
                NodeType::LoadMore => "▼ ",
                NodeType::Loading => "⏳ ",
            }
        } else {
            match item.node_type {
                NodeType::Directory if item.is_expanded => "[D] ",
                NodeType::Directory => "[D] ",
                NodeType::Symlink => "[L] ",
                NodeType::File => "[F] ",
                NodeType::LoadMore => "[+] ",
                NodeType::Loading => "[.] ",
            }
        }
    }

    /// Get a Nerd Font icon for a file based on its extension.
    fn file_icon_by_ext(name: &str) -> &'static str {
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        match ext.as_str() {
            "rs" => " ",
            "py" => " ",
            "js" | "jsx" => " ",
            "ts" | "tsx" => " ",
            "html" | "htm" => " ",
            "css" | "scss" | "sass" => " ",
            "json" => " ",
            "toml" | "yaml" | "yml" | "ini" | "cfg" => " ",
            "md" | "markdown" | "rst" | "txt" => " ",
            "sh" | "bash" | "zsh" | "fish" => " ",
            "go" => " ",
            "java" | "jar" | "class" => " ",
            "c" | "h" => " ",
            "cpp" | "cxx" | "cc" | "hpp" => " ",
            "rb" => " ",
            "php" => " ",
            "lua" => " ",
            "r" => " ",
            "swift" => " ",
            "kt" | "kts" => " ",
            "ex" | "exs" => " ",
            "lock" => " ",
            "gitignore" | "gitmodules" | "gitattributes" => " ",
            "dockerfile" => " ",
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "ico" | "webp" => " ",
            "mp3" | "wav" | "flac" | "ogg" | "aac" => " ",
            "mp4" | "mkv" | "avi" | "mov" | "webm" => " ",
            "zip" | "tar" | "gz" | "xz" | "bz2" | "rar" | "7z" => " ",
            "pdf" => " ",
            "ipynb" => " ",
            "sql" | "db" | "sqlite" => " ",
            _ => " ",
        }
    }
}

impl<'a> Widget for TreeWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner_area = if let Some(block) = &self.block {
            let inner = block.inner(area);
            block.clone().render(area, buf);
            inner
        } else {
            area
        };

        let items = &self.tree_state.flat_items;
        let selected = self.tree_state.selected_index;
        // Multi-selection is path-keyed, so highlight by identity rather than by
        // the stale row indices captured before the last refresh/sort.
        let selected_paths = self.tree_state.selected_paths();
        let visible_height = inner_area.height as usize;
        let total_items = items.len();

        if items.is_empty() || visible_height == 0 {
            return;
        }

        // Determine if scrollbar is needed
        let needs_scrollbar = total_items > visible_height && inner_area.width > 1;

        // Content width: reserve 1 column for scrollbar if needed
        let content_width = if needs_scrollbar {
            inner_area.width.saturating_sub(1)
        } else {
            inner_area.width
        };

        // Compute scroll offset to keep selected item visible
        let scroll = self.tree_state.scroll_offset;

        let visible_items = items.iter().enumerate().skip(scroll).take(visible_height);

        for (i, (idx, item)) in visible_items.enumerate() {
            let y = inner_area.y + i as u16;
            if y >= inner_area.y + inner_area.height {
                break;
            }

            let prefix = Self::build_prefix(item, items, idx);
            let indicator = self.item_indicator(item);

            let is_selected = idx == selected;
            let is_multi_selected =
                !is_selected && selected_paths.iter().any(|path| path == &item.path);

            let style = if is_selected {
                Style::default()
                    .bg(self.theme.tree_selected_bg)
                    .fg(self.theme.tree_selected_fg)
                    .add_modifier(Modifier::BOLD)
            } else if is_multi_selected {
                Style::default()
                    .bg(self.theme.accent_fg)
                    .fg(self.theme.warning_fg)
                    .add_modifier(Modifier::BOLD)
            } else if item.is_hidden {
                Style::default().fg(self.theme.tree_hidden_fg)
            } else if self.s3_mode {
                // S3 mode: differentiated colors for directories vs files
                match item.node_type {
                    NodeType::Directory => Style::default()
                        .fg(self.theme.s3_dir_fg)
                        .add_modifier(Modifier::BOLD),
                    NodeType::Loading => Style::default()
                        .fg(self.theme.info_fg)
                        .add_modifier(Modifier::DIM),
                    _ => Style::default().fg(self.theme.s3_file_fg),
                }
            } else {
                match item.node_type {
                    NodeType::Directory => Style::default()
                        .fg(self.theme.tree_dir_fg)
                        .add_modifier(Modifier::BOLD),
                    NodeType::Symlink => Style::default().fg(self.theme.info_fg),
                    NodeType::File => Style::default().fg(self.theme.tree_file_fg),
                    NodeType::LoadMore => Style::default()
                        .fg(self.theme.info_fg)
                        .add_modifier(Modifier::ITALIC),
                    NodeType::Loading => Style::default()
                        .fg(self.theme.info_fg)
                        .add_modifier(Modifier::DIM),
                }
            };

            let marker = if is_multi_selected { "● " } else { "" };
            let line_content = format!("{}{}{}{}", prefix, marker, indicator, item.name);

            // Build multi-span line: name + color-independent Git marker +
            // optional count badge for collapsed dirs.
            let name_span = Span::styled(line_content, style);
            let mut spans = vec![name_span];
            if let Some(mark) = self.item_mark(item) {
                spans.push(Span::styled(
                    format!(" {}", mark.glyph()),
                    Style::default()
                        .fg(mark.color(self.theme))
                        .add_modifier(Modifier::BOLD),
                ));
            }
            let line = if item.node_type == NodeType::Directory && !item.is_expanded && !is_selected
            {
                if let Some(count) = item.child_count {
                    let badge = format!(" ({} items)", count);
                    let badge_style = Style::default().fg(self.theme.tree_hidden_fg);
                    spans.push(Span::styled(badge, badge_style));
                    Line::from(spans)
                } else {
                    Line::from(spans)
                }
            } else {
                Line::from(spans)
            };

            let line_area = Rect::new(inner_area.x, y, content_width, 1);
            buf.set_line(line_area.x, line_area.y, &line, line_area.width);
        }

        // Render scrollbar if needed
        if needs_scrollbar {
            let scrollbar_x = inner_area.x + inner_area.width - 1;
            let max_scroll = total_items.saturating_sub(visible_height);

            // Thumb size: proportional to visible/total, minimum 1 row
            let thumb_size = (visible_height * visible_height / total_items).max(1);

            // Thumb position
            let thumb_pos = if max_scroll > 0 {
                scroll * (visible_height.saturating_sub(thumb_size)) / max_scroll
            } else {
                0
            };

            let track_style = Style::default().fg(self.theme.scrollbar_track_fg);
            let thumb_style = Style::default().fg(self.theme.scrollbar_thumb_fg);

            for row in 0..visible_height {
                let y = inner_area.y + row as u16;
                if row >= thumb_pos && row < thumb_pos + thumb_size {
                    buf.set_string(scrollbar_x, y, "█", thumb_style);
                } else {
                    buf.set_string(scrollbar_x, y, "░", track_style);
                }
            }
        }
    }
}

impl<'a> TreeWidget<'a> {
    /// Compute the scrollbar column x-coordinate, if scrollbar would be rendered.
    /// Returns `Some(x)` if scrollbar is needed, `None` otherwise.
    pub fn scrollbar_x(&self, area: Rect) -> Option<u16> {
        let inner_area = if let Some(block) = &self.block {
            block.inner(area)
        } else {
            area
        };
        let total = self.tree_state.flat_items.len();
        let visible_height = inner_area.height as usize;
        if total > visible_height && inner_area.width > 1 {
            Some(inner_area.x + inner_area.width - 1)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{BranchState, GitEntry, GitEntryKind, GitSnapshot};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn entry(path: &str, kind: GitEntryKind, status: [u8; 2]) -> GitEntry {
        GitEntry {
            path: path.to_string(),
            original_path: None,
            kind,
            status,
        }
    }

    fn snapshot(entries: Vec<GitEntry>) -> GitSnapshot {
        GitSnapshot {
            branch: BranchState::Symbolic {
                name: "main".to_string(),
                oid: "abc".to_string(),
            },
            entries,
        }
    }

    fn render_text(root: &std::path::Path, snapshot: &GitSnapshot, theme: &ThemeColors) -> String {
        let tree = TreeState::new(root).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    TreeWidget::new(&tree, theme, false).git(root, snapshot),
                    frame.area(),
                )
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn file_markers_are_distinct_and_color_independent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        for name in ["tracked.txt", "staged.txt", "untracked.txt", "conflict.txt"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::write(dir.path().join("sub/inner.txt"), b"x").unwrap();
        let snapshot = snapshot(vec![
            entry("tracked.txt", GitEntryKind::Ordinary, [b'.', b'M']),
            entry("staged.txt", GitEntryKind::Ordinary, [b'A', b'.']),
            entry("untracked.txt", GitEntryKind::Untracked, [b'?', b'?']),
            entry("conflict.txt", GitEntryKind::Unmerged, [b'U', b'U']),
            entry("sub/inner.txt", GitEntryKind::Ordinary, [b'.', b'M']),
        ]);

        // Icons OFF and a theme whose marker colors are all Reset: the glyphs
        // must still be present (a missing color never hides a state).
        let mut theme = crate::theme::dark_theme();
        theme.git_modified_fg = ratatui::style::Color::Reset;
        theme.git_staged_fg = ratatui::style::Color::Reset;
        theme.git_untracked_fg = ratatui::style::Color::Reset;
        theme.git_conflicted_fg = ratatui::style::Color::Reset;
        let text = render_text(dir.path(), &snapshot, &theme);

        for (name, glyph) in [
            ("tracked.txt", "M"),
            ("staged.txt", "S"),
            ("untracked.txt", "?"),
            ("conflict.txt", "U"),
        ] {
            let row = text
                .lines()
                .find(|line| line.contains(name))
                .unwrap_or_else(|| panic!("missing row {name} in {text:?}"));
            assert!(
                row.contains(glyph),
                "row {name:?} missing {glyph:?}: {row:?}"
            );
        }
        // The directory aggregates its modified descendant from the same snapshot.
        let dir_row = text
            .lines()
            .find(|line| line.contains("sub"))
            .expect("sub directory row");
        assert!(
            dir_row.contains('M'),
            "directory must aggregate: {dir_row:?}"
        );
    }

    #[test]
    fn directory_aggregation_prefers_more_severe_descendants() {
        let git = TreeGit {
            root: std::path::Path::new("/repo"),
            snapshot: &snapshot(vec![
                entry("pkg/modified.rs", GitEntryKind::Ordinary, [b'.', b'M']),
                entry("pkg/conflict.rs", GitEntryKind::Unmerged, [b'U', b'U']),
            ]),
        };
        assert_eq!(
            git_mark(git, std::path::Path::new("/repo/pkg"), &NodeType::Directory)
                .unwrap()
                .glyph(),
            "U"
        );
        // No per-directory process: a directory with no descendants has no mark.
        assert!(git_mark(
            git,
            std::path::Path::new("/repo/empty"),
            &NodeType::Directory
        )
        .is_none());
    }

    #[test]
    fn untracked_directory_entry_and_non_repository_paths() {
        let git = TreeGit {
            root: std::path::Path::new("/repo"),
            snapshot: &snapshot(vec![entry(
                "newdir/",
                GitEntryKind::Untracked,
                [b'?', b'?'],
            )]),
        };
        assert_eq!(
            git_mark(
                git,
                std::path::Path::new("/repo/newdir"),
                &NodeType::Directory
            )
            .unwrap()
            .glyph(),
            "?"
        );
        // A path outside the repository root (e.g. an S3/virtual or unrelated
        // root) never receives an indicator.
        assert!(git_mark(git, std::path::Path::new("/elsewhere/x"), &NodeType::File).is_none());
        // Ignored entries are not decorated.
        let ignored = TreeGit {
            root: std::path::Path::new("/repo"),
            snapshot: &snapshot(vec![entry(
                "ignored.log",
                GitEntryKind::Ignored,
                [b'!', b'!'],
            )]),
        };
        assert!(git_mark(
            ignored,
            std::path::Path::new("/repo/ignored.log"),
            &NodeType::File
        )
        .is_none());
    }
}
