//! Signatures and Stationery management.
//!
//! Signatures: plain text files in a `Signatures/` directory.
//! "Standard" and "Alternate" are the default two (always at indices 0 and 1).
//!
//! Stationery: RFC 822 message templates in a `Stationery/` directory.
//! Each file contains a complete message (headers + body) used as a template.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// ── Signature ──

/// A single signature entry (name, file path, cached content).
#[derive(Debug, Clone)]
pub struct Signature {
    pub name: String,
    pub path: PathBuf,
    content: Option<String>,
    dirty: bool,
}

impl Signature {
    fn new(name: &str, path: PathBuf) -> Self {
        Self { name: name.to_string(), path, content: None, dirty: false }
    }
}

/// Collection of signature files rooted in a directory.
#[derive(Debug)]
pub struct Signatures {
    dir_path: PathBuf,
    sigs: Vec<Signature>,
}

impl Signatures {
    /// Open (or create) a signatures directory. Creates "Standard" and "Alternate"
    /// files if they don't exist. Scans directory for additional signatures.
    pub fn open(sig_dir: &Path) -> io::Result<Self> {
        if !sig_dir.is_dir() {
            fs::create_dir_all(sig_dir)?;
        }

        let std_path = sig_dir.join("Standard");
        let alt_path = sig_dir.join("Alternate");

        // Ensure Standard and Alternate exist
        if !std_path.exists() {
            fs::write(&std_path, "")?;
        }
        if !alt_path.exists() {
            fs::write(&alt_path, "")?;
        }

        let mut sigs = Vec::new();

        // Standard first (index 0), Alternate second (index 1)
        sigs.push(Signature::new("Standard", std_path));
        sigs.push(Signature::new("Alternate", alt_path));

        // Scan for remaining signature files
        let mut extras: Vec<Signature> = Vec::new();
        for entry in fs::read_dir(sig_dir)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            if name.eq_ignore_ascii_case("Standard") || name.eq_ignore_ascii_case("Alternate") {
                continue;
            }
            if name.ends_with(".tmp") {
                continue;
            }
            let ft = entry.file_type()?;
            if !ft.is_file() {
                continue;
            }
            let full = entry.path();
            extras.push(Signature::new(&name, full));
        }
        extras.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        sigs.extend(extras);

        Ok(Self { dir_path: sig_dir.to_path_buf(), sigs })
    }

    /// Save all dirty signatures to disk (atomic write via temp file).
    pub fn save(&mut self) -> io::Result<()> {
        let mut last_err = None;
        for sig in &mut self.sigs {
            if sig.dirty {
                if let Some(ref content) = sig.content {
                    match atomic_write(&sig.path, content.as_bytes()) {
                        Ok(()) => sig.dirty = false,
                        Err(e) => last_err = Some(e),
                    }
                }
            }
        }
        match last_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Number of signatures.
    pub fn count(&self) -> usize {
        self.sigs.len()
    }

    /// Add a new signature. Returns the index of the new entry.
    pub fn add(&mut self, name: &str, content: &str) -> usize {
        let path = self.dir_path.join(name);
        let mut sig = Signature::new(name, path);
        sig.content = Some(content.to_string());
        sig.dirty = true;
        let idx = self.sigs.len();
        self.sigs.push(sig);
        idx
    }

    /// Remove a signature by index. Also deletes the file from disk.
    pub fn remove(&mut self, index: usize) -> io::Result<()> {
        if index >= self.sigs.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "index out of range"));
        }
        let _ = fs::remove_file(&self.sigs[index].path);
        self.sigs.remove(index);
        Ok(())
    }

    /// Rename a signature (both in-memory and on disk).
    pub fn rename(&mut self, index: usize, new_name: &str) -> io::Result<()> {
        if index >= self.sigs.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "index out of range"));
        }
        let new_path = self.dir_path.join(new_name);
        fs::rename(&self.sigs[index].path, &new_path)?;
        self.sigs[index].name = new_name.to_string();
        self.sigs[index].path = new_path;
        Ok(())
    }

    /// Get the content of a signature, loading from disk if needed.
    pub fn get(&mut self, index: usize) -> Option<&str> {
        if index >= self.sigs.len() {
            return None;
        }
        if self.sigs[index].content.is_none() {
            if let Ok(data) = fs::read_to_string(&self.sigs[index].path) {
                self.sigs[index].content = Some(data);
            }
        }
        self.sigs[index].content.as_deref()
    }

    /// Set the content of a signature (marks it dirty for save).
    pub fn set(&mut self, index: usize, content: &str) -> Result<(), ()> {
        if index >= self.sigs.len() {
            return Err(());
        }
        self.sigs[index].content = Some(content.to_string());
        self.sigs[index].dirty = true;
        Ok(())
    }

    /// Find a signature by name (case-insensitive). Returns index or None.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.sigs.iter().position(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Get the Standard signature content (index 0).
    pub fn standard(&mut self) -> Option<&str> {
        self.get(0)
    }

    /// Get the Alternate signature content (index 1).
    pub fn alternate(&mut self) -> Option<&str> {
        self.get(1)
    }

    /// Get a signature entry by index (read-only metadata).
    pub fn entry(&self, index: usize) -> Option<&Signature> {
        self.sigs.get(index)
    }
}

// ── Stationery ──

/// A single stationery template (RFC 822 message).
#[derive(Debug, Clone)]
pub struct Stationery {
    pub name: String,
    pub path: PathBuf,
    content: Option<Vec<u8>>,
    dirty: bool,
}

impl Stationery {
    fn new(name: &str, path: PathBuf) -> Self {
        Self { name: name.to_string(), path, content: None, dirty: false }
    }
}

/// Collection of stationery templates rooted in a directory.
#[derive(Debug)]
pub struct StationerySet {
    dir_path: PathBuf,
    items: Vec<Stationery>,
}

impl StationerySet {
    /// Open (or create) a stationery directory. Scans for existing templates.
    pub fn open(stat_dir: &Path) -> io::Result<Self> {
        if !stat_dir.is_dir() {
            fs::create_dir_all(stat_dir)?;
        }

        let mut items = Vec::new();
        for entry in fs::read_dir(stat_dir)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            if name.ends_with(".tmp") {
                continue;
            }
            let ft = entry.file_type()?;
            if !ft.is_file() {
                continue;
            }
            items.push(Stationery::new(&name, entry.path()));
        }
        items.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        Ok(Self { dir_path: stat_dir.to_path_buf(), items })
    }

    /// Save all dirty stationery templates to disk.
    pub fn save(&mut self) -> io::Result<()> {
        let mut last_err = None;
        for item in &mut self.items {
            if item.dirty {
                if let Some(ref content) = item.content {
                    match atomic_write(&item.path, content) {
                        Ok(()) => item.dirty = false,
                        Err(e) => last_err = Some(e),
                    }
                }
            }
        }
        match last_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Number of stationery templates.
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// Add a new stationery template. Returns the index.
    pub fn add(&mut self, name: &str, message: &[u8]) -> usize {
        let path = self.dir_path.join(name);
        let mut item = Stationery::new(name, path);
        item.content = Some(message.to_vec());
        item.dirty = true;
        let idx = self.items.len();
        self.items.push(item);
        idx
    }

    /// Remove a stationery template by index.
    pub fn remove(&mut self, index: usize) -> io::Result<()> {
        if index >= self.items.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "index out of range"));
        }
        let _ = fs::remove_file(&self.items[index].path);
        self.items.remove(index);
        Ok(())
    }

    /// Rename a stationery template (on disk and in memory).
    pub fn rename(&mut self, index: usize, new_name: &str) -> io::Result<()> {
        if index >= self.items.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "index out of range"));
        }
        let new_path = self.dir_path.join(new_name);
        fs::rename(&self.items[index].path, &new_path)?;
        self.items[index].name = new_name.to_string();
        self.items[index].path = new_path;
        Ok(())
    }

    /// Get the content of a stationery template, loading from disk if needed.
    pub fn get(&mut self, index: usize) -> Option<&[u8]> {
        if index >= self.items.len() {
            return None;
        }
        if self.items[index].content.is_none() {
            if let Ok(data) = fs::read(&self.items[index].path) {
                self.items[index].content = Some(data);
            }
        }
        self.items[index].content.as_deref()
    }

    /// Set the content of a stationery template (marks it dirty).
    pub fn set(&mut self, index: usize, message: &[u8]) -> Result<(), ()> {
        if index >= self.items.len() {
            return Err(());
        }
        self.items[index].content = Some(message.to_vec());
        self.items[index].dirty = true;
        Ok(())
    }

    /// Find a stationery template by name (case-insensitive).
    pub fn find(&self, name: &str) -> Option<usize> {
        self.items.iter().position(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Create a new message from a stationery template (returns a copy of the content).
    pub fn new_message(&mut self, index: usize) -> Option<Vec<u8>> {
        self.get(index).map(|data| data.to_vec())
    }

    /// Save a message as stationery. Updates existing or adds new.
    pub fn save_from_message(&mut self, name: &str, message: &[u8]) -> Result<(), ()> {
        if let Some(idx) = self.find(name) {
            self.set(idx, message)
        } else {
            self.add(name, message);
            Ok(())
        }
    }

    /// Get a stationery entry by index (read-only metadata).
    pub fn entry(&self, index: usize) -> Option<&Stationery> {
        self.items.get(index)
    }
}

// ── Helpers ──

/// Write data to a file atomically (write to .tmp, then rename).
fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -- Signature tests --

    #[test]
    fn test_sig_open_creates_default_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("sigs");
        let sigs = Signatures::open(&dir).unwrap();

        assert!(dir.join("Standard").exists());
        assert!(dir.join("Alternate").exists());
        assert_eq!(sigs.count(), 2);
        assert_eq!(sigs.entry(0).unwrap().name, "Standard");
        assert_eq!(sigs.entry(1).unwrap().name, "Alternate");
    }

    #[test]
    fn test_sig_open_discovers_existing_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("sigs");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Standard"), "std sig").unwrap();
        fs::write(dir.join("Alternate"), "alt sig").unwrap();
        fs::write(dir.join("Work"), "work sig").unwrap();
        fs::write(dir.join("Personal"), "personal sig").unwrap();
        // .tmp files should be ignored
        fs::write(dir.join("draft.tmp"), "tmp data").unwrap();

        let sigs = Signatures::open(&dir).unwrap();
        assert_eq!(sigs.count(), 4);
        assert_eq!(sigs.entry(0).unwrap().name, "Standard");
        assert_eq!(sigs.entry(1).unwrap().name, "Alternate");
        // Additional sigs sorted alphabetically
        let names: Vec<&str> = (2..sigs.count()).map(|i| sigs.entry(i).unwrap().name.as_str()).collect();
        assert_eq!(names, vec!["Personal", "Work"]);
    }

    #[test]
    fn test_sig_add_and_save() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("sigs");
        let mut sigs = Signatures::open(&dir).unwrap();

        let idx = sigs.add("Business", "Best regards,\nJohn");
        assert_eq!(idx, 2);
        assert_eq!(sigs.count(), 3);

        sigs.save().unwrap();
        let content = fs::read_to_string(dir.join("Business")).unwrap();
        assert_eq!(content, "Best regards,\nJohn");
    }

    #[test]
    fn test_sig_get_and_set() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("sigs");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Standard"), "Hello!").unwrap();
        fs::write(dir.join("Alternate"), "Bye!").unwrap();

        let mut sigs = Signatures::open(&dir).unwrap();

        // Lazy load from disk
        assert_eq!(sigs.get(0), Some("Hello!"));
        assert_eq!(sigs.standard(), Some("Hello!"));
        assert_eq!(sigs.alternate(), Some("Bye!"));

        // Modify
        sigs.set(0, "Updated").unwrap();
        assert_eq!(sigs.get(0), Some("Updated"));

        // Save and verify
        sigs.save().unwrap();
        assert_eq!(fs::read_to_string(dir.join("Standard")).unwrap(), "Updated");
    }

    #[test]
    fn test_sig_remove() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("sigs");
        let mut sigs = Signatures::open(&dir).unwrap();
        sigs.add("Extra", "content");
        sigs.save().unwrap();
        assert!(dir.join("Extra").exists());

        sigs.remove(2).unwrap();
        assert_eq!(sigs.count(), 2);
        assert!(!dir.join("Extra").exists());
    }

    #[test]
    fn test_sig_rename() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("sigs");
        let mut sigs = Signatures::open(&dir).unwrap();
        sigs.add("OldName", "content");
        sigs.save().unwrap();

        sigs.rename(2, "NewName").unwrap();
        assert_eq!(sigs.entry(2).unwrap().name, "NewName");
        assert!(!dir.join("OldName").exists());
        assert!(dir.join("NewName").exists());
    }

    #[test]
    fn test_sig_find() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("sigs");
        let mut sigs = Signatures::open(&dir).unwrap();
        sigs.add("Business", "sig");

        assert_eq!(sigs.find("standard"), Some(0));
        assert_eq!(sigs.find("ALTERNATE"), Some(1));
        assert_eq!(sigs.find("business"), Some(2));
        assert_eq!(sigs.find("nonexistent"), None);
    }

    #[test]
    fn test_sig_set_out_of_range() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("sigs");
        let mut sigs = Signatures::open(&dir).unwrap();
        assert!(sigs.set(99, "x").is_err());
        assert!(sigs.get(99).is_none());
    }

    // -- Stationery tests --

    #[test]
    fn test_stat_open_creates_dir() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("stat");
        let ss = StationerySet::open(&dir).unwrap();
        assert!(dir.is_dir());
        assert_eq!(ss.count(), 0);
    }

    #[test]
    fn test_stat_open_discovers_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("stat");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Formal"), b"From: me\r\nTo: \r\n\r\nDear Sir,").unwrap();
        fs::write(dir.join("Casual"), b"Hey!").unwrap();
        fs::write(dir.join("draft.tmp"), b"ignored").unwrap();

        let ss = StationerySet::open(&dir).unwrap();
        assert_eq!(ss.count(), 2);
    }

    #[test]
    fn test_stat_add_save_get() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("stat");
        let mut ss = StationerySet::open(&dir).unwrap();

        let msg = b"From: user@example.com\r\nSubject: Template\r\n\r\nBody text";
        let idx = ss.add("Template1", msg);
        assert_eq!(idx, 0);

        ss.save().unwrap();
        assert_eq!(fs::read(dir.join("Template1")).unwrap(), msg);

        // Reload and verify lazy loading
        let mut ss2 = StationerySet::open(&dir).unwrap();
        assert_eq!(ss2.get(0), Some(msg.as_slice()));
    }

    #[test]
    fn test_stat_set_and_save() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("stat");
        let mut ss = StationerySet::open(&dir).unwrap();
        ss.add("T1", b"original");
        ss.save().unwrap();

        ss.set(0, b"modified").unwrap();
        ss.save().unwrap();
        assert_eq!(fs::read(dir.join("T1")).unwrap(), b"modified");
    }

    #[test]
    fn test_stat_remove() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("stat");
        let mut ss = StationerySet::open(&dir).unwrap();
        ss.add("T1", b"data");
        ss.save().unwrap();

        ss.remove(0).unwrap();
        assert_eq!(ss.count(), 0);
        assert!(!dir.join("T1").exists());
    }

    #[test]
    fn test_stat_rename() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("stat");
        let mut ss = StationerySet::open(&dir).unwrap();
        ss.add("Old", b"data");
        ss.save().unwrap();

        ss.rename(0, "New").unwrap();
        assert_eq!(ss.entry(0).unwrap().name, "New");
        assert!(!dir.join("Old").exists());
        assert!(dir.join("New").exists());
    }

    #[test]
    fn test_stat_find() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("stat");
        let mut ss = StationerySet::open(&dir).unwrap();
        ss.add("Formal", b"data");
        ss.add("Casual", b"data");

        assert_eq!(ss.find("FORMAL"), Some(0));
        assert_eq!(ss.find("casual"), Some(1));
        assert_eq!(ss.find("nope"), None);
    }

    #[test]
    fn test_stat_new_message() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("stat");
        let mut ss = StationerySet::open(&dir).unwrap();
        let msg = b"Subject: Hi\r\n\r\nHello!";
        ss.add("Greeting", msg);
        ss.save().unwrap();

        let copy = ss.new_message(0).unwrap();
        assert_eq!(copy, msg);
        // It should be a separate copy
        assert_ne!(copy.as_ptr(), ss.get(0).unwrap().as_ptr());
    }

    #[test]
    fn test_stat_save_from_message_new() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("stat");
        let mut ss = StationerySet::open(&dir).unwrap();

        ss.save_from_message("New", b"data1").unwrap();
        assert_eq!(ss.count(), 1);
        assert_eq!(ss.get(0), Some(b"data1".as_slice()));
    }

    #[test]
    fn test_stat_save_from_message_update() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("stat");
        let mut ss = StationerySet::open(&dir).unwrap();
        ss.add("T", b"old");

        ss.save_from_message("T", b"new").unwrap();
        assert_eq!(ss.count(), 1);
        assert_eq!(ss.get(0), Some(b"new".as_slice()));
    }

    #[test]
    fn test_atomic_write() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("test.txt");
        atomic_write(&p, b"hello").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "hello");
        assert!(!tmp.path().join("test.tmp").exists());
    }
}
