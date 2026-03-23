//! Mailbox directory manager — manages a hierarchy of mailboxes and folders
//! rooted at a base path.
//!
//! Port of `macmbx_store.c`.  Scans the filesystem to build a tree of
//! [`Node`] entries (mailboxes and folders), then provides create / delete /
//! rename / move / find / list operations on that tree.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::toc::{MailboxType, Toc};

// ── Store-level events ──

/// Events emitted by store operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreEvent {
    MailboxCreated(PathBuf),
    MailboxDeleted(PathBuf),
    MailboxRenamed { old: PathBuf, new: PathBuf },
    MailboxMoved   { old: PathBuf, new: PathBuf },
    FolderCreated(PathBuf),
    FolderDeleted(PathBuf),
}

// ── Node types ──

/// Whether a node represents a mailbox file or a folder (directory).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    Mailbox,
    Folder,
}

/// A node in the mailbox tree.
#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub path: PathBuf,
    pub node_type: NodeType,
    /// Special mailbox type (only meaningful when `node_type == Mailbox`).
    pub mbox_type: MailboxType,
    /// Cached unread count (`None` = unknown).
    pub unread: Option<usize>,
    /// Cached total message count (`None` = unknown).
    pub total: Option<usize>,
    /// Child nodes (subfolders / mailboxes inside a folder).
    pub children: Vec<Node>,
}

// ── Detect special mailbox type from filename ──

/// Detect the special mailbox type from a path's filename.
pub fn detect_type(path: &Path) -> MailboxType {
    let name = path.file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("");
    match name.to_ascii_lowercase().as_str() {
        "in"       => MailboxType::In,
        "out"      => MailboxType::Out,
        "trash"    => MailboxType::Trash,
        "junk"     => MailboxType::Junk,
        "in.temp"  => MailboxType::InTemp,
        "out.temp" => MailboxType::OutTemp,
        _          => MailboxType::Normal,
    }
}

// ── Sorting helpers ──

/// Sort priority: special mailboxes first, then folders, then normal mailboxes.
/// Within the same priority band, alphabetical (case-insensitive).
fn sort_key(node: &Node) -> (u8, u8, String) {
    let special = if node.node_type == NodeType::Mailbox && node.mbox_type != MailboxType::Normal {
        0 // special mailboxes first
    } else if node.node_type == NodeType::Folder {
        1 // folders second
    } else {
        2 // normal mailboxes last
    };
    // Within the special band, order by type value for determinism
    let sub = if special == 0 { node.mbox_type as u8 } else { 0 };
    (special, sub, node.name.to_ascii_lowercase())
}

fn sort_children(children: &mut Vec<Node>) {
    children.sort_by(|a, b| {
        let ka = sort_key(a);
        let kb = sort_key(b);
        ka.cmp(&kb)
    });
}

// ── File-type helpers ──

fn is_toc_file(name: &str) -> bool {
    name.ends_with(".toc")
}

fn is_lock_file(name: &str) -> bool {
    name.ends_with(".lock")
}

fn is_temp_file(name: &str) -> bool {
    name.ends_with(".tmp")
}

fn should_skip(name: &str) -> bool {
    name.starts_with('.') || is_toc_file(name) || is_lock_file(name) || is_temp_file(name)
}

// ── Directory scanning ──

/// Recursively scan a directory, building a tree of [`Node`] entries.
fn scan_directory(dir_path: &Path, dir_name: &str) -> Option<Node> {
    let entries = fs::read_dir(dir_path).ok()?;

    let mut folder = Node {
        name: dir_name.to_string(),
        path: dir_path.to_path_buf(),
        node_type: NodeType::Folder,
        mbox_type: MailboxType::Normal,
        unread: None,
        total: None,
        children: Vec::new(),
    };

    for entry in entries.flatten() {
        let file_name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if should_skip(&file_name) {
            continue;
        }

        let full_path = entry.path();
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if ft.is_dir() {
            if let Some(sub) = scan_directory(&full_path, &file_name) {
                folder.children.push(sub);
            }
        } else if ft.is_file() {
            let mbox_type = detect_type(&full_path);
            folder.children.push(Node {
                name: file_name,
                path: full_path,
                node_type: NodeType::Mailbox,
                mbox_type,
                unread: None,
                total: None,
                children: Vec::new(),
            });
        }
    }

    sort_children(&mut folder.children);
    Some(folder)
}

// ── Mailbox file operations (create / remove / rename) ──

/// Create a new empty mailbox file and its .toc.
fn create_mailbox_file(path: &Path) -> io::Result<()> {
    // Create empty mbox file
    File::create(path)?;
    // Create empty TOC
    let mut toc = Toc::new(path);
    toc.save()?;
    Ok(())
}

/// Remove a mailbox file and its .toc.
fn remove_mailbox_file(path: &Path) -> io::Result<()> {
    let toc_path = path.with_extension("toc");
    // Remove TOC first (ok if missing)
    let _ = fs::remove_file(&toc_path);
    fs::remove_file(path)
}

/// Rename a mailbox file and its .toc.
fn rename_mailbox_file(old: &Path, new: &Path) -> io::Result<()> {
    fs::rename(old, new)?;
    // Rename .toc (ok if this fails — TOC can be rebuilt)
    let old_toc = old.with_extension("toc");
    let new_toc = new.with_extension("toc");
    let _ = fs::rename(&old_toc, &new_toc);
    Ok(())
}

// ── Store ──

/// The mailbox store — manages a directory tree of mailboxes and folders.
#[derive(Debug)]
pub struct Store {
    /// Root directory path.
    root_path: PathBuf,
    /// Tree of nodes (the root node is a virtual folder for the root dir).
    root: Option<Node>,
    /// Accumulated events since last drain.
    events: Vec<StoreEvent>,
    /// Store-level lock file handle (Unix only).
    #[cfg(unix)]
    lock_file: Option<File>,
}

impl Store {
    /// Open (or create) a store at the given root directory.
    /// Scans the directory tree and builds the mailbox hierarchy.
    pub fn open(root_path: impl AsRef<Path>) -> io::Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();

        // Create directory if needed
        if !root_path.is_dir() {
            fs::create_dir_all(&root_path)?;
        }

        let root = scan_directory(&root_path, "");
        Ok(Self {
            root_path,
            root,
            events: Vec::new(),
            #[cfg(unix)]
            lock_file: None,
        })
    }

    /// Get the root directory path.
    pub fn root_dir(&self) -> &Path {
        &self.root_path
    }

    /// Re-scan the directory tree, replacing the in-memory tree.
    pub fn refresh(&mut self) -> io::Result<()> {
        self.root = scan_directory(&self.root_path, "");
        if self.root.is_none() {
            return Err(io::Error::new(io::ErrorKind::Other, "failed to scan store root"));
        }
        Ok(())
    }

    /// Drain accumulated events.
    pub fn drain_events(&mut self) -> Vec<StoreEvent> {
        std::mem::take(&mut self.events)
    }

    // ── Tree navigation ──

    /// Get the top-level children of the store (mailboxes and folders at root).
    pub fn children(&self) -> &[Node] {
        match &self.root {
            Some(root) => &root.children,
            None => &[],
        }
    }

    /// Compute a relative path from the store root for a given absolute path.
    fn rel_path(&self, full: &Path) -> PathBuf {
        full.strip_prefix(&self.root_path)
            .unwrap_or(full)
            .to_path_buf()
    }

    /// Find a node by its path relative to the store root.
    pub fn find(&self, rel_path: impl AsRef<Path>) -> Option<&Node> {
        let target = rel_path.as_ref();
        Self::find_in_tree(self.children(), &self.root_path, target)
    }

    fn find_in_tree<'a>(nodes: &'a [Node], base: &Path, target: &Path) -> Option<&'a Node> {
        for node in nodes {
            let node_rel = node.path.strip_prefix(base).unwrap_or(&node.path);
            if node_rel == target {
                return Some(node);
            }
            if !node.children.is_empty() {
                if let Some(found) = Self::find_in_tree(&node.children, base, target) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Find a node by display name (case-insensitive, searches entire tree).
    pub fn find_by_name(&self, name: &str) -> Option<&Node> {
        Self::find_name_in_tree(self.children(), name)
    }

    fn find_name_in_tree<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Node> {
        for node in nodes {
            if node.name.eq_ignore_ascii_case(name) {
                return Some(node);
            }
            if let Some(found) = Self::find_name_in_tree(&node.children, name) {
                return Some(found);
            }
        }
        None
    }

    /// Find a special mailbox (In, Out, Trash, Junk) in the tree.
    pub fn find_special(&self, mbox_type: MailboxType) -> Option<&Node> {
        Self::find_special_in_tree(self.children(), mbox_type)
    }

    fn find_special_in_tree(nodes: &[Node], mbox_type: MailboxType) -> Option<&Node> {
        for node in nodes {
            if node.node_type == NodeType::Mailbox && node.mbox_type == mbox_type {
                return Some(node);
            }
            if let Some(found) = Self::find_special_in_tree(&node.children, mbox_type) {
                return Some(found);
            }
        }
        None
    }

    // ── Counting ──

    /// Count all mailbox nodes in the tree.
    pub fn count_mailboxes(&self) -> usize {
        Self::count_nodes(self.children(), NodeType::Mailbox)
    }

    /// Count all folder nodes in the tree.
    pub fn count_folders(&self) -> usize {
        Self::count_nodes(self.children(), NodeType::Folder)
    }

    fn count_nodes(nodes: &[Node], node_type: NodeType) -> usize {
        let mut n = 0;
        for node in nodes {
            if node.node_type == node_type {
                n += 1;
            }
            n += Self::count_nodes(&node.children, node_type);
        }
        n
    }

    // ── Creation ──

    /// Create a new mailbox under the given parent (relative path, or empty
    /// string / `None` for root).
    pub fn create_mailbox(&mut self, parent_path: Option<&str>, name: &str) -> io::Result<PathBuf> {
        if name.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty mailbox name"));
        }

        let parent_full = self.resolve_parent(parent_path);

        // Ensure parent directory exists
        if !parent_full.is_dir() {
            fs::create_dir_all(&parent_full)?;
        }

        let mbox_path = parent_full.join(name);
        create_mailbox_file(&mbox_path)?;

        self.events.push(StoreEvent::MailboxCreated(mbox_path.clone()));
        self.refresh()?;

        Ok(self.rel_path(&mbox_path))
    }

    /// Create a new folder (directory) under the given parent.
    pub fn create_folder(&mut self, parent_path: Option<&str>, name: &str) -> io::Result<PathBuf> {
        if name.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty folder name"));
        }

        let parent_full = self.resolve_parent(parent_path);
        let dir_path = parent_full.join(name);

        fs::create_dir_all(&dir_path)?;

        self.events.push(StoreEvent::FolderCreated(dir_path.clone()));
        self.refresh()?;

        Ok(self.rel_path(&dir_path))
    }

    fn resolve_parent(&self, parent_path: Option<&str>) -> PathBuf {
        match parent_path {
            Some(p) if !p.is_empty() => self.root_path.join(p),
            _ => self.root_path.clone(),
        }
    }

    // ── Delete ──

    /// Delete a mailbox or folder by relative path.
    /// Folders must be empty.
    pub fn delete(&mut self, rel_path: impl AsRef<Path>) -> io::Result<()> {
        let rel = rel_path.as_ref();
        let node = self.find(rel)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "node not found"))?;

        let node_type = node.node_type;
        let full_path = node.path.clone();
        let has_children = !node.children.is_empty();

        match node_type {
            NodeType::Folder => {
                if has_children {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "cannot delete non-empty folder",
                    ));
                }
                fs::remove_dir(&full_path)?;
                self.events.push(StoreEvent::FolderDeleted(full_path));
            }
            NodeType::Mailbox => {
                remove_mailbox_file(&full_path)?;
                self.events.push(StoreEvent::MailboxDeleted(full_path));
            }
        }

        self.refresh()?;
        Ok(())
    }

    // ── Rename ──

    /// Rename a mailbox or folder (keeps it in the same parent directory).
    pub fn rename(&mut self, rel_path: impl AsRef<Path>, new_name: &str) -> io::Result<()> {
        let rel = rel_path.as_ref();
        let node = self.find(rel)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "node not found"))?;

        let node_type = node.node_type;
        let old_path = node.path.clone();

        let new_path = old_path.parent()
            .unwrap_or(Path::new(""))
            .join(new_name);

        match node_type {
            NodeType::Mailbox => rename_mailbox_file(&old_path, &new_path)?,
            NodeType::Folder => fs::rename(&old_path, &new_path)?,
        }

        self.events.push(StoreEvent::MailboxRenamed {
            old: old_path,
            new: new_path,
        });
        self.refresh()?;
        Ok(())
    }

    // ── Move ──

    /// Move a mailbox or folder to a new parent folder.
    /// `new_parent` is relative to the store root (or `None` for root).
    pub fn move_node(&mut self, rel_path: impl AsRef<Path>, new_parent: Option<&str>) -> io::Result<()> {
        let rel = rel_path.as_ref();
        let node = self.find(rel)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "node not found"))?;

        let node_type = node.node_type;
        let old_path = node.path.clone();
        let node_name = node.name.clone();

        let dst_dir = self.resolve_parent(new_parent);
        if !dst_dir.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "destination folder not found"));
        }

        let new_path = dst_dir.join(&node_name);

        match node_type {
            NodeType::Mailbox => rename_mailbox_file(&old_path, &new_path)?,
            NodeType::Folder => fs::rename(&old_path, &new_path)?,
        }

        self.events.push(StoreEvent::MailboxMoved {
            old: old_path,
            new: new_path,
        });
        self.refresh()?;
        Ok(())
    }

    // ── Open mailbox through store ──

    /// Open a mailbox by relative path, returning its TOC.
    pub fn open_mailbox(&self, rel_path: impl AsRef<Path>) -> io::Result<Toc> {
        let rel = rel_path.as_ref();
        let node = self.find(rel)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "node not found"))?;
        if node.node_type != NodeType::Mailbox {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a mailbox"));
        }
        Toc::open(&node.path)
    }

    // ── Count updates ──

    /// Update cached unread/total counts for all mailbox nodes by peeking TOCs.
    pub fn update_counts(&mut self) {
        if let Some(ref mut root) = self.root {
            Self::update_counts_tree(&mut root.children);
        }
    }

    fn update_counts_tree(nodes: &mut [Node]) {
        for node in nodes.iter_mut() {
            if node.node_type == NodeType::Mailbox {
                // Try to open the TOC to get counts
                if let Ok(toc) = Toc::open(&node.path) {
                    node.total = Some(toc.msgs.len());
                    node.unread = Some(toc.unread_count());
                }
            }
            if !node.children.is_empty() {
                Self::update_counts_tree(&mut node.children);
            }
        }
    }

    // ── Batch operations ──

    /// Save all dirty TOCs for every mailbox in the store.
    ///
    /// Opens each mailbox TOC, checks if dirty, and saves it.
    /// Returns the number of TOCs saved.
    pub fn flush(&self) -> usize {
        let mut saved = 0;
        Self::flush_tree(self.children(), &mut saved);
        saved
    }

    fn flush_tree(nodes: &[Node], saved: &mut usize) {
        for node in nodes {
            if node.node_type == NodeType::Mailbox {
                if let Ok(mut toc) = Toc::open(&node.path) {
                    if toc.dirty {
                        if toc.save().is_ok() {
                            *saved += 1;
                        }
                    }
                }
            }
            if !node.children.is_empty() {
                Self::flush_tree(&node.children, saved);
            }
        }
    }

    /// Compact all mailboxes that have deleted messages.
    ///
    /// Opens each mailbox TOC, checks for deleted messages, and compacts
    /// (removes deleted messages from the mbox file and rewrites the TOC).
    /// Returns the number of mailboxes successfully compacted.
    pub fn compact_all(&self) -> usize {
        let mut compacted = 0;
        Self::compact_tree(self.children(), &mut compacted);
        compacted
    }

    fn compact_tree(nodes: &[Node], compacted: &mut usize) {
        for node in nodes {
            if node.node_type == NodeType::Mailbox {
                if let Ok(mut toc) = Toc::open(&node.path) {
                    if crate::compact::count_deleted(&toc) > 0 {
                        if crate::compact::compact(&mut toc).is_ok() {
                            *compacted += 1;
                        }
                    }
                }
            }
            if !node.children.is_empty() {
                Self::compact_tree(&node.children, compacted);
            }
        }
    }

    // ── Flat enumeration ──

    /// Collect all mailbox relative paths in the tree.
    pub fn list_mailboxes(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        Self::collect_paths(self.children(), &self.root_path, NodeType::Mailbox, &mut paths);
        paths
    }

    /// Collect all folder relative paths in the tree.
    pub fn list_folders(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        Self::collect_paths(self.children(), &self.root_path, NodeType::Folder, &mut paths);
        paths
    }

    fn collect_paths(nodes: &[Node], base: &Path, node_type: NodeType, out: &mut Vec<PathBuf>) {
        for node in nodes {
            if node.node_type == node_type {
                let rel = node.path.strip_prefix(base)
                    .unwrap_or(&node.path)
                    .to_path_buf();
                out.push(rel);
            }
            Self::collect_paths(&node.children, base, node_type, out);
        }
    }

    // ── Store locking (Unix only) ──

    /// Acquire an exclusive store-level lock.
    #[cfg(unix)]
    pub fn lock(&mut self) -> io::Result<()> {
        use std::os::unix::io::AsRawFd;
        if self.lock_file.is_some() {
            return Ok(()); // already locked
        }
        let lock_path = self.root_path.join(".store.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        let fd = file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
        self.lock_file = Some(file);
        Ok(())
    }

    /// Release the store-level lock.
    #[cfg(unix)]
    pub fn unlock(&mut self) {
        if let Some(ref file) = self.lock_file {
            use std::os::unix::io::AsRawFd;
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN); }
        }
        self.lock_file = None;
        let lock_path = self.root_path.join(".store.lock");
        let _ = fs::remove_file(&lock_path);
    }

    // Non-Unix stubs
    #[cfg(not(unix))]
    pub fn lock(&mut self) -> io::Result<()> { Ok(()) }

    #[cfg(not(unix))]
    pub fn unlock(&mut self) {} // no-op on non-Unix

    // ── Embedded files ──

    /// Get (or create) the Embedded/ directory path within the store.
    pub fn embedded_dir(&self) -> io::Result<PathBuf> {
        let path = self.root_path.join("Embedded");
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    /// Resolve a relative path (e.g. "Embedded/image.jpg") to an absolute path.
    pub fn resolve_embedded(&self, rel_path: impl AsRef<Path>) -> PathBuf {
        self.root_path.join(rel_path)
    }

    /// Embed (copy) an external file into the store's Embedded/ directory.
    ///
    /// If the file is already inside Embedded/, returns its relative path
    /// without copying.  If a file with the same name already exists, a
    /// numeric suffix is appended (e.g. `image_1.jpg`).
    ///
    /// Returns the relative path from the store root (e.g. `Embedded/image.jpg`).
    pub fn embed_file(&self, src_path: impl AsRef<Path>) -> io::Result<PathBuf> {
        let src = src_path.as_ref();
        let emb_dir = self.embedded_dir()?;

        // If already inside Embedded/, just return relative path
        if src.starts_with(&emb_dir) {
            return Ok(src.strip_prefix(&self.root_path)
                .unwrap_or(src)
                .to_path_buf());
        }

        // Extract filename
        let basename = src.file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no filename"))?
            .to_string_lossy();

        // Build destination, handling name collisions
        let mut dst = emb_dir.join(&*basename);
        if dst.exists() {
            let stem = src.file_stem()
                .unwrap_or(OsStr::new("file"))
                .to_string_lossy();
            let ext = src.extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            for i in 1..10000 {
                dst = emb_dir.join(format!("{}_{}{}", stem, i, ext));
                if !dst.exists() {
                    break;
                }
            }
        }

        // Copy
        fs::copy(src, &dst)?;

        Ok(dst.strip_prefix(&self.root_path)
            .unwrap_or(&dst)
            .to_path_buf())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        #[cfg(unix)]
        self.unlock();
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a temp dir and return its path.
    fn tmp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn open_creates_dir() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("new_store");
        assert!(!sub.exists());
        let store = Store::open(&sub).unwrap();
        assert!(sub.is_dir());
        assert_eq!(store.count_mailboxes(), 0);
        assert_eq!(store.count_folders(), 0);
    }

    #[test]
    fn detect_type_special() {
        assert_eq!(detect_type(Path::new("/mail/In")), MailboxType::In);
        assert_eq!(detect_type(Path::new("/mail/out")), MailboxType::Out);
        assert_eq!(detect_type(Path::new("/mail/TRASH")), MailboxType::Trash);
        assert_eq!(detect_type(Path::new("/mail/Junk")), MailboxType::Junk);
        assert_eq!(detect_type(Path::new("/mail/In.temp")), MailboxType::InTemp);
        assert_eq!(detect_type(Path::new("/mail/Out.temp")), MailboxType::OutTemp);
        assert_eq!(detect_type(Path::new("/mail/Work")), MailboxType::Normal);
    }

    #[test]
    fn create_and_find_mailbox() {
        let (_dir, mut store) = tmp_store();
        let rel = store.create_mailbox(None, "TestBox").unwrap();
        assert_eq!(rel, PathBuf::from("TestBox"));
        assert_eq!(store.count_mailboxes(), 1);

        let node = store.find("TestBox").unwrap();
        assert_eq!(node.name, "TestBox");
        assert_eq!(node.node_type, NodeType::Mailbox);
        assert_eq!(node.mbox_type, MailboxType::Normal);
    }

    #[test]
    fn create_special_mailboxes() {
        let (_dir, mut store) = tmp_store();
        store.create_mailbox(None, "In").unwrap();
        store.create_mailbox(None, "Out").unwrap();
        store.create_mailbox(None, "Trash").unwrap();
        store.create_mailbox(None, "Junk").unwrap();

        assert_eq!(store.find_special(MailboxType::In).unwrap().name, "In");
        assert_eq!(store.find_special(MailboxType::Out).unwrap().name, "Out");
        assert_eq!(store.find_special(MailboxType::Trash).unwrap().name, "Trash");
        assert_eq!(store.find_special(MailboxType::Junk).unwrap().name, "Junk");
    }

    #[test]
    fn create_folder_and_nested_mailbox() {
        let (_dir, mut store) = tmp_store();
        store.create_folder(None, "Work").unwrap();
        store.create_mailbox(Some("Work"), "Projects").unwrap();

        assert_eq!(store.count_folders(), 1);
        assert_eq!(store.count_mailboxes(), 1);

        let node = store.find("Work/Projects").unwrap();
        assert_eq!(node.name, "Projects");
        assert_eq!(node.node_type, NodeType::Mailbox);
    }

    #[test]
    fn find_by_name_case_insensitive() {
        let (_dir, mut store) = tmp_store();
        store.create_mailbox(None, "MyMailbox").unwrap();

        assert!(store.find_by_name("mymailbox").is_some());
        assert!(store.find_by_name("MYMAILBOX").is_some());
        assert!(store.find_by_name("MyMailbox").is_some());
        assert!(store.find_by_name("nonexistent").is_none());
    }

    #[test]
    fn delete_mailbox() {
        let (_dir, mut store) = tmp_store();
        store.create_mailbox(None, "ToDelete").unwrap();
        assert_eq!(store.count_mailboxes(), 1);

        store.delete("ToDelete").unwrap();
        assert_eq!(store.count_mailboxes(), 0);
        assert!(store.find("ToDelete").is_none());
    }

    #[test]
    fn delete_empty_folder() {
        let (_dir, mut store) = tmp_store();
        store.create_folder(None, "EmptyDir").unwrap();
        assert_eq!(store.count_folders(), 1);

        store.delete("EmptyDir").unwrap();
        assert_eq!(store.count_folders(), 0);
    }

    #[test]
    fn delete_nonempty_folder_fails() {
        let (_dir, mut store) = tmp_store();
        store.create_folder(None, "HasStuff").unwrap();
        store.create_mailbox(Some("HasStuff"), "Child").unwrap();

        let err = store.delete("HasStuff").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Other);
    }

    #[test]
    fn rename_mailbox() {
        let (_dir, mut store) = tmp_store();
        store.create_mailbox(None, "OldName").unwrap();
        store.rename("OldName", "NewName").unwrap();

        assert!(store.find("OldName").is_none());
        assert!(store.find("NewName").is_some());
        assert_eq!(store.find("NewName").unwrap().name, "NewName");
    }

    #[test]
    fn rename_folder() {
        let (_dir, mut store) = tmp_store();
        store.create_folder(None, "OldFolder").unwrap();
        store.create_mailbox(Some("OldFolder"), "Inside").unwrap();

        store.rename("OldFolder", "NewFolder").unwrap();

        assert!(store.find("OldFolder").is_none());
        assert!(store.find("NewFolder").is_some());
        assert!(store.find("NewFolder/Inside").is_some());
    }

    #[test]
    fn move_mailbox() {
        let (_dir, mut store) = tmp_store();
        store.create_mailbox(None, "Movable").unwrap();
        store.create_folder(None, "Dest").unwrap();

        store.move_node("Movable", Some("Dest")).unwrap();

        assert!(store.find("Movable").is_none());
        assert!(store.find("Dest/Movable").is_some());
    }

    #[test]
    fn move_to_root() {
        let (_dir, mut store) = tmp_store();
        store.create_folder(None, "Sub").unwrap();
        store.create_mailbox(Some("Sub"), "Nested").unwrap();

        store.move_node("Sub/Nested", None).unwrap();

        assert!(store.find("Sub/Nested").is_none());
        assert!(store.find("Nested").is_some());
    }

    #[test]
    fn list_mailboxes_and_folders() {
        let (_dir, mut store) = tmp_store();
        store.create_mailbox(None, "In").unwrap();
        store.create_mailbox(None, "Out").unwrap();
        store.create_folder(None, "Archive").unwrap();
        store.create_mailbox(Some("Archive"), "2023").unwrap();

        let mboxes = store.list_mailboxes();
        assert_eq!(mboxes.len(), 3); // In, Out, Archive/2023

        let folders = store.list_folders();
        assert_eq!(folders.len(), 1); // Archive
    }

    #[test]
    fn sort_order_special_first() {
        let (_dir, mut store) = tmp_store();
        store.create_mailbox(None, "Zebra").unwrap();
        store.create_mailbox(None, "In").unwrap();
        store.create_mailbox(None, "Alpha").unwrap();
        store.create_mailbox(None, "Out").unwrap();
        store.create_folder(None, "MyFolder").unwrap();

        let children = store.children();
        // Special mailboxes first (In, Out), then folders, then normal mailboxes
        assert_eq!(children[0].name, "In");
        assert_eq!(children[1].name, "Out");
        assert_eq!(children[2].name, "MyFolder");
        assert_eq!(children[3].name, "Alpha");
        assert_eq!(children[4].name, "Zebra");
    }

    #[test]
    fn refresh_picks_up_external_changes() {
        let (dir, mut store) = tmp_store();
        assert_eq!(store.count_mailboxes(), 0);

        // Create a file externally
        let mbox = dir.path().join("External");
        File::create(&mbox).unwrap();

        store.refresh().unwrap();
        assert_eq!(store.count_mailboxes(), 1);
        assert!(store.find("External").is_some());
    }

    #[test]
    fn skips_toc_lock_tmp_hidden() {
        let (dir, mut store) = tmp_store();

        // Create files that should be skipped
        File::create(dir.path().join("test.toc")).unwrap();
        File::create(dir.path().join("test.lock")).unwrap();
        File::create(dir.path().join("test.tmp")).unwrap();
        File::create(dir.path().join(".hidden")).unwrap();
        // Create a real mailbox
        File::create(dir.path().join("Real")).unwrap();

        store.refresh().unwrap();
        assert_eq!(store.count_mailboxes(), 1);
        assert_eq!(store.find("Real").unwrap().name, "Real");
    }

    #[test]
    fn open_mailbox_through_store() {
        let (_dir, mut store) = tmp_store();
        store.create_mailbox(None, "OpenMe").unwrap();

        let toc = store.open_mailbox("OpenMe").unwrap();
        assert_eq!(toc.msgs.len(), 0);
    }

    #[test]
    fn open_nonexistent_fails() {
        let (_dir, store) = tmp_store();
        assert!(store.open_mailbox("NoSuch").is_err());
    }

    #[test]
    fn open_folder_as_mailbox_fails() {
        let (_dir, mut store) = tmp_store();
        store.create_folder(None, "AFolder").unwrap();
        assert!(store.open_mailbox("AFolder").is_err());
    }

    #[test]
    fn embedded_dir_created() {
        let (_dir, store) = tmp_store();
        let emb = store.embedded_dir().unwrap();
        assert!(emb.is_dir());
        assert!(emb.ends_with("Embedded"));
    }

    #[test]
    fn resolve_embedded_path() {
        let (_dir, store) = tmp_store();
        let abs = store.resolve_embedded("Embedded/image.jpg");
        assert!(abs.ends_with("Embedded/image.jpg"));
        assert!(abs.starts_with(store.root_dir()));
    }

    #[test]
    fn embed_file_copies() {
        let (dir, store) = tmp_store();

        // Create a source file outside the store
        let src = dir.path().join("outside.jpg");
        fs::write(&src, b"JPEG data").unwrap();

        let rel = store.embed_file(&src).unwrap();
        assert_eq!(rel, PathBuf::from("Embedded/outside.jpg"));

        // Verify the copy exists
        let abs = store.resolve_embedded(&rel);
        assert!(abs.exists());
        assert_eq!(fs::read(&abs).unwrap(), b"JPEG data");
    }

    #[test]
    fn embed_file_already_inside() {
        let (_dir, store) = tmp_store();

        // Create a file inside Embedded/
        let emb = store.embedded_dir().unwrap();
        let existing = emb.join("already.png");
        fs::write(&existing, b"PNG").unwrap();

        // embed_file should return relative path without copying
        let rel = store.embed_file(&existing).unwrap();
        assert_eq!(rel, PathBuf::from("Embedded/already.png"));
    }

    #[test]
    fn embed_file_name_collision() {
        let (dir, store) = tmp_store();
        let emb = store.embedded_dir().unwrap();

        // Pre-create a file with the same name in Embedded/
        fs::write(emb.join("dup.txt"), b"original").unwrap();

        // Create external source
        let src = dir.path().join("dup.txt");
        fs::write(&src, b"new content").unwrap();

        let rel = store.embed_file(&src).unwrap();
        assert_eq!(rel, PathBuf::from("Embedded/dup_1.txt"));

        let abs = store.resolve_embedded(&rel);
        assert_eq!(fs::read(&abs).unwrap(), b"new content");
        // Original unchanged
        assert_eq!(fs::read(emb.join("dup.txt")).unwrap(), b"original");
    }

    #[test]
    fn update_counts() {
        let (_dir, mut store) = tmp_store();
        store.create_mailbox(None, "Counted").unwrap();

        store.update_counts();

        let node = store.find("Counted").unwrap();
        assert_eq!(node.total, Some(0));
    }

    #[test]
    fn events_emitted() {
        let (_dir, mut store) = tmp_store();
        store.create_mailbox(None, "Ev").unwrap();
        let events = store.drain_events();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StoreEvent::MailboxCreated(p) if p.ends_with("Ev")));

        // After drain, events are empty
        assert!(store.drain_events().is_empty());
    }

    #[test]
    fn delete_nonexistent_fails() {
        let (_dir, mut store) = tmp_store();
        assert!(store.delete("nothing").is_err());
    }

    #[test]
    fn rename_nonexistent_fails() {
        let (_dir, mut store) = tmp_store();
        assert!(store.rename("nothing", "new").is_err());
    }

    #[test]
    fn move_to_nonexistent_parent_fails() {
        let (_dir, mut store) = tmp_store();
        store.create_mailbox(None, "Orphan").unwrap();
        assert!(store.move_node("Orphan", Some("no_such_dir")).is_err());
    }

    #[test]
    fn create_mailbox_empty_name_fails() {
        let (_dir, mut store) = tmp_store();
        assert!(store.create_mailbox(None, "").is_err());
    }

    #[test]
    fn create_folder_empty_name_fails() {
        let (_dir, mut store) = tmp_store();
        assert!(store.create_folder(None, "").is_err());
    }

    #[test]
    fn root_dir_accessor() {
        let (dir, store) = tmp_store();
        assert_eq!(store.root_dir(), dir.path());
    }

    #[test]
    fn deeply_nested_structure() {
        let (_dir, mut store) = tmp_store();
        store.create_folder(None, "A").unwrap();
        store.create_folder(Some("A"), "B").unwrap();
        store.create_folder(Some("A/B"), "C").unwrap();
        store.create_mailbox(Some("A/B/C"), "Deep").unwrap();

        assert_eq!(store.count_folders(), 3);
        assert_eq!(store.count_mailboxes(), 1);

        let node = store.find("A/B/C/Deep").unwrap();
        assert_eq!(node.name, "Deep");
        assert_eq!(node.node_type, NodeType::Mailbox);
    }

    #[cfg(unix)]
    #[test]
    fn lock_unlock() {
        let (_dir, mut store) = tmp_store();
        store.lock().unwrap();
        // Double lock is fine (already locked)
        store.lock().unwrap();
        store.unlock();
        // Can lock again after unlock
        store.lock().unwrap();
        store.unlock();
    }
}
