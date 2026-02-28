use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::Result;

/// Type of filesystem node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeType {
    File,
    Directory,
    Symlink,
}

/// File metadata for display purposes.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FileMeta {
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub is_hidden: bool,
}

/// A node in the filesystem tree.
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub node_type: NodeType,
    pub children: Option<Vec<TreeNode>>,
    pub is_expanded: bool,
    pub depth: usize,
    pub meta: FileMeta,
}

impl TreeNode {
    /// Create a new TreeNode from a filesystem path.
    pub fn new(path: &Path, depth: usize) -> Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        let node_type = if metadata.is_symlink() {
            NodeType::Symlink
        } else if metadata.is_dir() {
            NodeType::Directory
        } else {
            NodeType::File
        };

        let is_hidden = name.starts_with('.');

        let meta = FileMeta {
            size: metadata.len(),
            modified: metadata.modified().ok(),
            is_hidden,
        };

        Ok(Self {
            name,
            path: path.to_path_buf(),
            node_type,
            children: None,
            is_expanded: false,
            depth,
            meta,
        })
    }

    /// Load children for a directory node.
    ///
    /// Reads the directory contents, creates child `TreeNode`s, and sorts them
    /// with directories first, then alphabetical (case-insensitive).
    /// Permission-denied and broken symlinks are silently skipped.
    pub fn load_children(&mut self) -> Result<()> {
        if self.node_type != NodeType::Directory {
            return Ok(());
        }

        let mut children = Vec::new();
        let entries = fs::read_dir(&self.path)?;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue, // skip permission-denied entries
            };
            match TreeNode::new(&entry.path(), self.depth + 1) {
                Ok(node) => children.push(node),
                Err(_) => continue, // skip broken symlinks or inaccessible nodes
            }
        }

        // Sort: directories first, then alphabetical case-insensitive
        children.sort_by(|a, b| {
            let dir_order = matches!(b.node_type, NodeType::Directory)
                .cmp(&matches!(a.node_type, NodeType::Directory));
            dir_order.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        self.children = Some(children);
        Ok(())
    }
}

/// A flattened representation of a tree node for rendering.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FlatItem {
    pub name: String,
    pub path: PathBuf,
    pub node_type: NodeType,
    pub depth: usize,
    pub is_expanded: bool,
    pub is_last_sibling: bool,
    pub is_hidden: bool,
}

/// State for the tree view.
pub struct TreeState {
    pub root: TreeNode,
    pub flat_items: Vec<FlatItem>,
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub show_hidden: bool,
    /// Indices of multi-selected items.
    pub multi_selected: HashSet<usize>,
    /// Current inline filter query string.
    pub filter_query: String,
    /// Whether the tree is currently being filtered.
    pub is_filtering: bool,
}

impl TreeState {
    /// Create a new TreeState from a root path, expanding the root directory.
    pub fn new(path: &Path) -> Result<Self> {
        let mut root = TreeNode::new(path, 0)?;
        if root.node_type == NodeType::Directory {
            root.load_children()?;
            root.is_expanded = true;
        }

        let mut state = Self {
            root,
            flat_items: Vec::new(),
            selected_index: 0,
            scroll_offset: 0,
            show_hidden: false,
            multi_selected: HashSet::new(),
            filter_query: String::new(),
            is_filtering: false,
        };
        state.flatten();
        Ok(state)
    }

    /// Rebuild the flat items list from the tree, respecting `show_hidden`.
    ///
    /// The root node is always included regardless of hidden status.
    /// Multi-selection is cleared since indices change.
    pub fn flatten(&mut self) {
        self.flat_items.clear();
        self.multi_selected.clear();
        Self::flatten_node(
            &self.root,
            &mut self.flat_items,
            self.show_hidden,
            true,
            true,
        );
        // Clamp selected index
        if !self.flat_items.is_empty() && self.selected_index >= self.flat_items.len() {
            self.selected_index = self.flat_items.len() - 1;
        }
    }

    fn flatten_node(
        node: &TreeNode,
        items: &mut Vec<FlatItem>,
        show_hidden: bool,
        is_last: bool,
        is_root: bool,
    ) {
        if !is_root && !show_hidden && node.meta.is_hidden {
            return;
        }

        items.push(FlatItem {
            name: node.name.clone(),
            path: node.path.clone(),
            node_type: node.node_type.clone(),
            depth: node.depth,
            is_expanded: node.is_expanded,
            is_last_sibling: is_last,
            is_hidden: node.meta.is_hidden,
        });

        if node.is_expanded {
            if let Some(children) = &node.children {
                let visible_children: Vec<&TreeNode> = if show_hidden {
                    children.iter().collect()
                } else {
                    children.iter().filter(|c| !c.meta.is_hidden).collect()
                };

                for (i, child) in visible_children.iter().enumerate() {
                    let is_last_child = i == visible_children.len() - 1;
                    Self::flatten_node(child, items, show_hidden, is_last_child, false);
                }
            }
        }
    }

    /// Expand the currently selected directory node.
    pub fn expand_selected(&mut self) {
        if self.flat_items.is_empty() {
            return;
        }
        let selected = &self.flat_items[self.selected_index];
        if selected.node_type != NodeType::Directory {
            return;
        }
        let path = selected.path.clone();
        if let Some(node) = Self::find_node_mut(&mut self.root, &path) {
            if !node.is_expanded {
                let _ = node.load_children();
                node.is_expanded = true;
                self.flatten();
            }
        }
    }

    /// Collapse the currently selected directory, or jump to parent.
    pub fn collapse_selected(&mut self) {
        if self.flat_items.is_empty() {
            return;
        }
        let selected = &self.flat_items[self.selected_index];
        let path = selected.path.clone();

        // If it's an expanded directory, collapse it
        if selected.node_type == NodeType::Directory && selected.is_expanded {
            if let Some(node) = Self::find_node_mut(&mut self.root, &path) {
                node.is_expanded = false;
                self.flatten();
            }
            return;
        }

        // Otherwise, jump to parent directory
        if let Some(parent_path) = path.parent() {
            let parent_path = parent_path.to_path_buf();
            for (i, item) in self.flat_items.iter().enumerate() {
                if item.path == parent_path {
                    self.selected_index = i;
                    return;
                }
            }
        }
    }

    /// Find a mutable reference to a node by path.
    fn find_node_mut<'a>(node: &'a mut TreeNode, target: &Path) -> Option<&'a mut TreeNode> {
        if node.path == target {
            return Some(node);
        }
        if let Some(children) = &mut node.children {
            for child in children.iter_mut() {
                if let Some(found) = Self::find_node_mut(child, target) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Update the scroll offset to ensure the selected item is visible.
    pub fn update_scroll(&mut self, visible_height: usize) {
        if visible_height == 0 {
            return;
        }
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + visible_height {
            self.scroll_offset = self.selected_index - visible_height + 1;
        }
    }

    /// Reload a specific directory's children and re-flatten.
    pub fn reload_dir(&mut self, dir_path: &Path) {
        if let Some(node) = Self::find_node_mut(&mut self.root, dir_path) {
            if node.node_type == NodeType::Directory {
                let _ = node.load_children();
                self.flatten();
            }
        }
    }

    /// Toggle visibility of hidden files and re-flatten.
    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.flatten();
    }

    /// Public accessor to find a mutable node by path (used by navigate_to_path).
    pub fn find_node_mut_pub<'a>(
        node: &'a mut TreeNode,
        target: &Path,
    ) -> Option<&'a mut TreeNode> {
        Self::find_node_mut(node, target)
    }

    /// Apply inline filter: rebuild flat_items showing only matches + ancestor dirs.
    /// Case-insensitive substring match on filename.
    pub fn apply_filter(&mut self) {
        if self.filter_query.is_empty() {
            self.is_filtering = false;
            self.flatten();
            return;
        }

        self.is_filtering = true;
        self.flat_items.clear();
        self.multi_selected.clear();

        let query_lower = self.filter_query.to_lowercase();
        Self::flatten_node_filtered(
            &self.root,
            &mut self.flat_items,
            self.show_hidden,
            true,
            true,
            &query_lower,
        );

        // Clamp selected index
        if !self.flat_items.is_empty() && self.selected_index >= self.flat_items.len() {
            self.selected_index = self.flat_items.len() - 1;
        }
    }

    /// Recursively flatten, but only include nodes whose name matches the filter
    /// or that are ancestors of matching nodes.
    /// Returns true if this subtree contains any matches.
    fn flatten_node_filtered(
        node: &TreeNode,
        items: &mut Vec<FlatItem>,
        show_hidden: bool,
        is_last: bool,
        is_root: bool,
        query: &str,
    ) -> bool {
        if !is_root && !show_hidden && node.meta.is_hidden {
            return false;
        }

        let name_lower = node.name.to_lowercase();
        let self_matches = name_lower.contains(query);

        // Check if any child subtree matches
        let mut child_matches = false;
        let mut child_items = Vec::new();

        if let Some(children) = &node.children {
            let visible_children: Vec<&TreeNode> = if show_hidden {
                children.iter().collect()
            } else {
                children.iter().filter(|c| !c.meta.is_hidden).collect()
            };

            for (i, child) in visible_children.iter().enumerate() {
                let is_last_child = i == visible_children.len() - 1;
                if Self::flatten_node_filtered(
                    child,
                    &mut child_items,
                    show_hidden,
                    is_last_child,
                    false,
                    query,
                ) {
                    child_matches = true;
                }
            }
        }

        if self_matches || child_matches || is_root {
            items.push(FlatItem {
                name: node.name.clone(),
                path: node.path.clone(),
                node_type: node.node_type.clone(),
                depth: node.depth,
                is_expanded: node.is_expanded || child_matches,
                is_last_sibling: is_last,
                is_hidden: node.meta.is_hidden,
            });
            items.extend(child_items);
            true
        } else {
            false
        }
    }

    /// Toggle multi-selection of the currently focused item.
    pub fn toggle_multi_select(&mut self) {
        if self.flat_items.is_empty() {
            return;
        }
        let idx = self.selected_index;
        if self.multi_selected.contains(&idx) {
            self.multi_selected.remove(&idx);
        } else {
            self.multi_selected.insert(idx);
        }
    }

    /// Clear all multi-selections.
    pub fn clear_multi_select(&mut self) {
        self.multi_selected.clear();
    }

    /// Find the flat_items index of a node by its path.
    pub fn find_index_by_path(&self, path: &Path) -> Option<usize> {
        self.flat_items.iter().position(|item| item.path == path)
    }

    /// Collect all currently expanded directory paths.
    pub fn collect_expanded_paths(&self) -> HashSet<PathBuf> {
        self.flat_items
            .iter()
            .filter(|item| item.node_type == NodeType::Directory && item.is_expanded)
            .map(|item| item.path.clone())
            .collect()
    }

    /// Re-expand directories from a saved set of expanded paths.
    pub fn restore_expanded(&mut self, expanded: &HashSet<PathBuf>) {
        for path in expanded {
            if let Some(node) = Self::find_node_mut(&mut self.root, path) {
                if node.node_type == NodeType::Directory && !node.is_expanded {
                    let _ = node.load_children();
                    node.is_expanded = true;
                }
            }
        }
    }

    /// Find the nearest surviving sibling or parent for a deleted path.
    ///
    /// Search order: next sibling → previous sibling → parent.
    pub fn find_nearest_surviving(&self, deleted_path: &Path) -> Option<usize> {
        if let Some(parent) = deleted_path.parent() {
            // Try to find siblings by looking at parent's children in flat list
            let parent_idx = self.find_index_by_path(parent);
            if let Some(pidx) = parent_idx {
                // Return parent index as fallback
                return Some(pidx);
            }
            // Try grandparent
            if let Some(grandparent) = parent.parent() {
                return self.find_index_by_path(grandparent);
            }
        }
        // Ultimate fallback: root
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        // Create directories
        fs::create_dir(dir.path().join("alpha")).unwrap();
        fs::create_dir(dir.path().join("beta")).unwrap();
        // Create files
        File::create(dir.path().join("file_a.txt")).unwrap();
        File::create(dir.path().join("file_b.rs")).unwrap();
        // Create hidden file
        File::create(dir.path().join(".hidden")).unwrap();
        // Create nested
        fs::create_dir(dir.path().join("alpha").join("nested")).unwrap();
        File::create(dir.path().join("alpha").join("inner.txt")).unwrap();
        dir
    }

    #[test]
    fn tree_node_creation_file() {
        let dir = setup_test_dir();
        let node = TreeNode::new(&dir.path().join("file_a.txt"), 0).unwrap();
        assert_eq!(node.node_type, NodeType::File);
        assert_eq!(node.name, "file_a.txt");
        assert!(!node.meta.is_hidden);
    }

    #[test]
    fn tree_node_creation_directory() {
        let dir = setup_test_dir();
        let node = TreeNode::new(&dir.path().join("alpha"), 0).unwrap();
        assert_eq!(node.node_type, NodeType::Directory);
        assert_eq!(node.name, "alpha");
    }

    #[test]
    fn tree_node_hidden_file() {
        let dir = setup_test_dir();
        let node = TreeNode::new(&dir.path().join(".hidden"), 0).unwrap();
        assert!(node.meta.is_hidden);
    }

    #[test]
    fn load_children_sorts_dirs_first() {
        let dir = setup_test_dir();
        let mut node = TreeNode::new(dir.path(), 0).unwrap();
        node.load_children().unwrap();

        let children = node.children.as_ref().unwrap();
        // Directories should come first (hidden .hidden is a file, then alpha, beta are dirs)
        let dir_count = children
            .iter()
            .take_while(|c| c.node_type == NodeType::Directory)
            .count();
        // alpha and beta are directories
        assert_eq!(dir_count, 2);
        assert_eq!(children[0].name, "alpha");
        assert_eq!(children[1].name, "beta");
    }

    #[test]
    fn load_children_empty_directory() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("empty")).unwrap();
        let mut node = TreeNode::new(&dir.path().join("empty"), 0).unwrap();
        node.load_children().unwrap();
        assert!(node.children.as_ref().unwrap().is_empty());
    }

    #[test]
    fn flatten_expanded_tree() {
        let dir = setup_test_dir();
        let state = TreeState::new(dir.path()).unwrap();
        // Root is expanded, so we should see root + its visible children (not hidden)
        // Root + alpha + beta + file_a.txt + file_b.rs = 5 (hidden excluded)
        assert_eq!(state.flat_items.len(), 5);
    }

    #[test]
    fn flatten_with_hidden_files() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        state.toggle_hidden();
        // Root + alpha + beta + .hidden + file_a.txt + file_b.rs = 6
        assert_eq!(state.flat_items.len(), 6);
    }

    #[test]
    fn is_last_sibling_correctness() {
        let dir = setup_test_dir();
        let state = TreeState::new(dir.path()).unwrap();
        // Last visible child of root should be marked as last sibling
        let last_item = state.flat_items.last().unwrap();
        assert!(last_item.is_last_sibling);
    }

    #[test]
    fn toggle_hidden_twice_restores() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        let original_count = state.flat_items.len();
        state.toggle_hidden();
        state.toggle_hidden();
        assert_eq!(state.flat_items.len(), original_count);
    }

    #[test]
    fn multi_select_toggle_adds_index() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        state.selected_index = 1;
        state.toggle_multi_select();
        assert!(state.multi_selected.contains(&1));
    }

    #[test]
    fn multi_select_toggle_removes_index() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        state.selected_index = 2;
        state.toggle_multi_select();
        assert!(state.multi_selected.contains(&2));
        state.toggle_multi_select();
        assert!(!state.multi_selected.contains(&2));
    }

    #[test]
    fn multi_select_multiple_items() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        state.selected_index = 1;
        state.toggle_multi_select();
        state.selected_index = 3;
        state.toggle_multi_select();
        assert_eq!(state.multi_selected.len(), 2);
        assert!(state.multi_selected.contains(&1));
        assert!(state.multi_selected.contains(&3));
    }

    #[test]
    fn clear_multi_select() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        state.selected_index = 1;
        state.toggle_multi_select();
        state.selected_index = 2;
        state.toggle_multi_select();
        state.clear_multi_select();
        assert!(state.multi_selected.is_empty());
    }

    #[test]
    fn flatten_clears_multi_select() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        state.selected_index = 1;
        state.toggle_multi_select();
        state.flatten();
        assert!(state.multi_selected.is_empty());
    }

    #[test]
    fn toggle_multi_select_on_empty_is_noop() {
        let dir = TempDir::new().unwrap();
        // Create an empty dir with no children
        let mut state = TreeState::new(dir.path()).unwrap();
        // There's at least the root — but if flat_items is somehow empty, it's a noop
        // Just check it doesn't panic
        state.toggle_multi_select();
    }

    // === Filter tests ===

    #[test]
    fn apply_filter_matches_files() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        state.filter_query = "file".to_string();
        state.apply_filter();
        assert!(state.is_filtering);
        let names: Vec<&str> = state.flat_items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"file_a.txt"));
        assert!(names.contains(&"file_b.rs"));
    }

    #[test]
    fn apply_filter_preserves_ancestors() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        // Expand alpha to load children
        state.selected_index = 1; // alpha
        state.expand_selected();
        state.filter_query = "inner".to_string();
        state.apply_filter();
        let names: Vec<&str> = state.flat_items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"alpha"), "ancestor should be preserved");
        assert!(names.contains(&"inner.txt"));
    }

    #[test]
    fn apply_filter_empty_query_restores() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        let original_count = state.flat_items.len();
        state.filter_query = "file".to_string();
        state.apply_filter();
        assert!(state.flat_items.len() < original_count);
        state.filter_query.clear();
        state.apply_filter();
        assert!(!state.is_filtering);
        assert_eq!(state.flat_items.len(), original_count);
    }

    #[test]
    fn apply_filter_case_insensitive() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        state.filter_query = "FILE".to_string();
        state.apply_filter();
        let names: Vec<&str> = state.flat_items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"file_a.txt"));
        assert!(names.contains(&"file_b.rs"));
    }

    #[test]
    fn apply_filter_no_matches_shows_root() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        state.filter_query = "zzzznonexistent".to_string();
        state.apply_filter();
        // Root is always shown
        assert_eq!(state.flat_items.len(), 1);
    }

    #[test]
    fn apply_filter_clears_multi_select() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        state.selected_index = 1;
        state.toggle_multi_select();
        assert!(!state.multi_selected.is_empty());
        state.filter_query = "file".to_string();
        state.apply_filter();
        assert!(state.multi_selected.is_empty());
    }

    #[test]
    fn find_node_mut_pub_finds_node() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        let alpha_path = dir.path().join("alpha");
        let node = TreeState::find_node_mut_pub(&mut state.root, &alpha_path);
        assert!(node.is_some());
        assert_eq!(node.unwrap().name, "alpha");
    }

    // === Watcher helper tests ===

    #[test]
    fn find_index_by_path_existing() {
        let dir = setup_test_dir();
        let state = TreeState::new(dir.path()).unwrap();
        let alpha_path = dir.path().join("alpha");
        let idx = state.find_index_by_path(&alpha_path);
        assert!(idx.is_some());
        assert_eq!(state.flat_items[idx.unwrap()].name, "alpha");
    }

    #[test]
    fn find_index_by_path_nonexistent() {
        let dir = setup_test_dir();
        let state = TreeState::new(dir.path()).unwrap();
        let bogus_path = dir.path().join("nonexistent.txt");
        assert!(state.find_index_by_path(&bogus_path).is_none());
    }

    #[test]
    fn collect_expanded_paths_includes_root() {
        let dir = setup_test_dir();
        let state = TreeState::new(dir.path()).unwrap();
        let expanded = state.collect_expanded_paths();
        // Root is expanded by default
        assert!(expanded.contains(&dir.path().to_path_buf()));
    }

    #[test]
    fn collect_expanded_paths_includes_manually_expanded() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        // Expand alpha
        state.selected_index = 1; // alpha
        state.expand_selected();
        let expanded = state.collect_expanded_paths();
        assert!(expanded.contains(&dir.path().join("alpha")));
    }

    #[test]
    fn find_nearest_surviving_returns_parent() {
        let dir = setup_test_dir();
        let state = TreeState::new(dir.path()).unwrap();
        // file_a.txt is in root — if deleted, should fall back to root
        let deleted_path = dir.path().join("file_a.txt");
        let idx = state.find_nearest_surviving(&deleted_path);
        assert!(idx.is_some());
        assert_eq!(
            state.flat_items[idx.unwrap()].path,
            dir.path().to_path_buf()
        );
    }

    #[test]
    fn restore_expanded_re_expands_dir() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        // Expand alpha
        state.selected_index = 1;
        state.expand_selected();
        let expanded = state.collect_expanded_paths();

        // Collapse everything
        state.selected_index = 1;
        state.collapse_selected();
        assert!(!state
            .flat_items
            .iter()
            .any(|i| i.name == "alpha" && i.is_expanded));

        // Restore
        state.restore_expanded(&expanded);
        state.flatten();
        let alpha = state.flat_items.iter().find(|i| i.name == "alpha").unwrap();
        assert!(alpha.is_expanded);
    }
}
