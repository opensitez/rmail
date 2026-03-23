//! Nickname / Address Book management.
//!
//! Ported from macmbx_nicknames.c. Eudora nickname file format compatible:
//! - `alias nickname addr1,addr2,...`
//! - `alias "nick with spaces" addr1,addr2,...`
//! - `note nickname <key:value><key:value>...`
//!
//! Multiple address book files in a directory.
//! Supports: CRUD, search, autocomplete, import/export Eudora format.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::vcard;

// ── Hash (djb2, case-insensitive) ──

pub fn nick_hash(s: &str) -> u32 {
    let mut h: u32 = 5381;
    for c in s.bytes() {
        let lower = if c >= b'A' && c <= b'Z' { c + 32 } else { c };
        h = h.wrapping_shl(5).wrapping_add(h).wrapping_add(lower as u32);
    }
    if h == 0 { 1 } else { h }
}

// ── Note field (tagged key-value) ──

#[derive(Debug, Clone, PartialEq)]
pub struct NoteField {
    pub key: String,
    pub value: String,
}

// ── Single nickname entry ──

#[derive(Debug, Clone)]
pub struct Nickname {
    pub name: String,
    pub addresses: Option<String>,
    pub notes: Vec<NoteField>,
    pub name_hash: u32,
    pub addr_hash: u32,
    pub deleted: bool,
}

impl Nickname {
    pub fn new(name: &str, addresses: Option<&str>) -> Self {
        let addr_hash = addresses.map(|a| hash_first_address(a)).unwrap_or(0);
        Self {
            name: name.to_string(),
            addresses: addresses.map(|s| s.to_string()),
            notes: Vec::new(),
            name_hash: nick_hash(name),
            addr_hash,
            deleted: false,
        }
    }
}

// ── Address book (one file) ──

#[derive(Debug, Clone)]
pub struct AddressBook {
    pub name: String,
    pub path: PathBuf,
    pub entries: Vec<Nickname>,
    pub dirty: bool,
}

impl AddressBook {
    pub fn new(name: &str, path: impl AsRef<Path>) -> Self {
        Self {
            name: name.to_string(),
            path: path.as_ref().to_path_buf(),
            entries: Vec::new(),
            dirty: false,
        }
    }
}

// ── Address books collection ──

#[derive(Debug)]
pub struct AddressBooks {
    pub dir_path: PathBuf,
    pub books: Vec<AddressBook>,
}

// ── Note field parsing/serialization ──

/// Find unescaped character, skipping \-escaped ones.
fn find_unescaped(s: &str, c: char) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == c as u8 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Unescape \< \> \\ in a value string.
fn unescape_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            if next == b'<' || next == b'>' || next == b'\\' {
                out.push(next as char);
                i += 2;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Escape < > \ in a value string.
fn escape_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if c == '<' || c == '>' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Parse notes string: `<first:John><last:Doe><note:some \> text>`.
fn parse_notes(s: &str) -> Vec<NoteField> {
    let mut notes = Vec::new();
    let mut pos = 0;
    let bytes = s.as_bytes();

    while pos < bytes.len() {
        if bytes[pos] != b'<' {
            pos += 1;
            continue;
        }
        pos += 1; // skip '<'

        let rest = &s[pos..];
        let colon = match rest.find(':') {
            Some(c) => c,
            None => {
                pos += 1;
                continue;
            }
        };

        let close = match find_unescaped(rest, '>') {
            Some(c) if c > colon => c,
            _ => {
                pos += 1;
                continue;
            }
        };

        let key = &rest[..colon];
        let value = unescape_value(&rest[colon + 1..close]);

        notes.push(NoteField {
            key: key.to_string(),
            value,
        });
        pos += close + 1;
    }
    notes
}

/// Serialize notes to string with escaping.
fn notes_to_string(notes: &[NoteField]) -> String {
    let mut out = String::new();
    for f in notes {
        out.push('<');
        out.push_str(&f.key);
        out.push(':');
        out.push_str(&escape_value(&f.value));
        out.push('>');
    }
    out
}

/// Compute hash from first address in comma-separated list.
fn hash_first_address(addresses: &str) -> u32 {
    let first = match addresses.find(',') {
        Some(pos) => &addresses[..pos],
        None => addresses,
    };
    let trimmed = first.trim();
    if trimmed.is_empty() {
        return 0;
    }
    nick_hash(&trimmed.to_ascii_lowercase())
}

// ── File I/O ──

/// Read a Eudora nickname file into an address book.
fn read_nick_file(book: &mut AddressBook) -> io::Result<()> {
    let content = fs::read_to_string(&book.path)?;

    for raw_line in content.lines() {
        let line = raw_line.trim_end();
        if line.is_empty() {
            continue;
        }

        let is_alias = line.len() > 6
            && line[..6].eq_ignore_ascii_case("alias ");
        let is_note = line.len() > 5
            && line[..5].eq_ignore_ascii_case("note ");

        if !is_alias && !is_note {
            continue;
        }

        let mut p = if is_alias { &line[6..] } else { &line[5..] };
        p = p.trim_start();

        // Parse nickname (may be quoted)
        let (nick, rest) = if p.starts_with('"') {
            let inner = &p[1..];
            match inner.find('"') {
                Some(end) => (&inner[..end], inner[end + 1..].trim_start()),
                None => continue,
            }
        } else {
            match p.find(' ') {
                Some(pos) => (&p[..pos], p[pos..].trim_start()),
                None => (p, ""),
            }
        };

        if nick.is_empty() {
            continue;
        }

        // Find or create the nickname entry
        let idx = match find_nick(&book.entries, nick) {
            Some(i) => i,
            None => {
                book.entries.push(Nickname::new(nick, None));
                book.entries.len() - 1
            }
        };

        if is_alias {
            book.entries[idx].addresses = if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            };
            book.entries[idx].addr_hash = if rest.is_empty() {
                0
            } else {
                hash_first_address(rest)
            };
        } else {
            book.entries[idx].notes = parse_notes(rest);
        }
    }

    book.dirty = false;
    Ok(())
}

/// Find nickname index by name (case-insensitive, skip deleted).
fn find_nick(entries: &[Nickname], name: &str) -> Option<usize> {
    let hash = nick_hash(name);
    entries.iter().position(|n| {
        !n.deleted && n.name_hash == hash && n.name.eq_ignore_ascii_case(name)
    })
}

// ── AddressBook methods ──

impl AddressBook {
    /// Save this address book to its file in Eudora format.
    pub fn save(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }

        let mut out = String::new();

        // Write all alias lines first (Eudora convention)
        for n in &self.entries {
            if n.deleted {
                continue;
            }
            let addrs = match &n.addresses {
                Some(a) if !a.is_empty() => a.as_str(),
                _ => continue,
            };
            if n.name.contains(' ') {
                out.push_str(&format!("alias \"{}\" {}\n", n.name, addrs));
            } else {
                out.push_str(&format!("alias {} {}\n", n.name, addrs));
            }
        }

        // Then all note lines
        for n in &self.entries {
            if n.deleted || n.notes.is_empty() {
                continue;
            }
            let note_str = notes_to_string(&n.notes);
            if note_str.is_empty() {
                continue;
            }
            if n.name.contains(' ') {
                out.push_str(&format!("note \"{}\" {}\n", n.name, note_str));
            } else {
                out.push_str(&format!("note {} {}\n", n.name, note_str));
            }
        }

        let tmp = self.path.with_extension("tmp");
        fs::write(&tmp, &out)?;
        fs::rename(&tmp, &self.path)?;
        self.dirty = false;
        Ok(())
    }

    /// Add a nickname. Returns index.
    pub fn add(&mut self, name: &str, addresses: Option<&str>) -> usize {
        let n = Nickname::new(name, addresses);
        self.entries.push(n);
        self.dirty = true;
        self.entries.len() - 1
    }

    /// Remove a nickname by index (soft delete).
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        self.entries[index].deleted = true;
        self.dirty = true;
        true
    }

    /// Find nickname by name. Returns index or None.
    pub fn find(&self, name: &str) -> Option<usize> {
        find_nick(&self.entries, name)
    }

    /// Get addresses for a nickname.
    pub fn get_addresses(&self, index: usize) -> Option<&str> {
        self.entries.get(index).and_then(|n| n.addresses.as_deref())
    }

    /// Set addresses for a nickname.
    pub fn set_addresses(&mut self, index: usize, addresses: Option<&str>) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        self.entries[index].addresses = addresses.map(|s| s.to_string());
        self.entries[index].addr_hash = addresses.map(|a| hash_first_address(a)).unwrap_or(0);
        self.dirty = true;
        true
    }

    /// Rename a nickname.
    pub fn rename(&mut self, index: usize, new_name: &str) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        self.entries[index].name = new_name.to_string();
        self.entries[index].name_hash = nick_hash(new_name);
        self.dirty = true;
        true
    }

    /// Get a note field value.
    pub fn get_field(&self, index: usize, key: &str) -> Option<&str> {
        let entry = self.entries.get(index)?;
        entry
            .notes
            .iter()
            .find(|f| f.key.eq_ignore_ascii_case(key))
            .map(|f| f.value.as_str())
    }

    /// Set a note field value. Creates field if not exists.
    pub fn set_field(&mut self, index: usize, key: &str, value: &str) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        let entry = &mut self.entries[index];
        if let Some(f) = entry.notes.iter_mut().find(|f| f.key.eq_ignore_ascii_case(key)) {
            f.value = value.to_string();
        } else {
            entry.notes.push(NoteField {
                key: key.to_string(),
                value: value.to_string(),
            });
        }
        self.dirty = true;
        true
    }

    /// Remove a note field. Returns true if found and removed.
    pub fn remove_field(&mut self, index: usize, key: &str) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        let notes = &mut self.entries[index].notes;
        let before = notes.len();
        notes.retain(|f| !f.key.eq_ignore_ascii_case(key));
        if notes.len() < before {
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Import contacts from a .vcf (vCard) file.
    ///
    /// Parses all vCards in the file and adds each as a nickname.
    /// Maps vCard fields to Eudora nickname note fields:
    /// FN/N -> nickname name, EMAIL -> addresses,
    /// TEL -> phone/phone2/mobile/fax, ORG -> company,
    /// TITLE -> title, URL -> web, NOTE -> note, ADR -> address fields.
    /// Returns the number of contacts imported.
    pub fn import_vcard(&mut self, vcf_path: &Path) -> io::Result<usize> {
        let cards = vcard::parse_file(vcf_path)?;
        if cards.is_empty() {
            return Ok(0);
        }

        let mut imported = 0;
        for vc in &cards {
            // Determine nickname from FN, N, or EMAIL
            let fn_name = vc.fn_name().unwrap_or("");
            let given = vc.given().unwrap_or("");
            let family = vc.family().unwrap_or("");
            let email = vc.email().unwrap_or("");

            let nick = if !fn_name.is_empty() {
                fn_name.to_string()
            } else if !given.is_empty() || !family.is_empty() {
                let mut n = String::new();
                if !given.is_empty() {
                    n.push_str(given);
                }
                if !given.is_empty() && !family.is_empty() {
                    n.push(' ');
                }
                if !family.is_empty() {
                    n.push_str(family);
                }
                n
            } else if !email.is_empty() {
                email.to_string()
            } else {
                continue;
            };

            // Collect all emails
            let emails = vc.emails();
            let addrs = if emails.is_empty() {
                None
            } else {
                Some(emails.join(","))
            };

            let idx = self.add(&nick, addrs.as_deref());

            // Set name fields
            if !given.is_empty() {
                self.set_field(idx, "first", given);
            }
            if !family.is_empty() {
                self.set_field(idx, "last", family);
            }

            // Phone — try types
            if let Some(ph) = vc.phone(Some("HOME")) {
                self.set_field(idx, "phone", ph);
            } else if let Some(ph) = vc.phone(None) {
                self.set_field(idx, "phone", ph);
            }
            if let Some(ph) = vc.phone(Some("WORK")) {
                self.set_field(idx, "phone2", ph);
            }
            if let Some(ph) = vc.phone(Some("CELL")) {
                self.set_field(idx, "mobile", ph);
            }
            if let Some(ph) = vc.phone(Some("FAX")) {
                self.set_field(idx, "fax", ph);
            }

            // Organization, title
            if let Some(org) = vc.org() {
                self.set_field(idx, "company", org);
            }
            if let Some(title) = vc.get("TITLE") {
                self.set_field(idx, "title", title);
            }

            // URL
            if let Some(url) = vc.get("URL") {
                self.set_field(idx, "web", url);
            }

            // Note
            if let Some(note) = vc.get("NOTE") {
                self.set_field(idx, "note", note);
            }

            // Address — ADR: PO;Ext;Street;City;State;Zip;Country
            let street = vc.get_part("ADR", 2).unwrap_or("");
            let city = vc.get_part("ADR", 3).unwrap_or("");
            let state = vc.get_part("ADR", 4).unwrap_or("");
            let zip = vc.get_part("ADR", 5).unwrap_or("");
            let country = vc.get_part("ADR", 6).unwrap_or("");
            if !street.is_empty() { self.set_field(idx, "address", street); }
            if !city.is_empty() { self.set_field(idx, "city", city); }
            if !state.is_empty() { self.set_field(idx, "state", state); }
            if !zip.is_empty() { self.set_field(idx, "zip", zip); }
            if !country.is_empty() { self.set_field(idx, "country", country); }

            imported += 1;
        }

        Ok(imported)
    }

    /// Export a nickname as a vCard string.
    ///
    /// Builds a vCard 3.0 from the nickname's addresses and note fields:
    /// FN, N (from first/last notes), EMAIL (from addresses),
    /// TEL (phone/phone2/mobile/fax), ORG, TITLE, URL, NOTE, ADR.
    /// Returns None if the nickname is deleted or index is out of range.
    pub fn export_vcard(&self, index: usize) -> Option<String> {
        let n = self.entries.get(index)?;
        if n.deleted {
            return None;
        }

        let mut vc = vcard::Vcard::new();

        // FN + N
        let first = n.notes.iter().find(|f| f.key.eq_ignore_ascii_case("first")).map(|f| f.value.as_str());
        let last = n.notes.iter().find(|f| f.key.eq_ignore_ascii_case("last")).map(|f| f.value.as_str());

        if first.is_some() || last.is_some() {
            let fn_name = format!(
                "{}{}{}",
                first.unwrap_or(""),
                if first.is_some() && last.is_some() { " " } else { "" },
                last.unwrap_or("")
            );
            vc.add("FN", &fn_name);
            vc.add_structured("N", &[
                last.unwrap_or(""),
                first.unwrap_or(""),
                "", "", "",
            ]);
        } else {
            vc.add("FN", &n.name);
        }

        // EMAIL(s)
        if let Some(ref addrs) = n.addresses {
            for addr in addrs.split(',') {
                let addr = addr.trim();
                if !addr.is_empty() {
                    vc.add_typed("EMAIL", addr, "INTERNET");
                }
            }
        }

        // Phones with types
        let get_field = |key: &str| -> Option<&str> {
            n.notes.iter()
                .find(|f| f.key.eq_ignore_ascii_case(key))
                .map(|f| f.value.as_str())
        };

        if let Some(v) = get_field("phone") { vc.add_typed("TEL", v, "HOME,VOICE"); }
        if let Some(v) = get_field("phone2") { vc.add_typed("TEL", v, "WORK,VOICE"); }
        if let Some(v) = get_field("mobile") { vc.add_typed("TEL", v, "CELL"); }
        if let Some(v) = get_field("fax") { vc.add_typed("TEL", v, "FAX"); }

        // Org, title, URL, note
        if let Some(v) = get_field("company") { vc.add("ORG", v); }
        if let Some(v) = get_field("title") { vc.add("TITLE", v); }
        if let Some(v) = get_field("web") { vc.add("URL", v); }
        if let Some(v) = get_field("note") { vc.add("NOTE", v); }

        // Address — ADR: PO;Ext;Street;City;State;Zip;Country
        let addr = get_field("address");
        let city = get_field("city");
        let state = get_field("state");
        let zip = get_field("zip");
        let country = get_field("country");

        if addr.is_some() || city.is_some() || state.is_some() || zip.is_some() || country.is_some() {
            vc.add_structured("ADR", &[
                "", "",
                addr.unwrap_or(""),
                city.unwrap_or(""),
                state.unwrap_or(""),
                zip.unwrap_or(""),
                country.unwrap_or(""),
            ]);
        }

        Some(vc.serialize())
    }
}

// ── AddressBooks methods ──

impl AddressBooks {
    /// Open all address books from a nicknames directory.
    /// Creates directory if needed.
    pub fn open(nick_dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = nick_dir.as_ref();
        if !dir.exists() {
            fs::create_dir_all(dir)?;
        }

        let mut books = Vec::new();

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Skip hidden files, .toc, .tmp, .lock
            if name.starts_with('.') {
                continue;
            }
            if name.ends_with(".toc") || name.ends_with(".tmp") || name.ends_with(".lock") {
                continue;
            }

            let mut book = AddressBook::new(&name, &path);
            let _ = read_nick_file(&mut book);
            books.push(book);
        }

        Ok(Self {
            dir_path: dir.to_path_buf(),
            books,
        })
    }

    /// Save all dirty address books.
    pub fn save(&mut self) -> io::Result<()> {
        for book in &mut self.books {
            if book.dirty {
                book.save()?;
            }
        }
        Ok(())
    }

    /// Create a new empty address book.
    pub fn create_book(&mut self, name: &str) -> io::Result<usize> {
        let path = self.dir_path.join(name);
        // Create empty file
        fs::write(&path, "")?;
        let mut book = AddressBook::new(name, &path);
        book.dirty = true;
        self.books.push(book);
        Ok(self.books.len() - 1)
    }

    /// Remove an address book (deletes file).
    pub fn remove_book(&mut self, index: usize) -> io::Result<()> {
        if index >= self.books.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "book index out of range",
            ));
        }
        let _ = fs::remove_file(&self.books[index].path);
        self.books.remove(index);
        Ok(())
    }

    /// Get address book by index.
    pub fn get_book(&self, index: usize) -> Option<&AddressBook> {
        self.books.get(index)
    }

    /// Get mutable address book by index.
    pub fn get_book_mut(&mut self, index: usize) -> Option<&mut AddressBook> {
        self.books.get_mut(index)
    }

    /// Find address book by name (case-insensitive).
    pub fn find_book(&self, name: &str) -> Option<usize> {
        self.books
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(name))
    }

    /// Find nickname by name across all books. Returns (book_idx, nick_idx).
    pub fn find_all(&self, name: &str) -> Option<(usize, usize)> {
        for (bi, book) in self.books.iter().enumerate() {
            if let Some(ni) = book.find(name) {
                return Some((bi, ni));
            }
        }
        None
    }

    /// Check if an email address appears in any nickname (case-insensitive substring).
    pub fn contains_address(&self, email: &str) -> bool {
        let email_lower = email.to_ascii_lowercase();
        for book in &self.books {
            for n in &book.entries {
                if n.deleted {
                    continue;
                }
                if let Some(ref addrs) = n.addresses {
                    if addrs.to_ascii_lowercase().contains(&email_lower) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if an address hash exists in any book.
    pub fn contains_hash(&self, addr_hash: u32) -> bool {
        if addr_hash == 0 {
            return false;
        }
        for book in &self.books {
            for n in &book.entries {
                if !n.deleted && n.addr_hash == addr_hash {
                    return true;
                }
            }
        }
        false
    }

    /// Expand a nickname to its addresses.
    pub fn expand(&self, name: &str) -> Option<String> {
        let (bi, ni) = self.find_all(name)?;
        self.books[bi].entries[ni].addresses.clone()
    }

    /// Search nicknames by pattern (case-insensitive substring in name, addresses, or notes).
    /// Returns vec of (book_idx, nick_idx).
    pub fn search(&self, pattern: &str) -> Vec<(usize, usize)> {
        let pat_lower = pattern.to_ascii_lowercase();
        let mut results = Vec::new();

        for (bi, book) in self.books.iter().enumerate() {
            for (ni, n) in book.entries.iter().enumerate() {
                if n.deleted {
                    continue;
                }
                let mut matched = n.name.to_ascii_lowercase().contains(&pat_lower);
                if !matched {
                    if let Some(ref addrs) = n.addresses {
                        if addrs.to_ascii_lowercase().contains(&pat_lower) {
                            matched = true;
                        }
                    }
                }
                if !matched {
                    for f in &n.notes {
                        if f.value.to_ascii_lowercase().contains(&pat_lower) {
                            matched = true;
                            break;
                        }
                    }
                }
                if matched {
                    results.push((bi, ni));
                }
            }
        }
        results
    }
}

// ── Autocomplete ──

/// Autocomplete match priority (lower = better).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchPriority {
    NamePrefix = 1,
    FirstLastPrefix = 2,
    AddrPrefix = 3,
    NameContains = 4,
    AddrContains = 5,
}

/// Autocomplete result.
#[derive(Debug, Clone)]
pub struct CompleteResult {
    pub book_idx: usize,
    pub nick_idx: usize,
    pub name: String,
    pub addresses: Option<String>,
    pub display: String,
}

fn ci_prefix(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len()
        && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn ci_contains(s: &str, needle: &str) -> bool {
    s.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}

impl AddressBooks {
    /// Autocomplete: find nicknames matching a prefix.
    /// Results sorted by match quality: name prefix > first/last > address prefix > contains.
    pub fn complete(&self, prefix: &str, max_results: usize) -> Vec<CompleteResult> {
        if prefix.is_empty() {
            return Vec::new();
        }

        let mut entries: Vec<(MatchPriority, CompleteResult)> = Vec::new();

        for (bi, book) in self.books.iter().enumerate() {
            for (ni, n) in book.entries.iter().enumerate() {
                if n.deleted {
                    continue;
                }

                let priority = if ci_prefix(&n.name, prefix) {
                    Some(MatchPriority::NamePrefix)
                } else {
                    // Check first+last name
                    let first = n.notes.iter().find(|f| f.key.eq_ignore_ascii_case("first"));
                    let last = n.notes.iter().find(|f| f.key.eq_ignore_ascii_case("last"));

                    let fl_match = match (first, last) {
                        (Some(ff), _) if ci_prefix(&ff.value, prefix) => true,
                        (_, Some(lf)) if ci_prefix(&lf.value, prefix) => true,
                        (Some(ff), Some(lf)) => {
                            let full = format!("{} {}", ff.value, lf.value);
                            ci_prefix(&full, prefix)
                        }
                        _ => false,
                    };

                    if fl_match {
                        Some(MatchPriority::FirstLastPrefix)
                    } else if n
                        .addresses
                        .as_ref()
                        .map(|a| ci_prefix(a, prefix))
                        .unwrap_or(false)
                    {
                        Some(MatchPriority::AddrPrefix)
                    } else if ci_contains(&n.name, prefix) {
                        Some(MatchPriority::NameContains)
                    } else if n
                        .addresses
                        .as_ref()
                        .map(|a| ci_contains(a, prefix))
                        .unwrap_or(false)
                    {
                        Some(MatchPriority::AddrContains)
                    } else {
                        None
                    }
                };

                if let Some(prio) = priority {
                    // Build display string
                    let first = n.notes.iter().find(|f| f.key.eq_ignore_ascii_case("first"));
                    let last = n.notes.iter().find(|f| f.key.eq_ignore_ascii_case("last"));
                    let first_addr = n.addresses.as_ref().map(|a| {
                        match a.find(',') {
                            Some(pos) => a[..pos].trim().to_string(),
                            None => a.trim().to_string(),
                        }
                    });
                    let addr_part = first_addr.as_deref().unwrap_or("");

                    let display = match (first, last) {
                        (Some(ff), Some(lf)) => {
                            format!("{} {} <{}>", ff.value, lf.value, addr_part)
                        }
                        (Some(ff), None) => format!("{} <{}>", ff.value, addr_part),
                        _ => format!("{} <{}>", n.name, addr_part),
                    };

                    entries.push((
                        prio,
                        CompleteResult {
                            book_idx: bi,
                            nick_idx: ni,
                            name: n.name.clone(),
                            addresses: n.addresses.clone(),
                            display,
                        },
                    ));
                }
            }
        }

        // Sort by priority (stable)
        entries.sort_by_key(|(p, _)| *p);

        // Limit
        let limit = if max_results > 0 {
            max_results.min(entries.len())
        } else {
            entries.len()
        };

        entries
            .into_iter()
            .take(limit)
            .map(|(_, r)| r)
            .collect()
    }
}

// ── Import/Export Eudora format ──

impl AddressBook {
    /// Import from a Eudora nickname file into this book.
    pub fn import_eudora(&mut self, eudora_file: impl AsRef<Path>) -> io::Result<()> {
        let saved_path = self.path.clone();
        self.path = eudora_file.as_ref().to_path_buf();
        let result = read_nick_file(self);
        self.path = saved_path;
        self.dirty = true;
        result
    }

    /// Export this book to a Eudora nickname file.
    pub fn export_eudora(&self, eudora_file: impl AsRef<Path>) -> io::Result<()> {
        let mut out = String::new();

        // Alias lines first
        for n in &self.entries {
            if n.deleted {
                continue;
            }
            let addrs = match &n.addresses {
                Some(a) if !a.is_empty() => a.as_str(),
                _ => continue,
            };
            if n.name.contains(' ') {
                out.push_str(&format!("alias \"{}\" {}\n", n.name, addrs));
            } else {
                out.push_str(&format!("alias {} {}\n", n.name, addrs));
            }
        }

        // Note lines
        for n in &self.entries {
            if n.deleted || n.notes.is_empty() {
                continue;
            }
            let note_str = notes_to_string(&n.notes);
            if note_str.is_empty() {
                continue;
            }
            if n.name.contains(' ') {
                out.push_str(&format!("note \"{}\" {}\n", n.name, note_str));
            } else {
                out.push_str(&format!("note {} {}\n", n.name, note_str));
            }
        }

        fs::write(eudora_file, &out)
    }
}

// ================================================================
// Tests
// ================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ── Hash tests ──

    #[test]
    fn hash_basic() {
        let h = nick_hash("hello");
        assert_ne!(h, 0);
        // Case insensitive
        assert_eq!(nick_hash("Hello"), nick_hash("hello"));
        assert_eq!(nick_hash("HELLO"), nick_hash("hello"));
    }

    #[test]
    fn hash_empty_string() {
        let h = nick_hash("");
        // djb2 of empty string is 5381
        assert_eq!(h, 5381);
    }

    #[test]
    fn hash_different_strings() {
        assert_ne!(nick_hash("alice"), nick_hash("bob"));
    }

    // ── Note parsing tests ──

    #[test]
    fn parse_notes_basic() {
        let notes = parse_notes("<first:John><last:Doe>");
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].key, "first");
        assert_eq!(notes[0].value, "John");
        assert_eq!(notes[1].key, "last");
        assert_eq!(notes[1].value, "Doe");
    }

    #[test]
    fn parse_notes_with_escaping() {
        let notes = parse_notes(r"<note:some \> text>");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].key, "note");
        assert_eq!(notes[0].value, "some > text");
    }

    #[test]
    fn parse_notes_empty() {
        let notes = parse_notes("");
        assert!(notes.is_empty());
    }

    #[test]
    fn parse_notes_malformed() {
        // No closing >
        let notes = parse_notes("<key:value");
        assert!(notes.is_empty());

        // No colon
        let notes = parse_notes("<keyvalue>");
        assert!(notes.is_empty());
    }

    #[test]
    fn notes_roundtrip() {
        let original = vec![
            NoteField {
                key: "first".to_string(),
                value: "John".to_string(),
            },
            NoteField {
                key: "note".to_string(),
                value: "has <angle> brackets and \\ backslash".to_string(),
            },
        ];

        let serialized = notes_to_string(&original);
        let parsed = parse_notes(&serialized);

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].key, "first");
        assert_eq!(parsed[0].value, "John");
        assert_eq!(parsed[1].key, "note");
        assert_eq!(parsed[1].value, "has <angle> brackets and \\ backslash");
    }

    #[test]
    fn escape_and_unescape() {
        let original = "test <value> with \\ special";
        let escaped = escape_value(original);
        assert_eq!(escaped, r"test \<value\> with \\ special");
        let unescaped = unescape_value(&escaped);
        assert_eq!(unescaped, original);
    }

    // ── Hash first address ──

    #[test]
    fn hash_first_address_single() {
        let h = hash_first_address("alice@example.com");
        assert_ne!(h, 0);
    }

    #[test]
    fn hash_first_address_multiple() {
        let h1 = hash_first_address("alice@example.com,bob@example.com");
        let h2 = hash_first_address("alice@example.com");
        assert_eq!(h1, h2); // uses first address only
    }

    #[test]
    fn hash_first_address_empty() {
        assert_eq!(hash_first_address(""), 0);
    }

    // ── Nickname / AddressBook tests ──

    #[test]
    fn nickname_new() {
        let n = Nickname::new("john", Some("john@example.com"));
        assert_eq!(n.name, "john");
        assert_eq!(n.addresses.as_deref(), Some("john@example.com"));
        assert_ne!(n.name_hash, 0);
        assert_ne!(n.addr_hash, 0);
        assert!(!n.deleted);
    }

    #[test]
    fn address_book_add_and_find() {
        let mut book = AddressBook::new("test", "/tmp/test_nicks");
        let idx = book.add("alice", Some("alice@example.com"));
        assert_eq!(idx, 0);
        assert_eq!(book.find("alice"), Some(0));
        assert_eq!(book.find("ALICE"), Some(0)); // case insensitive
        assert_eq!(book.find("bob"), None);
    }

    #[test]
    fn address_book_remove() {
        let mut book = AddressBook::new("test", "/tmp/test_nicks");
        book.add("alice", Some("alice@example.com"));
        assert!(book.remove(0));
        assert_eq!(book.find("alice"), None); // soft-deleted
        assert!(book.entries[0].deleted);
    }

    #[test]
    fn address_book_get_set_addresses() {
        let mut book = AddressBook::new("test", "/tmp/test_nicks");
        book.add("alice", Some("alice@example.com"));
        assert_eq!(book.get_addresses(0), Some("alice@example.com"));

        assert!(book.set_addresses(0, Some("alice@new.com,alice@other.com")));
        assert_eq!(book.get_addresses(0), Some("alice@new.com,alice@other.com"));

        assert!(!book.set_addresses(5, Some("nope")));
    }

    #[test]
    fn address_book_rename() {
        let mut book = AddressBook::new("test", "/tmp/test_nicks");
        book.add("alice", Some("alice@example.com"));
        assert!(book.rename(0, "alice_new"));
        assert_eq!(book.entries[0].name, "alice_new");
        assert_eq!(book.find("alice_new"), Some(0));
        assert_eq!(book.find("alice"), None);
    }

    #[test]
    fn address_book_note_fields() {
        let mut book = AddressBook::new("test", "/tmp/test_nicks");
        book.add("alice", Some("alice@example.com"));

        assert!(book.set_field(0, "first", "Alice"));
        assert!(book.set_field(0, "last", "Smith"));
        assert_eq!(book.get_field(0, "first"), Some("Alice"));
        assert_eq!(book.get_field(0, "last"), Some("Smith"));
        assert_eq!(book.get_field(0, "phone"), None);

        // Update existing field
        assert!(book.set_field(0, "first", "Alicia"));
        assert_eq!(book.get_field(0, "first"), Some("Alicia"));

        // Remove field
        assert!(book.remove_field(0, "last"));
        assert_eq!(book.get_field(0, "last"), None);
        assert!(!book.remove_field(0, "nonexistent"));
    }

    // ── File I/O tests ──

    fn make_nick_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("test_nicks_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn cleanup_dir(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn read_eudora_nick_file() {
        let dir = make_nick_dir();
        let file = dir.join("Eudora Nicknames");
        fs::write(
            &file,
            "\
alias alice alice@example.com
alias bob bob@example.com,bob2@example.com
alias \"John Doe\" john@example.com
note alice <first:Alice><last:Smith>
note \"John Doe\" <first:John><last:Doe><note:A note>
",
        )
        .unwrap();

        let mut book = AddressBook::new("Eudora Nicknames", &file);
        read_nick_file(&mut book).unwrap();

        assert_eq!(book.entries.len(), 3);

        // alice
        assert_eq!(book.find("alice"), Some(0));
        assert_eq!(book.get_addresses(0), Some("alice@example.com"));
        assert_eq!(book.get_field(0, "first"), Some("Alice"));
        assert_eq!(book.get_field(0, "last"), Some("Smith"));

        // bob
        assert_eq!(book.find("bob"), Some(1));
        assert_eq!(
            book.get_addresses(1),
            Some("bob@example.com,bob2@example.com")
        );

        // John Doe (quoted)
        assert_eq!(book.find("John Doe"), Some(2));
        assert_eq!(book.get_addresses(2), Some("john@example.com"));
        assert_eq!(book.get_field(2, "first"), Some("John"));
        assert_eq!(book.get_field(2, "last"), Some("Doe"));
        assert_eq!(book.get_field(2, "note"), Some("A note"));

        cleanup_dir(&dir);
    }

    #[test]
    fn save_and_reload_book() {
        let dir = make_nick_dir();
        let file = dir.join("test_book");

        {
            let mut book = AddressBook::new("test_book", &file);
            book.add("alice", Some("alice@example.com"));
            book.set_field(0, "first", "Alice");
            book.set_field(0, "last", "Smith");
            book.add("bob", Some("bob@example.com"));
            book.add("John Doe", Some("john@example.com"));
            book.save().unwrap();
        }

        {
            let mut book = AddressBook::new("test_book", &file);
            read_nick_file(&mut book).unwrap();
            assert_eq!(book.entries.len(), 3);
            assert_eq!(book.get_addresses(0), Some("alice@example.com"));
            assert_eq!(book.get_field(0, "first"), Some("Alice"));
            assert_eq!(book.find("bob"), Some(1));
            assert_eq!(book.find("John Doe"), Some(2));
        }

        cleanup_dir(&dir);
    }

    #[test]
    fn save_skips_deleted() {
        let dir = make_nick_dir();
        let file = dir.join("test_deleted");

        {
            let mut book = AddressBook::new("test_deleted", &file);
            book.add("keep", Some("keep@example.com"));
            book.add("remove", Some("remove@example.com"));
            book.remove(1);
            book.save().unwrap();
        }

        {
            let mut book = AddressBook::new("test_deleted", &file);
            read_nick_file(&mut book).unwrap();
            assert_eq!(book.entries.len(), 1);
            assert_eq!(book.entries[0].name, "keep");
        }

        cleanup_dir(&dir);
    }

    // ── AddressBooks tests ──

    #[test]
    fn open_address_books_dir() {
        let dir = make_nick_dir();
        let file1 = dir.join("book1");
        let file2 = dir.join("book2");
        fs::write(&file1, "alias alice alice@a.com\n").unwrap();
        fs::write(&file2, "alias bob bob@b.com\n").unwrap();

        let abs = AddressBooks::open(&dir).unwrap();
        assert_eq!(abs.books.len(), 2);

        cleanup_dir(&dir);
    }

    #[test]
    fn create_and_remove_book() {
        let dir = make_nick_dir();
        let mut abs = AddressBooks::open(&dir).unwrap();
        assert_eq!(abs.books.len(), 0);

        let idx = abs.create_book("new_book").unwrap();
        assert_eq!(abs.books.len(), 1);
        assert_eq!(abs.books[idx].name, "new_book");

        abs.remove_book(0).unwrap();
        assert_eq!(abs.books.len(), 0);

        cleanup_dir(&dir);
    }

    #[test]
    fn find_book_by_name() {
        let dir = make_nick_dir();
        fs::write(dir.join("MyBook"), "alias alice alice@a.com\n").unwrap();
        let abs = AddressBooks::open(&dir).unwrap();

        assert!(abs.find_book("MyBook").is_some());
        assert!(abs.find_book("mybook").is_some()); // case insensitive
        assert!(abs.find_book("other").is_none());

        cleanup_dir(&dir);
    }

    #[test]
    fn find_all_across_books() {
        let dir = make_nick_dir();
        fs::write(dir.join("book1"), "alias alice alice@a.com\n").unwrap();
        fs::write(dir.join("book2"), "alias bob bob@b.com\n").unwrap();

        let abs = AddressBooks::open(&dir).unwrap();
        assert!(abs.find_all("alice").is_some());
        assert!(abs.find_all("bob").is_some());
        assert!(abs.find_all("charlie").is_none());

        cleanup_dir(&dir);
    }

    #[test]
    fn contains_address_search() {
        let dir = make_nick_dir();
        fs::write(dir.join("book"), "alias alice alice@example.com,alice2@example.com\n").unwrap();
        let abs = AddressBooks::open(&dir).unwrap();

        assert!(abs.contains_address("alice@example.com"));
        assert!(abs.contains_address("ALICE@EXAMPLE.COM")); // case insensitive
        assert!(abs.contains_address("alice2@example.com"));
        assert!(!abs.contains_address("bob@example.com"));

        cleanup_dir(&dir);
    }

    #[test]
    fn contains_hash_search() {
        let dir = make_nick_dir();
        fs::write(dir.join("book"), "alias alice alice@example.com\n").unwrap();
        let abs = AddressBooks::open(&dir).unwrap();

        let hash = hash_first_address("alice@example.com");
        assert!(abs.contains_hash(hash));
        assert!(!abs.contains_hash(0));
        assert!(!abs.contains_hash(99999));

        cleanup_dir(&dir);
    }

    #[test]
    fn expand_nickname() {
        let dir = make_nick_dir();
        fs::write(dir.join("book"), "alias team alice@a.com,bob@b.com\n").unwrap();
        let abs = AddressBooks::open(&dir).unwrap();

        assert_eq!(abs.expand("team"), Some("alice@a.com,bob@b.com".to_string()));
        assert_eq!(abs.expand("nonexistent"), None);

        cleanup_dir(&dir);
    }

    #[test]
    fn search_nicknames() {
        let dir = make_nick_dir();
        fs::write(
            &dir.join("book"),
            "\
alias alice alice@example.com
alias bob bob@example.com
note alice <first:Alice><last:Smith>
",
        )
        .unwrap();
        let abs = AddressBooks::open(&dir).unwrap();

        // Search by name
        let results = abs.search("alice");
        assert_eq!(results.len(), 1);

        // Search by address
        let results = abs.search("bob@");
        assert_eq!(results.len(), 1);

        // Search by note field value
        let results = abs.search("Smith");
        assert_eq!(results.len(), 1);

        // Search matches both
        let results = abs.search("example.com");
        assert_eq!(results.len(), 2);

        // No match
        let results = abs.search("charlie");
        assert_eq!(results.len(), 0);

        cleanup_dir(&dir);
    }

    // ── Autocomplete tests ──

    #[test]
    fn complete_by_name_prefix() {
        let dir = make_nick_dir();
        fs::write(dir.join("book"), "alias alice alice@a.com\nalias bob bob@b.com\n").unwrap();
        let abs = AddressBooks::open(&dir).unwrap();

        let results = abs.complete("ali", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "alice");
    }

    #[test]
    fn complete_by_first_last() {
        let dir = make_nick_dir();
        fs::write(
            &dir.join("book"),
            "alias jd john@example.com\nnote jd <first:John><last:Doe>\n",
        )
        .unwrap();
        let abs = AddressBooks::open(&dir).unwrap();

        let results = abs.complete("John", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "jd");
        assert!(results[0].display.contains("John"));
        assert!(results[0].display.contains("Doe"));
    }

    #[test]
    fn complete_by_address_prefix() {
        let dir = make_nick_dir();
        fs::write(dir.join("book"), "alias alice alice@example.com\n").unwrap();
        let abs = AddressBooks::open(&dir).unwrap();

        let results = abs.complete("alice@", 10);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn complete_priority_order() {
        let dir = make_nick_dir();
        // "al" matches:
        //   alice -> name prefix (1)
        //   xyzal -> name contains (4)
        //   al_addr -> address prefix (3)
        fs::write(
            &dir.join("book"),
            "\
alias alice alice@a.com
alias xyzal xyzal@b.com
alias zz al@c.com
",
        )
        .unwrap();
        let abs = AddressBooks::open(&dir).unwrap();

        let results = abs.complete("al", 10);
        assert!(results.len() >= 2);
        // First result should be alice (name prefix)
        assert_eq!(results[0].name, "alice");
    }

    #[test]
    fn complete_max_results() {
        let dir = make_nick_dir();
        let mut content = String::new();
        for i in 0..20 {
            content.push_str(&format!("alias test{} test{}@a.com\n", i, i));
        }
        fs::write(dir.join("book"), &content).unwrap();
        let abs = AddressBooks::open(&dir).unwrap();

        let results = abs.complete("test", 5);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn complete_empty_prefix_returns_nothing() {
        let dir = make_nick_dir();
        fs::write(dir.join("book"), "alias alice alice@a.com\n").unwrap();
        let abs = AddressBooks::open(&dir).unwrap();

        let results = abs.complete("", 10);
        assert!(results.is_empty());

        cleanup_dir(&dir);
    }

    // ── Import/Export tests ──

    #[test]
    fn import_eudora_file() {
        let dir = make_nick_dir();
        let src = dir.join("source");
        let dest = dir.join("dest");

        fs::write(
            &src,
            "alias imported imported@example.com\nnote imported <first:Test>\n",
        )
        .unwrap();
        fs::write(&dest, "").unwrap();

        let mut book = AddressBook::new("dest", &dest);
        book.import_eudora(&src).unwrap();

        assert_eq!(book.find("imported"), Some(0));
        assert_eq!(book.get_addresses(0), Some("imported@example.com"));
        assert_eq!(book.get_field(0, "first"), Some("Test"));

        cleanup_dir(&dir);
    }

    #[test]
    fn export_eudora_file() {
        let dir = make_nick_dir();
        let export_path = dir.join("exported");

        let mut book = AddressBook::new("test", dir.join("test"));
        book.add("alice", Some("alice@example.com"));
        book.set_field(0, "first", "Alice");
        book.add("John Doe", Some("john@example.com"));

        book.export_eudora(&export_path).unwrap();

        let content = fs::read_to_string(&export_path).unwrap();
        assert!(content.contains("alias alice alice@example.com"));
        assert!(content.contains("alias \"John Doe\" john@example.com"));
        assert!(content.contains("note alice <first:Alice>"));

        cleanup_dir(&dir);
    }

    #[test]
    fn export_preserves_escaping() {
        let dir = make_nick_dir();
        let export_path = dir.join("exported");

        let mut book = AddressBook::new("test", dir.join("test"));
        book.add("test", Some("test@example.com"));
        book.set_field(0, "note", "has <angle> brackets");

        book.export_eudora(&export_path).unwrap();

        let content = fs::read_to_string(&export_path).unwrap();
        assert!(content.contains(r"<note:has \<angle\> brackets>"));

        // Reload and verify round-trip
        let mut book2 = AddressBook::new("test2", &export_path);
        read_nick_file(&mut book2).unwrap();
        assert_eq!(book2.get_field(0, "note"), Some("has <angle> brackets"));

        cleanup_dir(&dir);
    }

    #[test]
    fn skip_hidden_and_temp_files() {
        let dir = make_nick_dir();
        fs::write(dir.join("good"), "alias a a@a.com\n").unwrap();
        fs::write(dir.join(".hidden"), "alias b b@b.com\n").unwrap();
        fs::write(dir.join("file.toc"), "binary data").unwrap();
        fs::write(dir.join("file.tmp"), "temp data").unwrap();
        fs::write(dir.join("file.lock"), "lock").unwrap();

        let abs = AddressBooks::open(&dir).unwrap();
        assert_eq!(abs.books.len(), 1);
        assert_eq!(abs.books[0].name, "good");

        cleanup_dir(&dir);
    }

    #[test]
    fn save_all_dirty_books() {
        let dir = make_nick_dir();
        let mut abs = AddressBooks::open(&dir).unwrap();

        let idx = abs.create_book("book1").unwrap();
        abs.books[idx].add("alice", Some("alice@a.com"));

        let idx2 = abs.create_book("book2").unwrap();
        abs.books[idx2].add("bob", Some("bob@b.com"));

        abs.save().unwrap();

        // Reload and verify
        let abs2 = AddressBooks::open(&dir).unwrap();
        assert_eq!(abs2.books.len(), 2);

        cleanup_dir(&dir);
    }

    #[test]
    fn remove_book_invalid_index() {
        let dir = make_nick_dir();
        let mut abs = AddressBooks::open(&dir).unwrap();
        assert!(abs.remove_book(99).is_err());
        cleanup_dir(&dir);
    }

    #[test]
    fn get_book_methods() {
        let dir = make_nick_dir();
        fs::write(dir.join("book"), "alias alice alice@a.com\n").unwrap();
        let mut abs = AddressBooks::open(&dir).unwrap();

        assert!(abs.get_book(0).is_some());
        assert!(abs.get_book(99).is_none());
        assert!(abs.get_book_mut(0).is_some());
        assert!(abs.get_book_mut(99).is_none());

        cleanup_dir(&dir);
    }

    #[test]
    fn address_book_remove_invalid() {
        let mut book = AddressBook::new("test", "/tmp/test");
        assert!(!book.remove(0)); // no entries
        assert!(!book.rename(0, "nope"));
    }
}
