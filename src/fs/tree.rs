use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::Result;

/// A lightweight entry in a directory snapshot.
/// Only stores the name and whether it's a directory — no expensive stat() call.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SnapshotEntry {
    pub name: OsString,
    pub is_dir: bool,
}

/// A snapshot of a directory's contents for efficient paginated access.
///
/// Collected via a single `read_dir()` pass. Sorted once. Paginated by index.
/// Full `TreeNode` metadata (stat) is loaded only for the current visible page.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DirSnapshot {
    /// All entries in sorted order.
    pub entries: Vec<SnapshotEntry>,
    /// Number of entries skipped due to permission errors during collection.
    pub skipped_count: usize,
    /// Whether the snapshot was capped at a maximum size.
    pub capped: bool,
}

#[allow(dead_code)]
impl DirSnapshot {
    /// Collect a directory snapshot in a single `read_dir()` pass.
    ///
    /// Returns a snapshot with lightweight entries (name + is_dir flag).
    /// Permission-denied and broken symlink entries are skipped and counted.
    /// The snapshot is unsorted — call `sort()` before pagination.
    pub fn collect(path: &Path) -> Result<Self> {
        Self::collect_with_limit(path, usize::MAX)
    }

    /// Collect a directory snapshot with a maximum entry limit.
    ///
    /// If the directory has more entries than `max_entries`, only the first
    /// `max_entries` are collected and `capped` is set to true.
    pub fn collect_with_limit(path: &Path, max_entries: usize) -> Result<Self> {
        let read_dir = fs::read_dir(path)?;
        let mut entries = Vec::new();
        let mut skipped_count = 0;
        let mut capped = false;

        for entry_result in read_dir {
            if entries.len() >= max_entries {
                capped = true;
                break;
            }
            let entry = match entry_result {
                Ok(e) => e,
                Err(_) => {
                    skipped_count += 1;
                    continue;
                }
            };
            // Use file_type() which is usually free (no extra stat on most OS)
            let is_dir = match entry.file_type() {
                Ok(ft) => ft.is_dir(),
                Err(_) => {
                    skipped_count += 1;
                    continue;
                }
            };
            entries.push(SnapshotEntry {
                name: entry.file_name(),
                is_dir,
            });
        }

        Ok(Self {
            entries,
            skipped_count,
            capped,
        })
    }

    /// Sort the snapshot entries.
    ///
    /// Applies the same sort logic as `TreeState::sort_children_of`:
    /// - `dirs_first`: directories before files
    /// - `sort_by`: name (case-insensitive), size, or modified
    ///
    /// For snapshot sorting, only Name sort is meaningful (we don't have
    /// size/modified metadata). Size and Modified fall back to name sort.
    pub fn sort(&mut self, sort_by: &SortBy, dirs_first: bool) {
        self.entries.sort_by(|a, b| {
            let mut cmp = std::cmp::Ordering::Equal;
            if dirs_first {
                cmp = b.is_dir.cmp(&a.is_dir);
            }
            // Snapshot only has name — all sort modes fall back to name
            cmp.then_with(|| {
                let a_name = a.name.to_string_lossy().to_lowercase();
                let b_name = b.name.to_string_lossy().to_lowercase();
                match sort_by {
                    SortBy::Name => a_name.cmp(&b_name),
                    // For Size/Modified, we don't have the metadata in snapshot,
                    // so sort by name as a stable fallback. The loaded TreeNodes
                    // will be re-sorted with full metadata by sort_children_of.
                    SortBy::Size | SortBy::Modified => a_name.cmp(&b_name),
                }
            })
        });
    }

    /// Get the total number of entries in the snapshot.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the snapshot is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get a page of entries from the snapshot.
    ///
    /// Returns a slice of entries from `offset` with at most `count` entries.
    pub fn page(&self, offset: usize, count: usize) -> &[SnapshotEntry] {
        let start = offset.min(self.entries.len());
        let end = (start + count).min(self.entries.len());
        &self.entries[start..end]
    }
}

/// Type of filesystem node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeType {
    File,
    Directory,
    Symlink,
    /// Virtual node for paginated directory loading.
    #[allow(dead_code)]
    LoadMore,
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
#[allow(dead_code)]
pub struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub node_type: NodeType,
    pub children: Option<Vec<TreeNode>>,
    pub is_expanded: bool,
    pub depth: usize,
    pub meta: FileMeta,
    /// Total immediate children count (from fast `read_dir().count()`).
    /// `None` means not yet counted.
    pub total_child_count: Option<usize>,
    /// Number of children currently loaded (for pagination tracking).
    pub loaded_child_count: usize,
    /// Whether more children remain to be loaded.
    pub has_more_children: bool,
    /// Sorted snapshot for O(1) paginated access into large directories.
    pub snapshot: Option<DirSnapshot>,
    /// Index into the snapshot of the next unloaded entry.
    pub loaded_offset: usize,
    /// Whether the directory contents may have changed since last load.
    /// Set by FS watcher for paginated dirs; cleared on re-scan.
    pub is_stale: bool,
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
            total_child_count: None,
            loaded_child_count: 0,
            has_more_children: false,
            snapshot: None,
            loaded_offset: 0,
            is_stale: false,
        })
    }

    /// Load ALL children for a directory node (no pagination).
    ///
    /// This is the original unpaginated loading. Used internally when the
    /// directory is small enough that pagination is not needed.
    /// Sorting is applied separately via `TreeState::sort_children_of`.
    /// Permission-denied and broken symlinks are silently skipped.
    fn load_children_all(&mut self) -> Result<()> {
        if self.node_type != NodeType::Directory {
            return Ok(());
        }

        let mut children = Vec::new();
        let entries = fs::read_dir(&self.path)?;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            match TreeNode::new(&entry.path(), self.depth + 1) {
                Ok(node) => children.push(node),
                Err(_) => continue,
            }
        }

        let count = children.len();
        self.children = Some(children);
        self.total_child_count = Some(count);
        self.loaded_child_count = count;
        self.has_more_children = false;
        Ok(())
    }

    /// Load children with pagination support using snapshot-based approach.
    ///
    /// 1. Collects a DirSnapshot in a single `read_dir()` pass
    /// 2. If total ≤ `page_size`, loads all (backward compatible, no snapshot overhead)
    /// 3. If total > `page_size`, sorts the snapshot and loads only the first page
    ///    of `TreeNode`s from snapshot indices. `total_child_count` comes from
    ///    `snapshot.len()` — no separate count pass needed.
    ///
    /// Sorting is applied separately via `TreeState::sort_children_of`.
    pub fn load_children_paged(&mut self, page_size: usize) -> Result<()> {
        self.load_children_paged_with_sort(page_size, &SortBy::Name, true)
    }

    /// Load children with pagination support, using provided sort settings for snapshot.
    pub fn load_children_paged_with_sort(
        &mut self,
        page_size: usize,
        sort_by: &SortBy,
        dirs_first: bool,
    ) -> Result<()> {
        if self.node_type != NodeType::Directory {
            return Ok(());
        }

        // Collect snapshot in a single read_dir() pass
        let mut snapshot = match DirSnapshot::collect(&self.path) {
            Ok(s) => s,
            Err(_) => {
                self.total_child_count = Some(0);
                self.children = Some(Vec::new());
                self.snapshot = None;
                self.loaded_offset = 0;
                return Ok(());
            }
        };

        let total = snapshot.len();
        self.total_child_count = Some(total);

        // If small enough, load all — backward compatible, no snapshot overhead
        if total <= page_size {
            self.snapshot = None;
            self.loaded_offset = 0;
            return self.load_children_all();
        }

        // Sort the snapshot for consistent pagination order
        snapshot.sort(sort_by, dirs_first);

        // Load first page of TreeNodes from snapshot
        let page_entries = snapshot.page(0, page_size);
        let children = Self::load_nodes_from_snapshot(page_entries, &self.path, self.depth + 1);
        let loaded = children.len();

        self.children = Some(children);
        self.loaded_child_count = loaded;
        self.loaded_offset = loaded;
        self.has_more_children = loaded < total;
        self.snapshot = Some(snapshot);
        self.is_stale = false;

        Ok(())
    }

    /// Create TreeNodes from snapshot entries by stat-ing each one.
    ///
    /// Entries that fail to stat (permission denied, broken symlink) are skipped.
    pub fn load_nodes_from_snapshot(
        entries: &[SnapshotEntry],
        parent_path: &Path,
        child_depth: usize,
    ) -> Vec<TreeNode> {
        let mut nodes = Vec::with_capacity(entries.len());
        for entry in entries {
            let child_path = parent_path.join(&entry.name);
            match TreeNode::new(&child_path, child_depth) {
                Ok(node) => nodes.push(node),
                Err(_) => continue,
            }
        }
        nodes
    }

    /// Load children for a directory node (backward-compatible API).
    ///
    /// Loads all entries without pagination.
    /// Sorting is applied separately via `TreeState::sort_children_of`.
    /// Permission-denied and broken symlinks are silently skipped.
    #[allow(dead_code)]
    pub fn load_children(&mut self) -> Result<()> {
        self.load_children_all()
    }

    /// Get the count of immediate children (fast, no recursion).
    ///
    /// Uses cached `total_child_count` if available, otherwise performs
    /// a fast `read_dir().count()` and caches the result.
    /// Returns `None` on permission denied or other errors.
    ///
    /// **Note**: This can block on large or network directories.
    /// Prefer `child_count_cached()` for UI rendering code.
    #[allow(dead_code)]
    pub fn get_child_count(&mut self) -> Option<usize> {
        if let Some(count) = self.total_child_count {
            return Some(count);
        }
        if self.node_type != NodeType::Directory {
            return None;
        }
        match fs::read_dir(&self.path) {
            Ok(rd) => {
                let count = rd.count();
                self.total_child_count = Some(count);
                Some(count)
            }
            Err(_) => None,
        }
    }

    /// Get the cached child count without any I/O.
    ///
    /// Returns `None` if the count hasn't been computed yet.
    /// Use `spawn_async_child_count` to populate this value asynchronously.
    #[allow(dead_code)]
    pub fn child_count_cached(&self) -> Option<usize> {
        self.total_child_count
    }

    /// Load the next page of children for a paginated directory.
    ///
    /// Uses `loaded_offset` to index directly into the sorted snapshot.
    /// O(1) access — no iteration, no HashSet dedup.
    ///
    /// Returns the number of newly loaded entries.
    pub fn load_next_page(&mut self, page_size: usize) -> Result<usize> {
        if self.node_type != NodeType::Directory || !self.has_more_children {
            return Ok(0);
        }

        // If stale, re-collect snapshot before loading next page
        if self.is_stale && self.snapshot.is_some() {
            self.load_children_paged_with_sort(page_size, &SortBy::Name, true)?;
            return Ok(self.loaded_child_count);
        }

        // If we have a snapshot, use O(1) index-based access
        if let Some(ref snapshot) = self.snapshot {
            let page_entries = snapshot.page(self.loaded_offset, page_size);
            let new_nodes =
                Self::load_nodes_from_snapshot(page_entries, &self.path, self.depth + 1);
            let newly_loaded = new_nodes.len();

            let children = self.children.get_or_insert_with(Vec::new);
            children.extend(new_nodes);

            self.loaded_offset += newly_loaded;
            self.loaded_child_count += newly_loaded;
            let total = snapshot.len();
            self.has_more_children = self.loaded_offset < total;

            return Ok(newly_loaded);
        }

        // Fallback for dirs without snapshot (backward compat / small dirs reloaded)
        let existing: std::collections::HashSet<PathBuf> = self
            .children
            .as_ref()
            .map(|c| c.iter().map(|n| n.path.clone()).collect())
            .unwrap_or_default();

        let entries = fs::read_dir(&self.path)?;
        let children = self.children.get_or_insert_with(Vec::new);
        let mut newly_loaded = 0;

        for entry in entries {
            if newly_loaded >= page_size {
                break;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if existing.contains(&path) {
                continue;
            }
            match TreeNode::new(&path, self.depth + 1) {
                Ok(node) => {
                    children.push(node);
                    newly_loaded += 1;
                }
                Err(_) => continue,
            }
        }

        self.loaded_child_count += newly_loaded;
        let total = self.total_child_count.unwrap_or(0);
        self.has_more_children = self.loaded_child_count < total;
        Ok(newly_loaded)
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
    /// For `NodeType::LoadMore`: the parent directory path to load more from.
    pub load_more_parent: Option<PathBuf>,
    /// For `NodeType::LoadMore`: approximate remaining entries.
    pub load_more_remaining: Option<usize>,
    /// For directories: total immediate child count (for count badge).
    pub child_count: Option<usize>,
}

/// Sort criteria for the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortBy {
    /// Alphabetical (case-insensitive), default.
    Name,
    /// By file size (largest first).
    Size,
    /// By modification time (newest first).
    Modified,
}

impl SortBy {
    /// Parse sort_by from config string.
    pub fn from_str(s: &str) -> Self {
        match s {
            "size" => SortBy::Size,
            "modified" => SortBy::Modified,
            _ => SortBy::Name,
        }
    }

    /// Get the display label for the current sort.
    pub fn label(&self) -> &'static str {
        match self {
            SortBy::Name => "Name",
            SortBy::Size => "Size",
            SortBy::Modified => "Modified",
        }
    }

    /// Cycle to the next sort option.
    pub fn next(&self) -> Self {
        match self {
            SortBy::Name => SortBy::Size,
            SortBy::Size => SortBy::Modified,
            SortBy::Modified => SortBy::Name,
        }
    }
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
    /// Current sort criteria.
    pub sort_by: SortBy,
    /// Whether directories are shown before files.
    pub dirs_first: bool,
    /// Max entries to load per page (pagination threshold).
    pub page_size: usize,
}

impl TreeState {
    /// Create a new TreeState from a root path, expanding the root directory.
    #[allow(dead_code)]
    pub fn new(path: &Path) -> Result<Self> {
        Self::with_page_size(path, usize::MAX)
    }

    /// Create a new TreeState with a specific page size for pagination.
    pub fn with_page_size(path: &Path, page_size: usize) -> Result<Self> {
        let mut root = TreeNode::new(path, 0)?;
        if root.node_type == NodeType::Directory {
            root.load_children_paged(page_size)?;
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
            sort_by: SortBy::Name,
            dirs_first: true,
            page_size,
        };
        state.sort_all_children();
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
            load_more_parent: None,
            load_more_remaining: None,
            child_count: node.total_child_count,
        });

        if node.is_expanded {
            if let Some(children) = &node.children {
                let visible_children: Vec<&TreeNode> = if show_hidden {
                    children.iter().collect()
                } else {
                    children.iter().filter(|c| !c.meta.is_hidden).collect()
                };

                // If paginated, the last real child is NOT the last sibling —
                // the LoadMore node will be the last sibling instead.
                let has_load_more = node.has_more_children;

                for (i, child) in visible_children.iter().enumerate() {
                    let is_last_child = i == visible_children.len() - 1 && !has_load_more;
                    Self::flatten_node(child, items, show_hidden, is_last_child, false);
                }

                // Emit the "Load more..." virtual node
                if has_load_more {
                    let remaining = node
                        .total_child_count
                        .unwrap_or(0)
                        .saturating_sub(node.loaded_child_count);
                    let label = format!("Load more... (remaining: ~{})", remaining);
                    items.push(FlatItem {
                        name: label,
                        path: node.path.clone(), // path points to the parent dir
                        node_type: NodeType::LoadMore,
                        depth: node.depth + 1,
                        is_expanded: false,
                        is_last_sibling: true,
                        is_hidden: false,
                        load_more_parent: Some(node.path.clone()),
                        load_more_remaining: Some(remaining),
                        child_count: None,
                    });
                }
            }
        }
    }

    /// Expand the currently selected directory node.
    ///
    /// If the node is already expanded but stale, re-loads its children.
    pub fn expand_selected(&mut self) {
        if self.flat_items.is_empty() {
            return;
        }
        let selected = &self.flat_items[self.selected_index];
        if selected.node_type != NodeType::Directory {
            return;
        }
        let path = selected.path.clone();
        let sort_by = self.sort_by.clone();
        let dirs_first = self.dirs_first;
        let page_size = self.page_size;
        if let Some(node) = Self::find_node_mut(&mut self.root, &path) {
            if !node.is_expanded || node.is_stale {
                let _ = node.load_children_paged_with_sort(page_size, &sort_by, dirs_first);
                Self::sort_children_of(node, &sort_by, dirs_first);
                node.is_expanded = true;
                self.flatten();
            }
        }
    }

    /// Load the next page of a paginated directory.
    ///
    /// Called when the user activates a "Load more..." virtual node.
    /// `parent_path` is the directory to load more entries from.
    /// Returns the number of newly loaded entries.
    pub fn load_next_page(&mut self, parent_path: &Path) -> usize {
        let sort_by = self.sort_by.clone();
        let dirs_first = self.dirs_first;
        let page_size = self.page_size;

        let loaded = if let Some(node) = Self::find_node_mut(&mut self.root, parent_path) {
            let count = node.load_next_page(page_size).unwrap_or(0);
            if count > 0 {
                Self::sort_children_of(node, &sort_by, dirs_first);
            }
            count
        } else {
            0
        };

        if loaded > 0 {
            self.flatten();
        }
        loaded
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
        let sort_by = self.sort_by.clone();
        let dirs_first = self.dirs_first;
        let page_size = self.page_size;
        if let Some(node) = Self::find_node_mut(&mut self.root, dir_path) {
            if node.node_type == NodeType::Directory {
                let _ = node.load_children_paged_with_sort(page_size, &sort_by, dirs_first);
                Self::sort_children_of(node, &sort_by, dirs_first);
                self.flatten();
            }
        }
    }

    /// Toggle visibility of hidden files and re-flatten.
    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.flatten();
    }

    /// Sort a node's children (non-recursive, just immediate children).
    fn sort_children_of(node: &mut TreeNode, sort_by: &SortBy, dirs_first: bool) {
        if let Some(children) = &mut node.children {
            children.sort_by(|a, b| {
                let mut cmp = std::cmp::Ordering::Equal;

                if dirs_first {
                    cmp = matches!(b.node_type, NodeType::Directory)
                        .cmp(&matches!(a.node_type, NodeType::Directory));
                }

                cmp.then_with(|| match sort_by {
                    SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                    SortBy::Size => b.meta.size.cmp(&a.meta.size),
                    SortBy::Modified => b.meta.modified.cmp(&a.meta.modified),
                })
            });
        }
    }

    /// Recursively sort all loaded children in the tree.
    ///
    /// For paginated directories with snapshots: re-sort the snapshot,
    /// drop loaded children, and re-load the first page from the new order.
    fn sort_all_children_recursive(
        node: &mut TreeNode,
        sort_by: &SortBy,
        dirs_first: bool,
        page_size: usize,
    ) {
        // If this node has a snapshot (paginated), re-sort and re-paginate
        if let Some(ref mut snapshot) = node.snapshot {
            snapshot.sort(sort_by, dirs_first);
            // Re-load first page from re-sorted snapshot
            let page_entries = snapshot.page(0, page_size);
            let children =
                TreeNode::load_nodes_from_snapshot(page_entries, &node.path, node.depth + 1);
            let loaded = children.len();
            node.children = Some(children);
            node.loaded_child_count = loaded;
            node.loaded_offset = loaded;
            node.has_more_children = loaded < snapshot.len();
        }

        // Sort currently loaded children (applies full metadata sort: size/modified/name)
        Self::sort_children_of(node, sort_by, dirs_first);

        // Recurse into children
        if let Some(children) = &mut node.children {
            for child in children.iter_mut() {
                Self::sort_all_children_recursive(child, sort_by, dirs_first, page_size);
            }
        }
    }

    /// Sort all children in the entire tree and re-flatten.
    pub fn sort_all_children(&mut self) {
        let sort_by = self.sort_by.clone();
        let dirs_first = self.dirs_first;
        let page_size = self.page_size;
        Self::sort_all_children_recursive(&mut self.root, &sort_by, dirs_first, page_size);
    }

    /// Cycle to the next sort mode and re-sort.
    pub fn cycle_sort(&mut self) {
        self.sort_by = self.sort_by.next();
        self.sort_all_children();
        self.flatten();
    }

    /// Toggle dirs_first and re-sort.
    pub fn toggle_dirs_first(&mut self) {
        self.dirs_first = !self.dirs_first;
        self.sort_all_children();
        self.flatten();
    }

    /// Public accessor to find a mutable node by path (used by navigate_to_path).
    pub fn find_node_mut_pub<'a>(
        node: &'a mut TreeNode,
        target: &Path,
    ) -> Option<&'a mut TreeNode> {
        Self::find_node_mut(node, target)
    }

    /// Public accessor to sort a node's children (used by handle_fs_change, navigate_to_path).
    pub fn sort_children_of_pub(node: &mut TreeNode, sort_by: &SortBy, dirs_first: bool) {
        Self::sort_children_of(node, sort_by, dirs_first);
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
                load_more_parent: None,
                load_more_remaining: None,
                child_count: node.total_child_count,
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
        // Skip LoadMore virtual nodes — they're not real entries
        if let Some(item) = self.flat_items.get(idx) {
            if item.node_type == NodeType::LoadMore {
                return;
            }
        }
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
    ///
    /// After loading children, sorting is applied using current sort settings.
    pub fn restore_expanded(&mut self, expanded: &HashSet<PathBuf>) {
        let sort_by = self.sort_by.clone();
        let dirs_first = self.dirs_first;
        let page_size = self.page_size;
        for path in Self::expanded_paths_in_restore_order(expanded) {
            if let Some(node) = Self::find_node_mut(&mut self.root, path) {
                if node.node_type == NodeType::Directory && !node.is_expanded {
                    let _ = node.load_children_paged_with_sort(page_size, &sort_by, dirs_first);
                    Self::sort_children_of(node, &sort_by, dirs_first);
                    node.is_expanded = true;
                }
            }
        }
    }

    /// Return expanded paths sorted so ancestors are restored before descendants.
    fn expanded_paths_in_restore_order(expanded: &HashSet<PathBuf>) -> Vec<&PathBuf> {
        let mut ordered: Vec<&PathBuf> = expanded.iter().collect();
        ordered.sort_by(|a, b| {
            a.components()
                .count()
                .cmp(&b.components().count())
                .then_with(|| a.cmp(b))
        });
        ordered
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
    fn tree_state_sorts_dirs_first_by_default() {
        let dir = setup_test_dir();
        let state = TreeState::new(dir.path()).unwrap();

        // After TreeState::new, children should be sorted: dirs first (alpha, beta) then files
        // flat_items[0] is root
        assert_eq!(state.flat_items[1].name, "alpha");
        assert_eq!(state.flat_items[2].name, "beta");
    }

    #[test]
    fn cycle_sort_changes_mode() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        assert_eq!(state.sort_by, SortBy::Name);
        state.cycle_sort();
        assert_eq!(state.sort_by, SortBy::Size);
        state.cycle_sort();
        assert_eq!(state.sort_by, SortBy::Modified);
        state.cycle_sort();
        assert_eq!(state.sort_by, SortBy::Name);
    }

    #[test]
    fn toggle_dirs_first() {
        let dir = setup_test_dir();
        let mut state = TreeState::new(dir.path()).unwrap();
        assert!(state.dirs_first);
        state.toggle_dirs_first();
        assert!(!state.dirs_first);
        state.toggle_dirs_first();
        assert!(state.dirs_first);
    }

    #[test]
    fn sort_by_size() {
        let dir = setup_test_dir();
        // Write different amounts to files to give them different sizes
        std::fs::write(dir.path().join("file_a.txt"), "small").unwrap();
        std::fs::write(
            dir.path().join("file_b.rs"),
            "this is a much larger file content",
        )
        .unwrap();

        let mut state = TreeState::new(dir.path()).unwrap();
        state.sort_by = SortBy::Size;
        state.sort_all_children();
        state.flatten();

        // With dirs_first=true, dirs come first, then files by decreasing size
        // file_b.rs is larger than file_a.txt
        let file_items: Vec<&FlatItem> = state
            .flat_items
            .iter()
            .filter(|i| i.node_type == NodeType::File)
            .collect();
        assert!(file_items.len() >= 2);
        assert_eq!(file_items[0].name, "file_b.rs");
        assert_eq!(file_items[1].name, "file_a.txt");
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

    #[test]
    fn expanded_restore_order_is_parent_first() {
        let root = PathBuf::from("/tmp/root");
        let alpha = root.join("alpha");
        let nested = alpha.join("nested");

        let mut expanded = HashSet::new();
        expanded.insert(nested.clone());
        expanded.insert(root.clone());
        expanded.insert(alpha.clone());

        let ordered = TreeState::expanded_paths_in_restore_order(&expanded);
        let ordered_paths: Vec<PathBuf> = ordered.into_iter().cloned().collect();

        assert_eq!(ordered_paths, vec![root, alpha, nested]);
    }

    #[test]
    fn restore_expanded_re_expands_nested_dir() {
        let dir = setup_test_dir();
        let nested_file = dir.path().join("alpha").join("nested").join("deep.txt");
        File::create(&nested_file).unwrap();

        let mut state = TreeState::new(dir.path()).unwrap();
        let alpha_path = dir.path().join("alpha");
        let nested_path = alpha_path.join("nested");

        let alpha_idx = state.find_index_by_path(&alpha_path).unwrap();
        state.selected_index = alpha_idx;
        state.expand_selected();

        let nested_idx = state.find_index_by_path(&nested_path).unwrap();
        state.selected_index = nested_idx;
        state.expand_selected();

        let expanded = state.collect_expanded_paths();

        // Simulate watcher refresh of root: this recreates root children as collapsed nodes.
        state.reload_dir(dir.path());
        let alpha_before = state
            .flat_items
            .iter()
            .find(|i| i.path == alpha_path)
            .expect("alpha should exist after reload");
        assert!(!alpha_before.is_expanded);

        state.restore_expanded(&expanded);
        state.flatten();

        let alpha_after = state
            .flat_items
            .iter()
            .find(|i| i.path == alpha_path)
            .expect("alpha should exist after restore");
        assert!(alpha_after.is_expanded);

        let nested_after = state
            .flat_items
            .iter()
            .find(|i| i.path == nested_path)
            .expect("nested should exist after restore");
        assert!(nested_after.is_expanded);
        assert!(state.flat_items.iter().any(|i| i.path == nested_file));
    }

    // === Pagination tests ===

    fn setup_large_dir(count: usize) -> TempDir {
        let dir = TempDir::new().unwrap();
        for i in 0..count {
            File::create(dir.path().join(format!("file_{:05}.txt", i))).unwrap();
        }
        dir
    }

    #[test]
    fn load_children_paged_small_dir_no_pagination() {
        let dir = setup_test_dir();
        let mut node = TreeNode::new(dir.path(), 0).unwrap();
        // 5 children (alpha, beta, file_a.txt, file_b.rs, .hidden)
        node.load_children_paged(1000).unwrap();
        assert_eq!(node.total_child_count, Some(5));
        assert_eq!(node.loaded_child_count, 5);
        assert!(!node.has_more_children);
        assert_eq!(node.children.as_ref().unwrap().len(), 5);
    }

    #[test]
    fn load_children_paged_large_dir_with_pagination() {
        let dir = setup_large_dir(50);
        let mut node = TreeNode::new(dir.path(), 0).unwrap();
        node.load_children_paged(10).unwrap();
        assert_eq!(node.total_child_count, Some(50));
        assert_eq!(node.loaded_child_count, 10);
        assert!(node.has_more_children);
        assert_eq!(node.children.as_ref().unwrap().len(), 10);
    }

    #[test]
    fn flatten_emits_load_more_node() {
        let dir = setup_large_dir(20);
        let mut state = TreeState::with_page_size(dir.path(), 5).unwrap();
        state.flatten();

        // Should have: root + 5 children + 1 LoadMore = 7 items
        let load_more_items: Vec<&FlatItem> = state
            .flat_items
            .iter()
            .filter(|i| i.node_type == NodeType::LoadMore)
            .collect();
        assert_eq!(load_more_items.len(), 1);
        let lm = load_more_items[0];
        assert!(lm.name.contains("Load more"));
        assert!(lm.name.contains("15")); // ~15 remaining
        assert_eq!(lm.load_more_parent, Some(dir.path().to_path_buf()));
        assert_eq!(lm.load_more_remaining, Some(15));
        assert!(lm.is_last_sibling);
    }

    #[test]
    fn no_load_more_when_all_fit() {
        let dir = setup_large_dir(5);
        let state = TreeState::with_page_size(dir.path(), 10).unwrap();

        let load_more_items: Vec<&FlatItem> = state
            .flat_items
            .iter()
            .filter(|i| i.node_type == NodeType::LoadMore)
            .collect();
        assert_eq!(load_more_items.len(), 0);
    }

    #[test]
    fn get_child_count_caches_result() {
        let dir = setup_test_dir();
        let mut node = TreeNode::new(dir.path(), 0).unwrap();
        assert!(node.total_child_count.is_none());

        let count = node.get_child_count();
        assert_eq!(count, Some(5));
        assert_eq!(node.total_child_count, Some(5)); // cached

        // Second call should return cached value
        let count2 = node.get_child_count();
        assert_eq!(count2, Some(5));
    }

    #[test]
    fn get_child_count_returns_none_for_file() {
        let dir = setup_test_dir();
        let mut node = TreeNode::new(&dir.path().join("file_a.txt"), 0).unwrap();
        assert_eq!(node.get_child_count(), None);
    }

    #[test]
    fn backward_compat_load_children() {
        // load_children() (no page_size) should load all entries
        let dir = setup_large_dir(50);
        let mut node = TreeNode::new(dir.path(), 0).unwrap();
        node.load_children().unwrap();
        assert_eq!(node.children.as_ref().unwrap().len(), 50);
        assert!(!node.has_more_children);
    }

    #[test]
    fn load_next_page_sequential() {
        let dir = setup_large_dir(25);
        let mut node = TreeNode::new(dir.path(), 0).unwrap();

        // First page: 10 entries
        node.load_children_paged(10).unwrap();
        assert_eq!(node.loaded_child_count, 10);
        assert!(node.has_more_children);

        // Second page: 10 more entries
        let loaded = node.load_next_page(10).unwrap();
        assert_eq!(loaded, 10);
        assert_eq!(node.loaded_child_count, 20);
        assert_eq!(node.children.as_ref().unwrap().len(), 20);
        assert!(node.has_more_children);

        // Third page: only 5 remaining
        let loaded = node.load_next_page(10).unwrap();
        assert_eq!(loaded, 5);
        assert_eq!(node.loaded_child_count, 25);
        assert_eq!(node.children.as_ref().unwrap().len(), 25);
        assert!(!node.has_more_children);

        // Fourth page: nothing left
        let loaded = node.load_next_page(10).unwrap();
        assert_eq!(loaded, 0);
    }

    #[test]
    fn tree_state_load_next_page_removes_load_more() {
        let dir = setup_large_dir(15);
        let mut state = TreeState::with_page_size(dir.path(), 5).unwrap();
        state.flatten();

        // Initially: root + 5 children + LoadMore = 7
        assert_eq!(
            state
                .flat_items
                .iter()
                .filter(|i| i.node_type == NodeType::LoadMore)
                .count(),
            1
        );

        // Load next page
        let loaded = state.load_next_page(dir.path());
        assert_eq!(loaded, 5);

        // Now: root + 10 children + LoadMore = 12
        assert_eq!(
            state
                .flat_items
                .iter()
                .filter(|i| i.node_type == NodeType::LoadMore)
                .count(),
            1
        );

        // Load final page
        let loaded = state.load_next_page(dir.path());
        assert_eq!(loaded, 5);

        // All loaded: root + 15 children, no LoadMore
        assert_eq!(
            state
                .flat_items
                .iter()
                .filter(|i| i.node_type == NodeType::LoadMore)
                .count(),
            0
        );
        assert!(!state.root.has_more_children);
    }

    // === DirSnapshot tests ===

    #[test]
    fn snapshot_collect_basic() {
        let dir = setup_test_dir();
        let snapshot = DirSnapshot::collect(dir.path()).unwrap();
        // 5 entries: alpha, beta, file_a.txt, file_b.rs, .hidden
        assert_eq!(snapshot.len(), 5);
        assert_eq!(snapshot.skipped_count, 0);
        assert!(!snapshot.capped);
    }

    #[test]
    fn snapshot_collect_empty_dir() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("empty")).unwrap();
        let snapshot = DirSnapshot::collect(&dir.path().join("empty")).unwrap();
        assert!(snapshot.is_empty());
        assert_eq!(snapshot.skipped_count, 0);
    }

    #[test]
    fn snapshot_collect_permission_error() {
        // Collecting from a nonexistent dir should return an error
        let result = DirSnapshot::collect(Path::new("/nonexistent/path/that/does/not/exist"));
        assert!(result.is_err());
    }

    #[test]
    fn snapshot_sort_dirs_first_by_name() {
        let dir = setup_test_dir();
        let mut snapshot = DirSnapshot::collect(dir.path()).unwrap();
        snapshot.sort(&SortBy::Name, true);

        let names: Vec<String> = snapshot
            .entries
            .iter()
            .map(|e| e.name.to_string_lossy().to_string())
            .collect();

        // Dirs first (alpha, beta), then files (.hidden, file_a.txt, file_b.rs)
        // Within dirs: alpha < beta
        assert_eq!(names[0], "alpha");
        assert_eq!(names[1], "beta");
        // Files sorted alphabetically (case-insensitive): .hidden < file_a.txt < file_b.rs
        assert_eq!(names[2], ".hidden");
        assert_eq!(names[3], "file_a.txt");
        assert_eq!(names[4], "file_b.rs");
    }

    #[test]
    fn snapshot_sort_no_dirs_first() {
        let dir = setup_test_dir();
        let mut snapshot = DirSnapshot::collect(dir.path()).unwrap();
        snapshot.sort(&SortBy::Name, false);

        let names: Vec<String> = snapshot
            .entries
            .iter()
            .map(|e| e.name.to_string_lossy().to_string())
            .collect();

        // All sorted alphabetically: .hidden, alpha, beta, file_a.txt, file_b.rs
        assert_eq!(names[0], ".hidden");
        assert_eq!(names[1], "alpha");
        assert_eq!(names[2], "beta");
        assert_eq!(names[3], "file_a.txt");
        assert_eq!(names[4], "file_b.rs");
    }

    #[test]
    fn snapshot_collect_with_limit() {
        let dir = setup_large_dir(50);
        let snapshot = DirSnapshot::collect_with_limit(dir.path(), 10).unwrap();
        assert_eq!(snapshot.len(), 10);
        assert!(snapshot.capped);
    }

    #[test]
    fn snapshot_collect_with_limit_not_capped() {
        let dir = setup_test_dir();
        let snapshot = DirSnapshot::collect_with_limit(dir.path(), 100).unwrap();
        assert_eq!(snapshot.len(), 5);
        assert!(!snapshot.capped);
    }

    #[test]
    fn snapshot_page_access() {
        let dir = setup_large_dir(20);
        let mut snapshot = DirSnapshot::collect(dir.path()).unwrap();
        snapshot.sort(&SortBy::Name, false);

        // First page of 5
        let page1 = snapshot.page(0, 5);
        assert_eq!(page1.len(), 5);

        // Second page of 5
        let page2 = snapshot.page(5, 5);
        assert_eq!(page2.len(), 5);

        // Last page (partial)
        let last_page = snapshot.page(15, 10);
        assert_eq!(last_page.len(), 5);

        // Beyond end
        let beyond = snapshot.page(20, 5);
        assert_eq!(beyond.len(), 0);
    }

    #[test]
    fn snapshot_entries_have_correct_is_dir() {
        let dir = setup_test_dir();
        let snapshot = DirSnapshot::collect(dir.path()).unwrap();

        let dirs: Vec<&SnapshotEntry> = snapshot.entries.iter().filter(|e| e.is_dir).collect();
        let files: Vec<&SnapshotEntry> = snapshot.entries.iter().filter(|e| !e.is_dir).collect();

        assert_eq!(dirs.len(), 2); // alpha, beta
        assert_eq!(files.len(), 3); // .hidden, file_a.txt, file_b.rs
    }

    #[test]
    fn snapshot_sort_is_stable() {
        let dir = setup_large_dir(20);
        let mut snapshot1 = DirSnapshot::collect(dir.path()).unwrap();
        let mut snapshot2 = snapshot1.clone();

        snapshot1.sort(&SortBy::Name, true);
        snapshot2.sort(&SortBy::Name, true);

        let names1: Vec<String> = snapshot1
            .entries
            .iter()
            .map(|e| e.name.to_string_lossy().to_string())
            .collect();
        let names2: Vec<String> = snapshot2
            .entries
            .iter()
            .map(|e| e.name.to_string_lossy().to_string())
            .collect();

        assert_eq!(names1, names2);
    }

    #[test]
    fn sort_change_re_paginates_snapshot_dir() {
        let dir = setup_large_dir(20);
        let mut state = TreeState::with_page_size(dir.path(), 5).unwrap();

        // Root has snapshot with 20 entries, first page loaded
        assert!(state.root.snapshot.is_some());
        assert_eq!(state.root.loaded_child_count, 5);
        assert_eq!(state.root.loaded_offset, 5);
        assert!(state.root.has_more_children);

        // Load second page
        state.load_next_page(dir.path());
        assert_eq!(state.root.loaded_child_count, 10);
        assert_eq!(state.root.loaded_offset, 10);

        // Now change sort mode
        state.cycle_sort(); // Name -> Size

        // After sort change, paginated dir should be re-paginated from beginning
        assert_eq!(state.root.loaded_child_count, 5);
        assert_eq!(state.root.loaded_offset, 5);
        assert!(state.root.has_more_children);
        assert!(state.root.snapshot.is_some());
    }

    #[test]
    fn toggle_dirs_first_re_paginates_snapshot_dir() {
        let dir = TempDir::new().unwrap();
        // Create mix of files and dirs (>5 entries for pagination)
        for i in 0..4 {
            fs::create_dir(dir.path().join(format!("dir_{:02}", i))).unwrap();
        }
        for i in 0..6 {
            File::create(dir.path().join(format!("file_{:02}.txt", i))).unwrap();
        }

        let mut state = TreeState::with_page_size(dir.path(), 5).unwrap();

        // First page should be dirs + some files (dirs_first=true)
        let first_page_names: Vec<String> = state
            .root
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|n| n.name.clone())
            .collect();

        // All 4 dirs should be in first page (dirs_first=true)
        let dir_count = first_page_names
            .iter()
            .filter(|n| n.starts_with("dir_"))
            .count();
        assert_eq!(dir_count, 4);

        // Toggle dirs_first off
        state.toggle_dirs_first();

        // Now re-paginated: first 5 alphabetically (no dirs-first preference)
        assert_eq!(state.root.loaded_child_count, 5);
        assert_eq!(state.root.loaded_offset, 5);
    }
}
