//! INI-style configuration file + personality/account management.
//!
//! Ported from macmbx_conf.c. Full INI file support:
//! - [sections], key=value, # comments
//! - Preserves comments, blank lines, and section order on round-trip
//! - Personality/account management: enumerate, add, remove, get/set
//! - Eudora geudora.ini compatible

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// ── INI entry: either a key=value pair or a comment/blank line ──

#[derive(Debug, Clone)]
enum Entry {
    KeyValue { key: String, value: String },
    Comment(String), // full-line comment (including # or ;), or empty string for blank lines
}

// ── INI section ──

#[derive(Debug, Clone)]
struct Section {
    name: String, // empty string for global section
    entries: Vec<Entry>,
}

// ── Conf: the whole INI file ──

#[derive(Debug, Clone)]
pub struct Conf {
    path: PathBuf,
    sections: Vec<Section>,
    pub dirty: bool,
}

// ── Account info ──

#[derive(Debug, Clone, Default)]
pub struct Account {
    pub index: i32,
    pub name: String,
    pub real_name: String,
    pub email: String,
    pub account_type: String, // "POP" or "IMAP"
    pub server: String,
    pub smtp_server: String,
    pub username: String,
    pub check_interval: i32,
    pub ssl_mode: i32,
    pub leave_on_server: bool,
    pub enabled: bool,
}

impl Conf {
    // ── Lifecycle ──

    /// Create a new empty config with no file path.
    pub fn new() -> Self {
        Self {
            path: PathBuf::new(),
            sections: vec![Section {
                name: String::new(),
                entries: Vec::new(),
            }],
            dirty: false,
        }
    }

    /// Load an INI file. Returns an empty conf if file does not exist.
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut conf = Self {
            path: path.clone(),
            sections: vec![Section {
                name: String::new(),
                entries: Vec::new(),
            }],
            dirty: false,
        };

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(conf),
            Err(e) => return Err(e),
        };

        let mut cur_sec_idx: usize = 0; // global section

        for raw_line in content.lines() {
            let line = raw_line.trim_end_matches(|c| c == '\r' || c == '\n');
            let trimmed = line.trim_start_matches(|c: char| c == ' ' || c == '\t');

            // Comment line
            if trimmed.starts_with('#') || trimmed.starts_with(';') {
                conf.sections[cur_sec_idx]
                    .entries
                    .push(Entry::Comment(line.to_string()));
                continue;
            }

            // Empty line
            if trimmed.is_empty() {
                conf.sections[cur_sec_idx]
                    .entries
                    .push(Entry::Comment(String::new()));
                continue;
            }

            // Section header
            if trimmed.starts_with('[') {
                if let Some(end) = trimmed.find(']') {
                    let name = trimmed[1..end].trim().to_string();
                    cur_sec_idx = conf.ensure_section_idx(&name);
                }
                continue;
            }

            // Key = value
            if let Some(eq_pos) = trimmed.find('=') {
                let key = trimmed[..eq_pos].trim().to_string();
                let val = trimmed[eq_pos + 1..].trim().to_string();
                conf.ensure_entry(cur_sec_idx, &key, &val);
            }
        }

        Ok(conf)
    }

    /// Save the INI file. Preserves comments and section order.
    /// Writes to a .tmp file then atomically renames.
    pub fn save(&mut self) -> io::Result<()> {
        if self.path.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no file path set",
            ));
        }

        let mut out = String::new();
        for (si, sec) in self.sections.iter().enumerate() {
            // Write section header (skip global empty section name)
            if !sec.name.is_empty() {
                out.push('[');
                out.push_str(&sec.name);
                out.push_str("]\n");
            }

            for entry in &sec.entries {
                match entry {
                    Entry::KeyValue { key, value } => {
                        out.push_str(key);
                        out.push('=');
                        out.push_str(value);
                        out.push('\n');
                    }
                    Entry::Comment(c) => {
                        if c.is_empty() {
                            out.push('\n');
                        } else {
                            out.push_str(c);
                            out.push('\n');
                        }
                    }
                }
            }

            // Blank line between sections (except after last)
            if si + 1 < self.sections.len() {
                out.push('\n');
            }
        }

        let tmp = self.path.with_extension("ini.tmp");
        fs::write(&tmp, &out)?;
        fs::rename(&tmp, &self.path)?;
        self.dirty = false;
        Ok(())
    }

    /// Save to a specific path.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        self.path = path.as_ref().to_path_buf();
        self.save()
    }

    /// Get the file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    // ── Internal helpers ──

    fn find_section_idx(&self, name: &str) -> Option<usize> {
        self.sections
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
    }

    fn ensure_section_idx(&mut self, name: &str) -> usize {
        if let Some(i) = self.find_section_idx(name) {
            return i;
        }
        self.sections.push(Section {
            name: name.to_string(),
            entries: Vec::new(),
        });
        self.sections.len() - 1
    }

    fn find_entry_idx(entries: &[Entry], key: &str) -> Option<usize> {
        entries.iter().position(|e| match e {
            Entry::KeyValue { key: k, .. } => k.eq_ignore_ascii_case(key),
            _ => false,
        })
    }

    fn ensure_entry(&mut self, sec_idx: usize, key: &str, value: &str) {
        let entries = &mut self.sections[sec_idx].entries;
        if let Some(i) = Self::find_entry_idx(entries, key) {
            entries[i] = Entry::KeyValue {
                key: key.to_string(),
                value: value.to_string(),
            };
        } else {
            entries.push(Entry::KeyValue {
                key: key.to_string(),
                value: value.to_string(),
            });
        }
    }

    // ── Get values ──

    /// Get a string value. Returns fallback if not found.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        let sec = self.find_section_idx(section)?;
        let entries = &self.sections[sec].entries;
        let idx = Self::find_entry_idx(entries, key)?;
        match &entries[idx] {
            Entry::KeyValue { value, .. } => Some(value.as_str()),
            _ => None,
        }
    }

    /// Get string with fallback.
    pub fn get_or<'a>(&'a self, section: &str, key: &str, fallback: &'a str) -> &'a str {
        self.get(section, key).unwrap_or(fallback)
    }

    /// Get integer value.
    pub fn get_int(&self, section: &str, key: &str, fallback: i64) -> i64 {
        self.get(section, key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(fallback)
    }

    /// Get boolean value (true/false, yes/no, 1/0).
    pub fn get_bool(&self, section: &str, key: &str, fallback: bool) -> bool {
        match self.get(section, key) {
            Some(v) => {
                let lower = v.to_ascii_lowercase();
                lower == "true" || lower == "yes" || lower == "1"
            }
            None => fallback,
        }
    }

    // ── Set values ──

    /// Set a string value. Creates section and key if not exist.
    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let sec_idx = self.ensure_section_idx(section);
        self.ensure_entry(sec_idx, key, value);
        self.dirty = true;
    }

    /// Set an integer value.
    pub fn set_int(&mut self, section: &str, key: &str, value: i64) {
        self.set(section, key, &value.to_string());
    }

    /// Set a boolean value.
    pub fn set_bool(&mut self, section: &str, key: &str, value: bool) {
        self.set(section, key, if value { "true" } else { "false" });
    }

    // ── Section management ──

    /// Check if a section exists.
    pub fn has_section(&self, section: &str) -> bool {
        self.find_section_idx(section).is_some()
    }

    /// Check if a key exists in a section.
    pub fn has_key(&self, section: &str, key: &str) -> bool {
        self.find_section_idx(section)
            .and_then(|si| Self::find_entry_idx(&self.sections[si].entries, key))
            .is_some()
    }

    /// Remove a key from a section. Returns true if found and removed.
    pub fn remove_key(&mut self, section: &str, key: &str) -> bool {
        if let Some(si) = self.find_section_idx(section) {
            if let Some(ei) = Self::find_entry_idx(&self.sections[si].entries, key) {
                self.sections[si].entries.remove(ei);
                self.dirty = true;
                return true;
            }
        }
        false
    }

    /// Remove an entire section. Returns true if found and removed.
    pub fn remove_section(&mut self, section: &str) -> bool {
        if let Some(si) = self.find_section_idx(section) {
            self.sections.remove(si);
            self.dirty = true;
            return true;
        }
        false
    }

    /// List all section names (excluding the global empty section).
    pub fn list_sections(&self) -> Vec<&str> {
        self.sections
            .iter()
            .filter(|s| !s.name.is_empty())
            .map(|s| s.name.as_str())
            .collect()
    }

    /// List all keys in a section.
    pub fn list_keys(&self, section: &str) -> Vec<&str> {
        match self.find_section_idx(section) {
            Some(si) => self.sections[si]
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::KeyValue { key, .. } => Some(key.as_str()),
                    _ => None,
                })
                .collect(),
            None => Vec::new(),
        }
    }

    // ── Personality / Account management ──

    /// Get the dominant (main) account from global sections.
    pub fn get_dominant(&self) -> Account {
        let use_imap = self.get_bool("checking_mail", "use_imap", false);
        Account {
            index: 0,
            name: self.get_or("sending_mail", "real_name", "").to_string(),
            real_name: self.get_or("sending_mail", "real_name", "").to_string(),
            email: self.get_or("sending_mail", "email_address", "").to_string(),
            account_type: if use_imap { "IMAP" } else { "POP" }.to_string(),
            server: self.get_or("checking_mail", "pop_server", "").to_string(),
            smtp_server: self.get_or("sending_mail", "smtp_server", "").to_string(),
            username: self.get_or("checking_mail", "pop_username", "").to_string(),
            check_interval: self.get_int("checking_mail", "check_interval", 5) as i32,
            ssl_mode: self.get_int("ssl", "ssl_pop_mode", 0) as i32,
            leave_on_server: self.get_bool("checking_mail", "leave_on_server", false),
            enabled: true,
        }
    }

    /// Count personality accounts (account_1, account_2, ...).
    pub fn count_accounts(&self) -> usize {
        let mut count = 0;
        for i in 1..=100 {
            let sec = format!("account_{}", i);
            if self.has_section(&sec) {
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    /// Get account by 1-based index.
    pub fn get_account(&self, index: i32) -> Option<Account> {
        if index < 1 {
            return None;
        }
        let sec = format!("account_{}", index);
        if !self.has_section(&sec) {
            return None;
        }

        let name = self.get_or(&sec, "name", "").to_string();
        Some(Account {
            index,
            real_name: self.get_or(&sec, "real_name", &name).to_string(),
            name,
            email: self.get_or(&sec, "email", "").to_string(),
            account_type: self.get_or(&sec, "type", "POP").to_string(),
            server: self.get_or(&sec, "server", "").to_string(),
            smtp_server: self.get_or(&sec, "smtp_server", "").to_string(),
            username: self.get_or(&sec, "username", "").to_string(),
            check_interval: self.get_int(&sec, "check_interval", 5) as i32,
            ssl_mode: self.get_int(&sec, "ssl_mode", 0) as i32,
            leave_on_server: self.get_bool(&sec, "leave_on_server", false),
            enabled: self.get_bool(&sec, "enabled", true),
        })
    }

    /// Get all accounts.
    pub fn get_all_accounts(&self) -> Vec<Account> {
        let count = self.count_accounts();
        (1..=count as i32)
            .filter_map(|i| self.get_account(i))
            .collect()
    }

    /// Add a new account. Returns the new 1-based index.
    pub fn add_account(&mut self, acct: &Account) -> i32 {
        let idx = self.count_accounts() as i32 + 1;
        let sec = format!("account_{}", idx);

        self.set(&sec, "name", &acct.name);
        self.set(&sec, "real_name", &acct.real_name);
        self.set(&sec, "email", &acct.email);
        self.set(&sec, "type", &acct.account_type);
        self.set(&sec, "server", &acct.server);
        self.set(&sec, "smtp_server", &acct.smtp_server);
        self.set(&sec, "username", &acct.username);
        self.set_int(&sec, "check_interval", acct.check_interval as i64);
        self.set_int(&sec, "ssl_mode", acct.ssl_mode as i64);
        self.set_bool(&sec, "leave_on_server", acct.leave_on_server);
        self.set_bool(&sec, "enabled", acct.enabled);

        idx
    }

    /// Update an existing account by 1-based index.
    pub fn update_account(&mut self, index: i32, acct: &Account) -> bool {
        if index < 1 {
            return false;
        }
        let sec = format!("account_{}", index);
        if !self.has_section(&sec) {
            return false;
        }

        self.set(&sec, "name", &acct.name);
        self.set(&sec, "real_name", &acct.real_name);
        self.set(&sec, "email", &acct.email);
        self.set(&sec, "type", &acct.account_type);
        self.set(&sec, "server", &acct.server);
        self.set(&sec, "smtp_server", &acct.smtp_server);
        self.set(&sec, "username", &acct.username);
        self.set_int(&sec, "check_interval", acct.check_interval as i64);
        self.set_int(&sec, "ssl_mode", acct.ssl_mode as i64);
        self.set_bool(&sec, "leave_on_server", acct.leave_on_server);
        self.set_bool(&sec, "enabled", acct.enabled);

        true
    }

    /// Remove an account and renumber remaining accounts.
    pub fn remove_account(&mut self, index: i32) -> bool {
        if index < 1 {
            return false;
        }
        let count = self.count_accounts() as i32;
        if index > count {
            return false;
        }

        // Remove the section
        let sec = format!("account_{}", index);
        self.remove_section(&sec);

        // Renumber remaining accounts
        for i in (index + 1)..=count {
            let old_name = format!("account_{}", i);
            let new_name = format!("account_{}", i - 1);
            if let Some(si) = self.find_section_idx(&old_name) {
                self.sections[si].name = new_name;
            }
        }
        self.dirty = true;
        true
    }

    /// Find account by email address. Returns 1-based index or None.
    pub fn find_account_by_email(&self, email: &str) -> Option<i32> {
        let count = self.count_accounts();
        for i in 1..=count as i32 {
            let sec = format!("account_{}", i);
            if let Some(e) = self.get(&sec, "email") {
                if e.eq_ignore_ascii_case(email) {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Find account by name. Returns 1-based index or None.
    pub fn find_account_by_name(&self, name: &str) -> Option<i32> {
        let count = self.count_accounts();
        for i in 1..=count as i32 {
            let sec = format!("account_{}", i);
            if let Some(n) = self.get(&sec, "name") {
                if n.eq_ignore_ascii_case(name) {
                    return Some(i);
                }
            }
        }
        None
    }
}

// ================================================================
// Tests
// ================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_temp_ini(content: &str) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.ini");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        (tmp, path)
    }

    #[test]
    fn new_empty_conf() {
        let conf = Conf::new();
        assert!(!conf.dirty);
        assert_eq!(conf.list_sections().len(), 0);
    }

    #[test]
    fn load_nonexistent_returns_empty() {
        let conf = Conf::load("/tmp/nonexistent_conf_12345.ini").unwrap();
        assert_eq!(conf.list_sections().len(), 0);
    }

    #[test]
    fn load_basic_ini() {
        let (_tmp, path) = make_temp_ini(
            "\
# Global comment
global_key=global_val

[section1]
key1=value1
key2=value2

[section2]
alpha=beta
",
        );

        let conf = Conf::load(&path).unwrap();
        assert_eq!(conf.get("", "global_key"), Some("global_val"));
        assert_eq!(conf.get("section1", "key1"), Some("value1"));
        assert_eq!(conf.get("section1", "key2"), Some("value2"));
        assert_eq!(conf.get("section2", "alpha"), Some("beta"));
        assert_eq!(conf.get("section2", "missing"), None);

        let sections = conf.list_sections();
        assert_eq!(sections.len(), 2);
        assert!(sections.contains(&"section1"));
        assert!(sections.contains(&"section2"));


    }

    #[test]
    fn case_insensitive_lookup() {
        let (_tmp, path) = make_temp_ini("[MySection]\nMyKey=MyValue\n");
        let conf = Conf::load(&path).unwrap();
        assert_eq!(conf.get("mysection", "mykey"), Some("MyValue"));
        assert_eq!(conf.get("MYSECTION", "MYKEY"), Some("MyValue"));

    }

    #[test]
    fn get_typed_values() {
        let (_tmp, path) = make_temp_ini(
            "\
[test]
count=42
ratio=-7
flag_yes=true
flag_no=false
flag_1=1
flag_zero=0
yes_val=yes
",
        );

        let conf = Conf::load(&path).unwrap();
        assert_eq!(conf.get_int("test", "count", 0), 42);
        assert_eq!(conf.get_int("test", "ratio", 0), -7);
        assert_eq!(conf.get_int("test", "missing", 99), 99);

        assert!(conf.get_bool("test", "flag_yes", false));
        assert!(!conf.get_bool("test", "flag_no", true));
        assert!(conf.get_bool("test", "flag_1", false));
        assert!(!conf.get_bool("test", "flag_zero", true));
        assert!(conf.get_bool("test", "yes_val", false));
        assert!(!conf.get_bool("test", "missing", false));


    }

    #[test]
    fn set_and_get() {
        let mut conf = Conf::new();
        conf.set("section", "key", "value");
        assert_eq!(conf.get("section", "key"), Some("value"));
        assert!(conf.dirty);

        // Overwrite
        conf.set("section", "key", "new_value");
        assert_eq!(conf.get("section", "key"), Some("new_value"));
    }

    #[test]
    fn set_typed_values() {
        let mut conf = Conf::new();
        conf.set_int("s", "num", 42);
        assert_eq!(conf.get_int("s", "num", 0), 42);

        conf.set_bool("s", "flag", true);
        assert!(conf.get_bool("s", "flag", false));

        conf.set_bool("s", "flag", false);
        assert!(!conf.get_bool("s", "flag", true));
    }

    #[test]
    fn has_section_and_key() {
        let mut conf = Conf::new();
        conf.set("sec", "key", "val");
        assert!(conf.has_section("sec"));
        assert!(!conf.has_section("other"));
        assert!(conf.has_key("sec", "key"));
        assert!(!conf.has_key("sec", "other"));
    }

    #[test]
    fn remove_key() {
        let mut conf = Conf::new();
        conf.set("sec", "key1", "val1");
        conf.set("sec", "key2", "val2");

        assert!(conf.remove_key("sec", "key1"));
        assert!(!conf.has_key("sec", "key1"));
        assert!(conf.has_key("sec", "key2"));
        assert!(!conf.remove_key("sec", "nonexistent"));
    }

    #[test]
    fn remove_section() {
        let mut conf = Conf::new();
        conf.set("sec1", "k", "v");
        conf.set("sec2", "k", "v");

        assert!(conf.remove_section("sec1"));
        assert!(!conf.has_section("sec1"));
        assert!(conf.has_section("sec2"));
        assert!(!conf.remove_section("sec1")); // already removed
    }

    #[test]
    fn list_keys() {
        let mut conf = Conf::new();
        conf.set("sec", "alpha", "1");
        conf.set("sec", "beta", "2");
        conf.set("sec", "gamma", "3");

        let keys = conf.list_keys("sec");
        assert_eq!(keys.len(), 3);
        assert!(keys.contains(&"alpha"));
        assert!(keys.contains(&"beta"));
        assert!(keys.contains(&"gamma"));

        assert_eq!(conf.list_keys("nonexistent").len(), 0);
    }

    #[test]
    fn save_and_reload() {
        let _tmp = tempfile::tempdir().unwrap();
        let path = _tmp.path().join("test_save.ini");
        {
            let mut conf = Conf::new();
            conf.set("", "global", "yes");
            conf.set("mail", "server", "imap.example.com");
            conf.set("mail", "port", "993");
            conf.set_bool("mail", "ssl", true);
            conf.set_int("mail", "timeout", 30);
            conf.save_as(&path).unwrap();
            assert!(!conf.dirty);
        }

        let conf2 = Conf::load(&path).unwrap();
        assert_eq!(conf2.get("", "global"), Some("yes"));
        assert_eq!(conf2.get("mail", "server"), Some("imap.example.com"));
        assert_eq!(conf2.get_int("mail", "port", 0), 993);
        assert!(conf2.get_bool("mail", "ssl", false));
        assert_eq!(conf2.get_int("mail", "timeout", 0), 30);


    }

    #[test]
    fn preserves_comments() {
        let ini = "\
# Top comment
global_key=val

[section]
; Section comment
key=value

key2=value2
";
        let (_tmp, path) = make_temp_ini(ini);
        let mut conf = Conf::load(&path).unwrap();
        conf.set("section", "key3", "value3");
        conf.save_as(&path).unwrap();

        let reloaded = Conf::load(&path).unwrap();
        assert_eq!(reloaded.get("", "global_key"), Some("val"));
        assert_eq!(reloaded.get("section", "key"), Some("value"));
        assert_eq!(reloaded.get("section", "key3"), Some("value3"));

        // Verify comments are in the saved file
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Top comment"));
        assert!(content.contains("; Section comment"));


    }

    // ── Account management tests ──

    #[test]
    fn dominant_account() {
        let (_tmp, path) = make_temp_ini(
            "\
[sending_mail]
real_name=John Doe
email_address=john@example.com
smtp_server=smtp.example.com

[checking_mail]
pop_server=pop.example.com
pop_username=john
check_interval=10
leave_on_server=true

[ssl]
ssl_pop_mode=1
",
        );

        let conf = Conf::load(&path).unwrap();
        let acct = conf.get_dominant();
        assert_eq!(acct.index, 0);
        assert_eq!(acct.name, "John Doe");
        assert_eq!(acct.email, "john@example.com");
        assert_eq!(acct.server, "pop.example.com");
        assert_eq!(acct.smtp_server, "smtp.example.com");
        assert_eq!(acct.username, "john");
        assert_eq!(acct.account_type, "POP");
        assert_eq!(acct.check_interval, 10);
        assert_eq!(acct.ssl_mode, 1);
        assert!(acct.leave_on_server);
        assert!(acct.enabled);


    }

    #[test]
    fn dominant_account_imap() {
        let (_tmp, path) = make_temp_ini(
            "\
[checking_mail]
use_imap=true
",
        );
        let conf = Conf::load(&path).unwrap();
        assert_eq!(conf.get_dominant().account_type, "IMAP");

    }

    #[test]
    fn add_and_get_accounts() {
        let mut conf = Conf::new();

        let acct1 = Account {
            name: "Work".to_string(),
            real_name: "Jane Doe".to_string(),
            email: "jane@work.com".to_string(),
            account_type: "IMAP".to_string(),
            server: "imap.work.com".to_string(),
            smtp_server: "smtp.work.com".to_string(),
            username: "jane".to_string(),
            check_interval: 3,
            ssl_mode: 2,
            leave_on_server: true,
            enabled: true,
            ..Default::default()
        };

        let acct2 = Account {
            name: "Personal".to_string(),
            email: "jane@home.com".to_string(),
            account_type: "POP".to_string(),
            server: "pop.home.com".to_string(),
            enabled: false,
            ..Default::default()
        };

        assert_eq!(conf.add_account(&acct1), 1);
        assert_eq!(conf.add_account(&acct2), 2);
        assert_eq!(conf.count_accounts(), 2);

        let got1 = conf.get_account(1).unwrap();
        assert_eq!(got1.name, "Work");
        assert_eq!(got1.email, "jane@work.com");
        assert_eq!(got1.account_type, "IMAP");
        assert_eq!(got1.ssl_mode, 2);
        assert!(got1.leave_on_server);
        assert!(got1.enabled);

        let got2 = conf.get_account(2).unwrap();
        assert_eq!(got2.name, "Personal");
        assert!(!got2.enabled);

        assert!(conf.get_account(0).is_none());
        assert!(conf.get_account(3).is_none());
    }

    #[test]
    fn get_all_accounts() {
        let mut conf = Conf::new();
        let acct = Account {
            name: "A".to_string(),
            email: "a@a.com".to_string(),
            enabled: true,
            ..Default::default()
        };
        conf.add_account(&acct);
        let acct2 = Account {
            name: "B".to_string(),
            email: "b@b.com".to_string(),
            enabled: true,
            ..Default::default()
        };
        conf.add_account(&acct2);

        let all = conf.get_all_accounts();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "A");
        assert_eq!(all[1].name, "B");
    }

    #[test]
    fn update_account() {
        let mut conf = Conf::new();
        let acct = Account {
            name: "Old".to_string(),
            email: "old@example.com".to_string(),
            enabled: true,
            ..Default::default()
        };
        conf.add_account(&acct);

        let updated = Account {
            name: "New".to_string(),
            email: "new@example.com".to_string(),
            ssl_mode: 1,
            enabled: true,
            ..Default::default()
        };
        assert!(conf.update_account(1, &updated));
        let got = conf.get_account(1).unwrap();
        assert_eq!(got.name, "New");
        assert_eq!(got.email, "new@example.com");
        assert_eq!(got.ssl_mode, 1);

        assert!(!conf.update_account(5, &updated));
        assert!(!conf.update_account(0, &updated));
    }

    #[test]
    fn remove_account_and_renumber() {
        let mut conf = Conf::new();
        for name in &["A", "B", "C"] {
            let acct = Account {
                name: name.to_string(),
                email: format!("{}@example.com", name.to_lowercase()),
                enabled: true,
                ..Default::default()
            };
            conf.add_account(&acct);
        }
        assert_eq!(conf.count_accounts(), 3);

        // Remove middle account "B"
        assert!(conf.remove_account(2));
        assert_eq!(conf.count_accounts(), 2);

        // Account 1 should still be "A"
        let a = conf.get_account(1).unwrap();
        assert_eq!(a.name, "A");

        // Account 2 should now be "C" (renumbered from 3)
        let c = conf.get_account(2).unwrap();
        assert_eq!(c.name, "C");

        assert!(conf.get_account(3).is_none());
    }

    #[test]
    fn remove_account_invalid() {
        let mut conf = Conf::new();
        assert!(!conf.remove_account(0));
        assert!(!conf.remove_account(1));
    }

    #[test]
    fn find_account_by_email() {
        let mut conf = Conf::new();
        for (i, email) in ["a@a.com", "b@b.com", "c@c.com"].iter().enumerate() {
            let acct = Account {
                name: format!("Acct{}", i),
                email: email.to_string(),
                enabled: true,
                ..Default::default()
            };
            conf.add_account(&acct);
        }

        assert_eq!(conf.find_account_by_email("b@b.com"), Some(2));
        assert_eq!(conf.find_account_by_email("B@B.COM"), Some(2)); // case insensitive
        assert_eq!(conf.find_account_by_email("x@x.com"), None);
    }

    #[test]
    fn find_account_by_name() {
        let mut conf = Conf::new();
        for name in &["Alpha", "Beta", "Gamma"] {
            let acct = Account {
                name: name.to_string(),
                email: format!("{}@example.com", name.to_lowercase()),
                enabled: true,
                ..Default::default()
            };
            conf.add_account(&acct);
        }

        assert_eq!(conf.find_account_by_name("Beta"), Some(2));
        assert_eq!(conf.find_account_by_name("beta"), Some(2)); // case insensitive
        assert_eq!(conf.find_account_by_name("Delta"), None);
    }

    #[test]
    fn save_reload_with_accounts() {
        let _tmp = tempfile::tempdir().unwrap();
        let path = _tmp.path().join("test_acct.ini");

        {
            let mut conf = Conf::new();
            let acct = Account {
                name: "Work".to_string(),
                email: "work@example.com".to_string(),
                account_type: "IMAP".to_string(),
                server: "imap.work.com".to_string(),
                smtp_server: "smtp.work.com".to_string(),
                check_interval: 5,
                ssl_mode: 1,
                leave_on_server: true,
                enabled: true,
                ..Default::default()
            };
            conf.add_account(&acct);
            conf.save_as(&path).unwrap();
        }

        let conf2 = Conf::load(&path).unwrap();
        assert_eq!(conf2.count_accounts(), 1);
        let got = conf2.get_account(1).unwrap();
        assert_eq!(got.name, "Work");
        assert_eq!(got.email, "work@example.com");
        assert_eq!(got.account_type, "IMAP");
        assert!(got.leave_on_server);


    }

    #[test]
    fn get_or_fallback() {
        let conf = Conf::new();
        assert_eq!(conf.get_or("sec", "key", "default"), "default");
    }

    #[test]
    fn whitespace_trimming_in_values() {
        let (_tmp, path) = make_temp_ini("[sec]\n  key  =  value with spaces  \n");
        let conf = Conf::load(&path).unwrap();
        assert_eq!(conf.get("sec", "key"), Some("value with spaces"));

    }

    #[test]
    fn empty_value() {
        let (_tmp, path) = make_temp_ini("[sec]\nkey=\n");
        let conf = Conf::load(&path).unwrap();
        assert_eq!(conf.get("sec", "key"), Some(""));

    }

    #[test]
    fn section_with_spaces() {
        let (_tmp, path) = make_temp_ini("[ my section ]\nkey=val\n");
        let conf = Conf::load(&path).unwrap();
        assert_eq!(conf.get("my section", "key"), Some("val"));

    }
}
