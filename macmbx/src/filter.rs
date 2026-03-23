//! Filter rule engine — Eudora Filters file compatible.
//!
//! Rules with match criteria (header, body, from, subject, etc.)
//! and actions (transfer, delete, label, priority, sound, etc.).
//! Load/save from Eudora's text file format.
//!
//! Pure match + execute: no UI dependencies.

use crate::toc::{MsgFlags, State, Toc};
use regex::Regex;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// ================================================================
// Enums
// ================================================================

/// Match verb — how to compare a field value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Verb {
    Contains = 1,
    NotContains = 2,
    Is = 3,
    IsNot = 4,
    StartsWith = 5,
    EndsWith = 6,
    Appears = 7,
    NotAppears = 8,
    Regex = 13,
    JunkLess = 14,
    JunkMore = 15,
    DateBefore = 16,
    DateAfter = 17,
    DateIs = 18,
    PriorityIs = 19,
    PriorityAbove = 20,
    PriorityBelow = 21,
}

impl Verb {
    pub fn from_str_name(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "contains" => Self::Contains,
            "!contains" => Self::NotContains,
            "is" => Self::Is,
            "!is" => Self::IsNot,
            "starts" => Self::StartsWith,
            "ends" => Self::EndsWith,
            "appears" => Self::Appears,
            "!appears" => Self::NotAppears,
            "regex" => Self::Regex,
            "less" => Self::JunkLess,
            "greater" => Self::JunkMore,
            "before" => Self::DateBefore,
            "after" => Self::DateAfter,
            "dateis" => Self::DateIs,
            "prioris" => Self::PriorityIs,
            "priorabove" => Self::PriorityAbove,
            "priorbelow" => Self::PriorityBelow,
            _ => Self::Contains,
        }
    }

    pub fn to_str_name(self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::NotContains => "!contains",
            Self::Is => "is",
            Self::IsNot => "!is",
            Self::StartsWith => "starts",
            Self::EndsWith => "ends",
            Self::Appears => "appears",
            Self::NotAppears => "!appears",
            Self::Regex => "regex",
            Self::JunkLess => "less",
            Self::JunkMore => "greater",
            Self::DateBefore => "before",
            Self::DateAfter => "after",
            Self::DateIs => "dateIs",
            Self::PriorityIs => "priorIs",
            Self::PriorityAbove => "priorAbove",
            Self::PriorityBelow => "priorBelow",
        }
    }
}

/// Conjunction between conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Conjunction {
    None = 0,
    And = 1,
    Or = 2,
    Unless = 3,
}

/// Action type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ActionType {
    None = 0,
    Status = 1,
    Priority = 2,
    Label = 3,
    Subject = 4,
    Sound = 5,
    Open = 6,
    Print = 7,
    Forward = 8,
    Redirect = 9,
    Reply = 10,
    Copy = 11,
    Transfer = 12,
    Delete = 13,
    Junk = 14,
    Notify = 15,
    Stop = 16,
    ServerDelete = 17,
    ServerFetch = 18,
    Callback = 99,
}

// ================================================================
// When flags
// ================================================================

/// When the rule applies (bitflags).
pub const WHEN_INCOMING: u8 = 1 << 0;
pub const WHEN_OUTGOING: u8 = 1 << 1;
pub const WHEN_MANUAL: u8 = 1 << 2;

// ================================================================
// Limits
// ================================================================

pub const MAX_CONDITIONS: usize = 2;
pub const MAX_ACTIONS: usize = 5;

// ================================================================
// Condition
// ================================================================

/// One match clause in a filter rule.
#[derive(Debug, Clone)]
pub struct Condition {
    /// Header field name, e.g. "From:", "Subject:", "Any:", "Body:".
    pub header: String,
    pub verb: Verb,
    /// Value to match against.
    pub value: String,
    /// Cached compiled regex (built by `FilterSet::compile`).
    compiled_regex: Option<Regex>,
}

impl Default for Condition {
    fn default() -> Self {
        Self {
            header: String::new(),
            verb: Verb::Contains,
            value: String::new(),
            compiled_regex: None,
        }
    }
}

// ================================================================
// Action
// ================================================================

/// One operation to perform when a rule matches.
#[derive(Debug, Clone)]
pub struct Action {
    pub action_type: ActionType,
    /// Integer value: state, priority, label, junk score.
    pub int_value: i32,
    /// String value: mailbox path, sound name, address, etc.
    pub str_value: String,
}

impl Default for Action {
    fn default() -> Self {
        Self {
            action_type: ActionType::None,
            int_value: 0,
            str_value: String::new(),
        }
    }
}

// ================================================================
// Rule
// ================================================================

/// A filter rule: conditions + actions.
#[derive(Debug, Clone)]
pub struct Rule {
    pub name: String,
    pub id: i32,
    /// WHEN_INCOMING / WHEN_OUTGOING / WHEN_MANUAL flags.
    pub when: u8,
    pub conjunction: Conjunction,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    /// Timestamp of last match (seconds since epoch).
    pub last_match: u32,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            name: String::new(),
            id: 0,
            when: 0,
            conjunction: Conjunction::None,
            conditions: Vec::new(),
            actions: Vec::new(),
            last_match: 0,
        }
    }
}

// ================================================================
// FilterResult
// ================================================================

/// What happened when applying filter rules to a message.
#[derive(Debug, Clone)]
pub struct FilterResult {
    pub matched: bool,
    pub stopped: bool,
    pub transferred: bool,
    pub copied: bool,
    pub deleted: bool,
    pub transfer_dest: String,
    /// -1 = unchanged.
    pub new_state: i32,
    /// -1 = unchanged.
    pub new_priority: i32,
    /// -1 = unchanged.
    pub new_label: i32,
}

impl Default for FilterResult {
    fn default() -> Self {
        Self {
            matched: false,
            stopped: false,
            transferred: false,
            copied: false,
            deleted: false,
            transfer_dest: String::new(),
            new_state: -1,
            new_priority: -1,
            new_label: -1,
        }
    }
}

// ================================================================
// FilterSet
// ================================================================

/// Collection of filter rules, loaded from / saved to an Eudora Filters file.
#[derive(Debug)]
pub struct FilterSet {
    pub rules: Vec<Rule>,
    pub next_id: i32,
    pub path: PathBuf,
}

/// Callback for custom actions (sound, open, print, forward, etc.).
///
/// The callback receives the TOC, message index, and the action that triggered it.
pub type ActionFn = Box<dyn Fn(&mut Toc, usize, &Action)>;

// ================================================================
// String matching helpers
// ================================================================

fn ci_contains(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let hay_lower = hay.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    hay_lower.contains(&needle_lower)
}

fn ci_equals(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn ci_starts(s: &str, prefix: &str) -> bool {
    if prefix.len() > s.len() {
        return false;
    }
    s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn ci_ends(s: &str, suffix: &str) -> bool {
    if suffix.len() > s.len() {
        return false;
    }
    s[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

/// Apply a verb comparison to text and value.
fn match_verb(text: &str, verb: Verb, value: &str) -> bool {
    match verb {
        Verb::Contains => ci_contains(text, value),
        Verb::NotContains => !ci_contains(text, value),
        Verb::Is => ci_equals(text, value),
        Verb::IsNot => !ci_equals(text, value),
        Verb::StartsWith => ci_starts(text, value),
        Verb::EndsWith => ci_ends(text, value),
        Verb::Appears => !text.is_empty(),
        Verb::NotAppears => text.is_empty(),
        Verb::Regex => false, // use match_verb_cond for regex
        Verb::JunkLess | Verb::JunkMore => false, // needs numeric context
        Verb::DateBefore | Verb::DateAfter | Verb::DateIs => false,
        Verb::PriorityIs | Verb::PriorityAbove | Verb::PriorityBelow => false,
    }
}

/// Regex-aware match using cached compiled regex from condition.
fn match_verb_cond(text: &str, cond: &Condition) -> bool {
    if cond.verb != Verb::Regex {
        return match_verb(text, cond.verb, &cond.value);
    }
    // Try cached regex first
    if let Some(ref re) = cond.compiled_regex {
        return re.is_match(text);
    }
    // Fallback: compile on the fly
    match Regex::new(&format!("(?i){}", &cond.value)) {
        Ok(re) => re.is_match(text),
        Err(_) => false,
    }
}

// ================================================================
// Date parsing for filter comparison
// ================================================================

fn now_secs() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

/// Parse a filter date value.
///
/// Accepts:
/// - A plain number: interpreted as days ago from now.
/// - "mm/dd/yyyy" format.
/// - "yyyy-mm-dd" format.
///
/// Returns seconds since epoch, or 0 on failure.
pub fn parse_filter_date(s: &str) -> u32 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    // Plain number = days ago
    if s.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(days) = s.parse::<u32>() {
            return now_secs().saturating_sub(days * 86400);
        }
    }
    // Try mm/dd/yyyy
    if let Some(t) = parse_mdy(s) {
        return t;
    }
    // Try yyyy-mm-dd
    if let Some(t) = parse_ymd(s) {
        return t;
    }
    0
}

/// Parse "mm/dd/yyyy" into seconds since epoch (midnight UTC).
fn parse_mdy(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let m: u32 = parts[0].parse().ok()?;
    let d: u32 = parts[1].parse().ok()?;
    let mut y: u32 = parts[2].parse().ok()?;
    if y < 100 {
        y += 2000;
    }
    date_to_epoch(y, m, d)
}

/// Parse "yyyy-mm-dd" into seconds since epoch (midnight UTC).
fn parse_ymd(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let y: u32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let d: u32 = parts[2].parse().ok()?;
    date_to_epoch(y, m, d)
}

/// Convert a date to seconds since Unix epoch (midnight UTC).
/// Simple calculation without timezone handling, matching the C version's mktime approach.
fn date_to_epoch(year: u32, month: u32, day: u32) -> Option<u32> {
    if month < 1 || month > 12 || day < 1 || day > 31 || year < 1970 {
        return None;
    }
    // Days from 1970-01-01 using the standard algorithm
    let y = if month <= 2 { year - 1 } else { year };
    let m = if month <= 2 { month + 9 } else { month - 3 };
    let days = 365 * y + y / 4 - y / 100 + y / 400 + (m * 306 + 5) / 10 + (day - 1) - 719468;
    Some(days * 86400)
}

/// Compare dates: strip time, compare date-only (within same day).
fn date_day_cmp(a: u32, b: u32) -> i32 {
    let day_a = a / 86400;
    let day_b = b / 86400;
    if day_a < day_b {
        -1
    } else if day_a > day_b {
        1
    } else {
        0
    }
}

// ================================================================
// Condition matching
// ================================================================

/// Provider trait for reading message headers/body from the mbox.
///
/// The filter engine needs to read arbitrary headers and body text
/// from the mbox file. The caller provides this via a trait implementation.
pub trait MessageReader {
    /// Read a specific header field value for the message at `index`.
    fn read_header_field(&self, toc: &Toc, index: usize, field: &str) -> Option<String>;
    /// Read the body text for the message at `index`.
    fn read_body(&self, toc: &Toc, index: usize) -> Option<String>;
}

/// A no-op reader that only uses summary fields (from/subject).
/// Use this when mbox access is not available.
pub struct SummaryOnlyReader;

impl MessageReader for SummaryOnlyReader {
    fn read_header_field(&self, _toc: &Toc, _index: usize, _field: &str) -> Option<String> {
        None
    }
    fn read_body(&self, _toc: &Toc, _index: usize) -> Option<String> {
        None
    }
}

fn match_condition(
    cond: &Condition,
    toc: &Toc,
    index: usize,
    reader: &dyn MessageReader,
) -> bool {
    if cond.header.is_empty() {
        return false;
    }
    let msg = &toc.msgs[index];

    let hdr = cond.header.trim_end_matches(':');

    // --- From: (fast, from summary) ---
    if hdr.eq_ignore_ascii_case("From") {
        return match_verb_cond(&msg.from, cond);
    }

    // --- Subject: (fast, from summary) ---
    if hdr.eq_ignore_ascii_case("Subject") {
        return match_verb_cond(&msg.subject, cond);
    }

    // --- Date: comparison ---
    if hdr.eq_ignore_ascii_case("Date") {
        let target = parse_filter_date(&cond.value);
        if target == 0 {
            return false;
        }
        return match cond.verb {
            Verb::DateBefore => date_day_cmp(msg.seconds, target) < 0,
            Verb::DateAfter => date_day_cmp(msg.seconds, target) > 0,
            Verb::DateIs | Verb::Is => date_day_cmp(msg.seconds, target) == 0,
            Verb::JunkLess => msg.seconds < target,
            Verb::JunkMore => msg.seconds > target,
            _ => false,
        };
    }

    // --- Priority: comparison ---
    if hdr.eq_ignore_ascii_case("Priority") {
        let target: i32 = cond.value.parse().unwrap_or(0);
        return match cond.verb {
            Verb::PriorityIs | Verb::Is => msg.priority as i32 == target,
            Verb::PriorityAbove | Verb::JunkLess => (msg.priority as i32) < target,
            Verb::PriorityBelow | Verb::JunkMore => (msg.priority as i32) > target,
            _ => false,
        };
    }

    // --- Junk: score ---
    if hdr.eq_ignore_ascii_case("Junk") {
        let threshold: i32 = cond.value.parse().unwrap_or(0);
        return match cond.verb {
            Verb::JunkLess => (msg.spam_score as i32) < threshold,
            Verb::JunkMore => (msg.spam_score as i32) > threshold,
            _ => false,
        };
    }

    // --- Personality: match by pers_id or name ---
    if hdr.eq_ignore_ascii_case("Personality") {
        let pop_str = msg.pop_pers_id.to_string();
        if match_verb(&pop_str, cond.verb, &cond.value) {
            return true;
        }
        let pers_str = msg.pers_id.to_string();
        return match_verb(&pers_str, cond.verb, &cond.value);
    }

    // --- Body: search message body text ---
    if hdr.eq_ignore_ascii_case("Body") || cond.header.eq_ignore_ascii_case("<<Body>>") {
        if let Some(body) = reader.read_body(toc, index) {
            return match_verb_cond(&body, cond);
        }
        return false;
    }

    // --- Any: match against all summary fields + To + Cc + body ---
    if hdr.eq_ignore_ascii_case("Any") {
        if match_verb_cond(&msg.from, cond) {
            return true;
        }
        if match_verb_cond(&msg.subject, cond) {
            return true;
        }
        if let Some(to) = reader.read_header_field(toc, index, "To") {
            if match_verb_cond(&to, cond) {
                return true;
            }
        }
        if let Some(cc) = reader.read_header_field(toc, index, "Cc") {
            if match_verb_cond(&cc, cond) {
                return true;
            }
        }
        if let Some(body) = reader.read_body(toc, index) {
            if match_verb_cond(&body, cond) {
                return true;
            }
        }
        return false;
    }

    // --- Generic header — read from mbox ---
    if let Some(val) = reader.read_header_field(toc, index, hdr) {
        return match_verb_cond(&val, cond);
    }
    false
}

// ================================================================
// Rule matching
// ================================================================

impl Rule {
    /// Test if this rule matches a message.
    pub fn matches(&self, toc: &Toc, index: usize, reader: &dyn MessageReader) -> bool {
        if index >= toc.msgs.len() || self.conditions.is_empty() {
            return false;
        }

        let first = match_condition(&self.conditions[0], toc, index, reader);

        if self.conditions.len() < 2 {
            return first;
        }

        let second = match_condition(&self.conditions[1], toc, index, reader);

        match self.conjunction {
            Conjunction::And => first && second,
            Conjunction::Or => first || second,
            Conjunction::Unless => first && !second,
            Conjunction::None => first,
        }
    }
}

// ================================================================
// Action execution
// ================================================================

/// Action pass assignment (matches Eudora's multi-pass order):
/// - Pass 0: metadata changes (status, priority, label, junk, subject).
/// - Pass 1: transfer/copy/delete + stop + callbacks.
fn action_pass(t: ActionType) -> u8 {
    match t {
        ActionType::Status
        | ActionType::Priority
        | ActionType::Label
        | ActionType::Junk
        | ActionType::Subject => 0,
        _ => 1,
    }
}

const MAX_PASS: u8 = 2;

fn execute_action(
    act: &Action,
    toc: &mut Toc,
    index: usize,
    no_xfer: bool,
    result: &mut FilterResult,
    action_fn: Option<&ActionFn>,
) {
    match act.action_type {
        ActionType::Status => {
            if let Some(state) = State::from_u8(act.int_value as u8) {
                toc.msgs[index].state = state;
                toc.dirty = true;
            }
            result.new_state = act.int_value;
        }
        ActionType::Priority => {
            toc.msgs[index].priority = act.int_value as u8;
            toc.dirty = true;
            result.new_priority = act.int_value;
        }
        ActionType::Label => {
            toc.msgs[index].flags = toc.msgs[index].flags.with_label(act.int_value as u8);
            toc.dirty = true;
            result.new_label = act.int_value;
        }
        ActionType::Delete => {
            if !no_xfer {
                toc.msgs[index].flags.insert(MsgFlags::DELETED);
                toc.dirty = true;
                result.deleted = true;
            }
        }
        ActionType::Junk => {
            toc.msgs[index].flags = toc.msgs[index].flags.with_label(0);
            toc.msgs[index].spam_score = act.int_value as i8;
            toc.dirty = true;
        }
        ActionType::Transfer => {
            if !no_xfer && !act.str_value.is_empty() {
                // Transfer is signaled in the result; the caller is responsible
                // for performing the actual mailbox transfer via the store.
                result.transferred = true;
                result.transfer_dest = act.str_value.clone();
            }
        }
        ActionType::Copy => {
            if !no_xfer && !act.str_value.is_empty() {
                result.copied = true;
                result.transfer_dest = act.str_value.clone();
            }
        }
        ActionType::Stop => {
            result.stopped = true;
        }
        ActionType::Sound
        | ActionType::Open
        | ActionType::Print
        | ActionType::Forward
        | ActionType::Redirect
        | ActionType::Reply
        | ActionType::Notify
        | ActionType::Callback => {
            if let Some(f) = action_fn {
                f(toc, index, act);
            }
        }
        _ => {}
    }
}

/// Run all actions for a matched rule using multi-pass execution.
fn run_rule_actions(
    rule: &Rule,
    toc: &mut Toc,
    index: usize,
    no_xfer: bool,
    result: &mut FilterResult,
    action_fn: Option<&ActionFn>,
) {
    for pass in 0..MAX_PASS {
        if result.stopped || result.transferred {
            break;
        }
        for act in &rule.actions {
            if action_pass(act.action_type) != pass {
                continue;
            }
            execute_action(act, toc, index, no_xfer, result, action_fn);
            if result.stopped || result.transferred {
                break;
            }
        }
    }
}

// ================================================================
// FilterSet implementation
// ================================================================

impl FilterSet {
    /// Create a new empty filter set.
    pub fn new() -> Self {
        Self {
            rules: Vec::with_capacity(16),
            next_id: 1,
            path: PathBuf::new(),
        }
    }

    /// Add a rule. Returns the index of the newly added rule.
    /// Auto-assigns an ID if the rule's id is <= 0.
    pub fn add_rule(&mut self, mut rule: Rule) -> usize {
        if rule.id <= 0 {
            rule.id = self.next_id;
            self.next_id += 1;
        } else if rule.id >= self.next_id {
            self.next_id = rule.id + 1;
        }
        self.rules.push(rule);
        self.rules.len() - 1
    }

    /// Remove a rule by index. Returns Ok(()) on success.
    pub fn remove_rule(&mut self, index: usize) -> Result<(), &'static str> {
        if index >= self.rules.len() {
            return Err("index out of range");
        }
        self.rules.remove(index);
        Ok(())
    }

    /// Move a rule from one index to another (reorder).
    pub fn move_rule(&mut self, from: usize, to: usize) -> Result<(), &'static str> {
        if from >= self.rules.len() || to >= self.rules.len() {
            return Err("index out of range");
        }
        let rule = self.rules.remove(from);
        self.rules.insert(to, rule);
        Ok(())
    }

    /// Get a reference to a rule by index.
    pub fn get_rule(&self, index: usize) -> Option<&Rule> {
        self.rules.get(index)
    }

    /// Get a mutable reference to a rule by index.
    pub fn get_rule_mut(&mut self, index: usize) -> Option<&mut Rule> {
        self.rules.get_mut(index)
    }

    /// Find a rule by ID. Returns index or None.
    pub fn find_by_id(&self, id: i32) -> Option<usize> {
        if id <= 0 {
            return None;
        }
        self.rules.iter().position(|r| r.id == id)
    }

    /// Get a reference to a rule by ID.
    pub fn get_by_id(&self, id: i32) -> Option<&Rule> {
        self.find_by_id(id).map(|i| &self.rules[i])
    }

    /// Get a mutable reference to a rule by ID.
    pub fn get_by_id_mut(&mut self, id: i32) -> Option<&mut Rule> {
        if let Some(i) = self.find_by_id(id) {
            Some(&mut self.rules[i])
        } else {
            None
        }
    }

    /// Remove a rule by ID. Returns Ok(()) on success.
    pub fn remove_by_id(&mut self, id: i32) -> Result<(), &'static str> {
        match self.find_by_id(id) {
            Some(idx) => self.remove_rule(idx),
            None => Err("rule not found"),
        }
    }

    /// Precompile regex patterns in all rules for faster matching.
    pub fn compile(&mut self) {
        for rule in &mut self.rules {
            for cond in &mut rule.conditions {
                if cond.verb == Verb::Regex && !cond.value.is_empty() {
                    cond.compiled_regex = Regex::new(&format!("(?i){}", &cond.value)).ok();
                }
            }
        }
    }

    // ── Matching ──

    /// Test if a rule matches a message (by rule index).
    pub fn rule_matches(
        &self,
        rule_index: usize,
        toc: &Toc,
        msg_index: usize,
        reader: &dyn MessageReader,
    ) -> bool {
        match self.rules.get(rule_index) {
            Some(rule) => rule.matches(toc, msg_index, reader),
            None => false,
        }
    }

    // ── Execution ──

    /// Apply all matching rules to a single message.
    pub fn apply(
        &self,
        toc: &mut Toc,
        index: usize,
        reader: &dyn MessageReader,
        action_fn: Option<&ActionFn>,
    ) -> FilterResult {
        self.apply_ex(toc, index, false, reader, action_fn)
    }

    /// Apply all matching rules to a single message, with no_xfer option.
    ///
    /// If `no_xfer` is true, TRANSFER, COPY, and DELETE actions are skipped.
    pub fn apply_ex(
        &self,
        toc: &mut Toc,
        index: usize,
        no_xfer: bool,
        reader: &dyn MessageReader,
        action_fn: Option<&ActionFn>,
    ) -> FilterResult {
        let mut result = FilterResult::default();
        if index >= toc.msgs.len() {
            return result;
        }

        for rule in &self.rules {
            if !rule.matches(toc, index, reader) {
                continue;
            }
            result.matched = true;
            run_rule_actions(rule, toc, index, no_xfer, &mut result, action_fn);
            if result.stopped || result.transferred {
                break;
            }
        }
        result
    }

    /// Apply filters to all messages in a TOC.
    ///
    /// `when`: WHEN_INCOMING, WHEN_OUTGOING, or WHEN_MANUAL flags.
    /// Returns count of messages matched.
    pub fn apply_all(
        &self,
        toc: &mut Toc,
        when: u8,
        reader: &dyn MessageReader,
        action_fn: Option<&ActionFn>,
    ) -> usize {
        let count = toc.msgs.len();
        if count == 0 {
            return 0;
        }
        let mut matched = 0;

        // Process backwards so transfers don't invalidate indices
        for i in (0..count).rev() {
            if toc.msgs[i].flags.contains(MsgFlags::DELETED) {
                continue;
            }

            let mut result = FilterResult::default();

            for rule in &self.rules {
                if when != 0 && (rule.when & when) == 0 {
                    continue;
                }
                if !rule.matches(toc, i, reader) {
                    continue;
                }
                result.matched = true;
                run_rule_actions(rule, toc, i, false, &mut result, action_fn);
                if result.stopped || result.transferred {
                    break;
                }
            }
            if result.matched {
                matched += 1;
            }
        }
        matched
    }

    /// Apply filters to selected messages only (by index slice).
    ///
    /// Indices should be sorted ascending for correct backwards processing.
    pub fn apply_selected(
        &self,
        toc: &mut Toc,
        indices: &[usize],
        when: u8,
        no_xfer: bool,
        reader: &dyn MessageReader,
        action_fn: Option<&ActionFn>,
    ) -> usize {
        if indices.is_empty() {
            return 0;
        }
        let mut matched = 0;

        // Process backwards
        for &idx in indices.iter().rev() {
            if idx >= toc.msgs.len() {
                continue;
            }
            if toc.msgs[idx].flags.contains(MsgFlags::DELETED) {
                continue;
            }

            let mut result = FilterResult::default();

            for rule in &self.rules {
                if when != 0 && (rule.when & when) == 0 {
                    continue;
                }
                if !rule.matches(toc, idx, reader) {
                    continue;
                }
                result.matched = true;
                run_rule_actions(rule, toc, idx, no_xfer, &mut result, action_fn);
                if result.stopped || result.transferred {
                    break;
                }
            }
            if result.matched {
                matched += 1;
            }
        }
        matched
    }

    // ── File I/O ──

    /// Load a filter set from an Eudora Filters file.
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let content = fs::read_to_string(path)?;
        let mut fs = Self::new();
        fs.path = path.to_path_buf();

        let mut cur = Rule::default();
        let mut in_rule = false;
        let mut cond_idx: usize = 0;

        for raw_line in content.lines() {
            let line = raw_line.trim_end();
            if line.is_empty() {
                continue;
            }

            let (key, val) = match line.find(' ') {
                Some(pos) => (&line[..pos], Some(line[pos + 1..].trim_start())),
                None => (line, None),
            };

            match key.to_ascii_lowercase().as_str() {
                "rule" => {
                    // Save previous rule
                    if in_rule && !cur.conditions.is_empty() {
                        fs.add_rule(cur);
                    }
                    cur = Rule::default();
                    in_rule = true;
                    cond_idx = 0;
                    if let Some(v) = val {
                        cur.name = v.to_string();
                    }
                }
                "id" => {
                    if let Some(v) = val {
                        cur.id = v.parse().unwrap_or(0);
                    }
                }
                "incoming" => cur.when |= WHEN_INCOMING,
                "outgoing" => cur.when |= WHEN_OUTGOING,
                "manual" => cur.when |= WHEN_MANUAL,
                "header" => {
                    if cond_idx < MAX_CONDITIONS {
                        if let Some(v) = val {
                            // Ensure we have enough conditions
                            while cur.conditions.len() <= cond_idx {
                                cur.conditions.push(Condition::default());
                            }
                            cur.conditions[cond_idx].header = v.to_string();
                        }
                    }
                }
                "verb" => {
                    if cond_idx < MAX_CONDITIONS {
                        if let Some(v) = val {
                            while cur.conditions.len() <= cond_idx {
                                cur.conditions.push(Condition::default());
                            }
                            cur.conditions[cond_idx].verb = Verb::from_str_name(v);
                        }
                    }
                }
                "value" => {
                    if cond_idx < MAX_CONDITIONS {
                        if let Some(v) = val {
                            while cur.conditions.len() <= cond_idx {
                                cur.conditions.push(Condition::default());
                            }
                            cur.conditions[cond_idx].value = v.to_string();
                        }
                        cond_idx += 1; // move to next condition slot
                    }
                }
                "conjunction" => {
                    if let Some(v) = val {
                        cur.conjunction = match v.to_ascii_lowercase().as_str() {
                            "and" => Conjunction::And,
                            "or" => Conjunction::Or,
                            "unless" => Conjunction::Unless,
                            _ => Conjunction::None,
                        };
                    }
                }
                "status" => {
                    if let Some(v) = val {
                        if cur.actions.len() < MAX_ACTIONS {
                            cur.actions.push(Action {
                                action_type: ActionType::Status,
                                int_value: v.parse().unwrap_or(0),
                                str_value: String::new(),
                            });
                        }
                    }
                }
                "priority" => {
                    if let Some(v) = val {
                        if cur.actions.len() < MAX_ACTIONS {
                            cur.actions.push(Action {
                                action_type: ActionType::Priority,
                                int_value: v.parse().unwrap_or(0),
                                str_value: String::new(),
                            });
                        }
                    }
                }
                "label" => {
                    if let Some(v) = val {
                        if cur.actions.len() < MAX_ACTIONS {
                            cur.actions.push(Action {
                                action_type: ActionType::Label,
                                int_value: v.parse().unwrap_or(0),
                                str_value: String::new(),
                            });
                        }
                    }
                }
                "transfer" => {
                    if let Some(v) = val {
                        if cur.actions.len() < MAX_ACTIONS {
                            cur.actions.push(Action {
                                action_type: ActionType::Transfer,
                                int_value: 0,
                                str_value: v.to_string(),
                            });
                        }
                    }
                }
                "copy" => {
                    if let Some(v) = val {
                        if cur.actions.len() < MAX_ACTIONS {
                            cur.actions.push(Action {
                                action_type: ActionType::Copy,
                                int_value: 0,
                                str_value: v.to_string(),
                            });
                        }
                    }
                }
                "sound" => {
                    if cur.actions.len() < MAX_ACTIONS {
                        cur.actions.push(Action {
                            action_type: ActionType::Sound,
                            int_value: 0,
                            str_value: val.unwrap_or("").to_string(),
                        });
                    }
                }
                "forward" => {
                    if let Some(v) = val {
                        if cur.actions.len() < MAX_ACTIONS {
                            cur.actions.push(Action {
                                action_type: ActionType::Forward,
                                int_value: 0,
                                str_value: v.to_string(),
                            });
                        }
                    }
                }
                "redirect" => {
                    if let Some(v) = val {
                        if cur.actions.len() < MAX_ACTIONS {
                            cur.actions.push(Action {
                                action_type: ActionType::Redirect,
                                int_value: 0,
                                str_value: v.to_string(),
                            });
                        }
                    }
                }
                "junk" => {
                    if cur.actions.len() < MAX_ACTIONS {
                        cur.actions.push(Action {
                            action_type: ActionType::Junk,
                            int_value: val.and_then(|v| v.parse().ok()).unwrap_or(100),
                            str_value: String::new(),
                        });
                    }
                }
                "notifyuser" => {
                    if cur.actions.len() < MAX_ACTIONS {
                        cur.actions.push(Action {
                            action_type: ActionType::Notify,
                            int_value: 0,
                            str_value: String::new(),
                        });
                    }
                }
                "open" => {
                    if cur.actions.len() < MAX_ACTIONS {
                        cur.actions.push(Action {
                            action_type: ActionType::Open,
                            int_value: 0,
                            str_value: String::new(),
                        });
                    }
                }
                "print" => {
                    if cur.actions.len() < MAX_ACTIONS {
                        cur.actions.push(Action {
                            action_type: ActionType::Print,
                            int_value: 0,
                            str_value: String::new(),
                        });
                    }
                }
                "stop" => {
                    if cur.actions.len() < MAX_ACTIONS {
                        cur.actions.push(Action {
                            action_type: ActionType::Stop,
                            int_value: 0,
                            str_value: String::new(),
                        });
                    }
                }
                _ => {} // ignore unknown keys
            }
        }

        // Save last rule
        if in_rule && !cur.conditions.is_empty() {
            fs.add_rule(cur);
        }

        fs.compile();
        Ok(fs)
    }

    /// Save the filter set to the Eudora Filters file.
    pub fn save(&self) -> io::Result<()> {
        if self.path.as_os_str().is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no path set"));
        }
        let mut f = fs::File::create(&self.path)?;

        for rule in &self.rules {
            writeln!(f, "rule {}", rule.name)?;
            if rule.id != 0 {
                writeln!(f, "id {}", rule.id)?;
            }
            if rule.when & WHEN_INCOMING != 0 {
                writeln!(f, "incoming")?;
            }
            if rule.when & WHEN_OUTGOING != 0 {
                writeln!(f, "outgoing")?;
            }
            if rule.when & WHEN_MANUAL != 0 {
                writeln!(f, "manual")?;
            }

            // First condition
            if !rule.conditions.is_empty() {
                writeln!(f, "header {}", rule.conditions[0].header)?;
                writeln!(f, "verb {}", rule.conditions[0].verb.to_str_name())?;
                writeln!(f, "value {}", rule.conditions[0].value)?;
            }

            // Conjunction + second condition
            if rule.conditions.len() >= 2 {
                match rule.conjunction {
                    Conjunction::And => writeln!(f, "conjunction and")?,
                    Conjunction::Or => writeln!(f, "conjunction or")?,
                    Conjunction::Unless => writeln!(f, "conjunction unless")?,
                    Conjunction::None => {}
                }
                writeln!(f, "header {}", rule.conditions[1].header)?;
                writeln!(f, "verb {}", rule.conditions[1].verb.to_str_name())?;
                writeln!(f, "value {}", rule.conditions[1].value)?;
            }

            // Actions
            for act in &rule.actions {
                match act.action_type {
                    ActionType::Status => writeln!(f, "status {}", act.int_value)?,
                    ActionType::Priority => writeln!(f, "priority {}", act.int_value)?,
                    ActionType::Label => writeln!(f, "label {}", act.int_value)?,
                    ActionType::Transfer => writeln!(f, "transfer {}", act.str_value)?,
                    ActionType::Copy => writeln!(f, "copy {}", act.str_value)?,
                    ActionType::Sound => writeln!(f, "sound {}", act.str_value)?,
                    ActionType::Forward => writeln!(f, "forward {}", act.str_value)?,
                    ActionType::Redirect => writeln!(f, "redirect {}", act.str_value)?,
                    ActionType::Junk => writeln!(f, "junk {}", act.int_value)?,
                    ActionType::Notify => writeln!(f, "notifyUser")?,
                    ActionType::Open => writeln!(f, "open")?,
                    ActionType::Print => writeln!(f, "print")?,
                    ActionType::Delete => writeln!(f, "status {}", State::Read as u8)?,
                    ActionType::Stop => writeln!(f, "stop")?,
                    _ => {}
                }
            }
        }
        Ok(())
    }

    // ── Usage tracking ──

    /// Load filter usage (last match timestamps) from a binary file.
    ///
    /// Format: array of `{ id: i32, timestamp: u32 }` records (8 bytes each).
    pub fn load_usage(&mut self, usage_path: impl AsRef<Path>) -> io::Result<()> {
        let path = usage_path.as_ref();
        let data = match fs::read(path) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()), // no file = no history
            Err(e) => return Err(e),
        };

        let mut offset = 0;
        while offset + 8 <= data.len() {
            let id = i32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let when = u32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            offset += 8;

            for rule in &mut self.rules {
                if rule.id == id {
                    rule.last_match = when;
                    break;
                }
            }
        }
        Ok(())
    }

    /// Save filter usage (last match timestamps) to a binary file.
    pub fn save_usage(&self, usage_path: impl AsRef<Path>) -> io::Result<()> {
        let mut buf = Vec::new();
        for rule in &self.rules {
            if rule.last_match == 0 {
                continue;
            }
            buf.extend_from_slice(&rule.id.to_le_bytes());
            buf.extend_from_slice(&rule.last_match.to_le_bytes());
        }
        fs::write(usage_path, &buf)
    }

    /// Note that a rule matched. Updates last_match and optionally saves usage file.
    pub fn note_match(
        &mut self,
        rule_index: usize,
        usage_path: Option<&Path>,
    ) -> Result<(), &'static str> {
        if rule_index >= self.rules.len() {
            return Err("index out of range");
        }
        self.rules[rule_index].last_match = now_secs();
        if let Some(path) = usage_path {
            self.save_usage(path).map_err(|_| "failed to save usage")?;
        }
        Ok(())
    }

    /// Number of rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether the filter set is empty.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

impl Default for FilterSet {
    fn default() -> Self {
        Self::new()
    }
}

// ================================================================
// Tests
// ================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toc::{MsgFlags, MsgSum, State, Toc};

    fn make_toc_with_msgs(msgs: Vec<MsgSum>) -> Toc {
        let mut toc = Toc::new("/tmp/test_filter_mbox");
        toc.msgs = msgs;
        toc
    }

    fn make_msg(from: &str, subject: &str) -> MsgSum {
        let mut m = MsgSum::default();
        m.from = from.to_string();
        m.subject = subject.to_string();
        m
    }

    // ── String matching ──

    #[test]
    fn test_ci_contains() {
        assert!(ci_contains("Hello World", "hello"));
        assert!(ci_contains("Hello World", "WORLD"));
        assert!(ci_contains("Hello World", "lo Wo"));
        assert!(!ci_contains("Hello World", "xyz"));
        assert!(ci_contains("anything", ""));
    }

    #[test]
    fn test_ci_equals() {
        assert!(ci_equals("Hello", "hello"));
        assert!(ci_equals("HELLO", "hello"));
        assert!(!ci_equals("Hello", "World"));
    }

    #[test]
    fn test_ci_starts() {
        assert!(ci_starts("Hello World", "hello"));
        assert!(ci_starts("Hello World", "HELLO W"));
        assert!(!ci_starts("Hello World", "World"));
    }

    #[test]
    fn test_ci_ends() {
        assert!(ci_ends("Hello World", "world"));
        assert!(ci_ends("Hello World", "WORLD"));
        assert!(!ci_ends("Hello World", "Hello"));
    }

    // ── Verb matching ──

    #[test]
    fn test_match_verb_contains() {
        assert!(match_verb("foo bar baz", Verb::Contains, "bar"));
        assert!(!match_verb("foo bar baz", Verb::NotContains, "bar"));
        assert!(match_verb("foo bar baz", Verb::NotContains, "xyz"));
    }

    #[test]
    fn test_match_verb_is() {
        assert!(match_verb("hello", Verb::Is, "HELLO"));
        assert!(!match_verb("hello", Verb::IsNot, "HELLO"));
        assert!(match_verb("hello", Verb::IsNot, "world"));
    }

    #[test]
    fn test_match_verb_starts_ends() {
        assert!(match_verb("hello world", Verb::StartsWith, "hello"));
        assert!(match_verb("hello world", Verb::EndsWith, "world"));
        assert!(!match_verb("hello world", Verb::StartsWith, "world"));
        assert!(!match_verb("hello world", Verb::EndsWith, "hello"));
    }

    #[test]
    fn test_match_verb_appears() {
        assert!(match_verb("some text", Verb::Appears, ""));
        assert!(!match_verb("", Verb::Appears, ""));
        assert!(match_verb("", Verb::NotAppears, ""));
        assert!(!match_verb("some text", Verb::NotAppears, ""));
    }

    // ── Regex matching ──

    #[test]
    fn test_match_verb_cond_regex() {
        let cond = Condition {
            header: "From:".to_string(),
            verb: Verb::Regex,
            value: r"user\d+@example\.com".to_string(),
            compiled_regex: Regex::new(r"(?i)user\d+@example\.com").ok(),
        };
        assert!(match_verb_cond("user42@example.com", &cond));
        assert!(!match_verb_cond("admin@example.com", &cond));
    }

    #[test]
    fn test_match_verb_cond_regex_fallback() {
        // No compiled regex, falls back to on-the-fly compile
        let cond = Condition {
            header: "From:".to_string(),
            verb: Verb::Regex,
            value: r"test\d+".to_string(),
            compiled_regex: None,
        };
        assert!(match_verb_cond("test123", &cond));
        assert!(!match_verb_cond("hello", &cond));
    }

    // ── Date parsing ──

    #[test]
    fn test_parse_mdy() {
        // 01/01/2020 = 2020-01-01 midnight UTC
        let t = parse_mdy("01/01/2020").unwrap();
        assert!(t > 0);
        // 12/31/2020 should be later
        let t2 = parse_mdy("12/31/2020").unwrap();
        assert!(t2 > t);
    }

    #[test]
    fn test_parse_ymd() {
        let t = parse_ymd("2020-01-01").unwrap();
        assert!(t > 0);
        let t2 = parse_ymd("2020-12-31").unwrap();
        assert!(t2 > t);
    }

    #[test]
    fn test_parse_filter_date_days_ago() {
        let t = parse_filter_date("7");
        // Should be roughly 7 days ago from now
        let now = now_secs();
        let diff = now - t;
        assert!(diff >= 6 * 86400 && diff <= 8 * 86400);
    }

    #[test]
    fn test_date_day_cmp() {
        assert_eq!(date_day_cmp(86400, 86400 + 100), 0); // same day
        assert_eq!(date_day_cmp(0, 86400), -1); // a < b
        assert_eq!(date_day_cmp(86400 * 2, 86400), 1); // a > b
    }

    // ── Condition matching (summary fields only) ──

    #[test]
    fn test_condition_from() {
        let toc = make_toc_with_msgs(vec![make_msg("alice@test.com", "Hello")]);
        let reader = SummaryOnlyReader;

        let cond = Condition {
            header: "From:".to_string(),
            verb: Verb::Contains,
            value: "alice".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond, &toc, 0, &reader));

        let cond2 = Condition {
            header: "From".to_string(),
            verb: Verb::Is,
            value: "alice@test.com".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond2, &toc, 0, &reader));
    }

    #[test]
    fn test_condition_subject() {
        let toc = make_toc_with_msgs(vec![make_msg("bob@test.com", "Important Meeting")]);
        let reader = SummaryOnlyReader;

        let cond = Condition {
            header: "Subject:".to_string(),
            verb: Verb::StartsWith,
            value: "important".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond, &toc, 0, &reader));
    }

    #[test]
    fn test_condition_priority() {
        let mut msg = make_msg("x@y.com", "Test");
        msg.priority = 2;
        let toc = make_toc_with_msgs(vec![msg]);
        let reader = SummaryOnlyReader;

        let cond = Condition {
            header: "Priority:".to_string(),
            verb: Verb::PriorityIs,
            value: "2".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond, &toc, 0, &reader));

        // Lower number = higher priority; priority 2 is above 3
        let cond2 = Condition {
            header: "Priority:".to_string(),
            verb: Verb::PriorityAbove,
            value: "3".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond2, &toc, 0, &reader));
    }

    #[test]
    fn test_condition_junk() {
        let mut msg = make_msg("spammer@bad.com", "Buy now");
        msg.spam_score = 80;
        let toc = make_toc_with_msgs(vec![msg]);
        let reader = SummaryOnlyReader;

        let cond_more = Condition {
            header: "Junk:".to_string(),
            verb: Verb::JunkMore,
            value: "50".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond_more, &toc, 0, &reader));

        let cond_less = Condition {
            header: "Junk:".to_string(),
            verb: Verb::JunkLess,
            value: "50".to_string(),
            compiled_regex: None,
        };
        assert!(!match_condition(&cond_less, &toc, 0, &reader));
    }

    #[test]
    fn test_condition_date() {
        let mut msg = make_msg("x@y.com", "Test");
        // Set to a known date: 2020-06-15 = day 18428
        msg.seconds = 18428 * 86400;
        let toc = make_toc_with_msgs(vec![msg]);
        let reader = SummaryOnlyReader;

        let cond = Condition {
            header: "Date:".to_string(),
            verb: Verb::DateAfter,
            value: "2020-01-01".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond, &toc, 0, &reader));

        let cond2 = Condition {
            header: "Date:".to_string(),
            verb: Verb::DateBefore,
            value: "2020-01-01".to_string(),
            compiled_regex: None,
        };
        assert!(!match_condition(&cond2, &toc, 0, &reader));
    }

    #[test]
    fn test_condition_personality() {
        let mut msg = make_msg("x@y.com", "Test");
        msg.pop_pers_id = 12345;
        msg.pers_id = 67890;
        let toc = make_toc_with_msgs(vec![msg]);
        let reader = SummaryOnlyReader;

        let cond = Condition {
            header: "Personality:".to_string(),
            verb: Verb::Is,
            value: "12345".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond, &toc, 0, &reader));

        let cond2 = Condition {
            header: "Personality:".to_string(),
            verb: Verb::Is,
            value: "67890".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond2, &toc, 0, &reader));
    }

    #[test]
    fn test_condition_body_with_reader() {
        struct TestReader;
        impl MessageReader for TestReader {
            fn read_header_field(&self, _: &Toc, _: usize, _: &str) -> Option<String> {
                None
            }
            fn read_body(&self, _: &Toc, _: usize) -> Option<String> {
                Some("This is the body text with a secret keyword.".to_string())
            }
        }

        let toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let reader = TestReader;

        let cond = Condition {
            header: "Body:".to_string(),
            verb: Verb::Contains,
            value: "secret keyword".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond, &toc, 0, &reader));
    }

    #[test]
    fn test_condition_any_with_reader() {
        struct TestReader;
        impl MessageReader for TestReader {
            fn read_header_field(&self, _: &Toc, _: usize, field: &str) -> Option<String> {
                if field == "To" {
                    Some("recipient@example.com".to_string())
                } else {
                    None
                }
            }
            fn read_body(&self, _: &Toc, _: usize) -> Option<String> {
                Some("body text".to_string())
            }
        }

        let toc = make_toc_with_msgs(vec![make_msg("sender@x.com", "Hello")]);
        let reader = TestReader;

        // Matches To header
        let cond = Condition {
            header: "Any:".to_string(),
            verb: Verb::Contains,
            value: "recipient".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond, &toc, 0, &reader));

        // Matches From (summary)
        let cond2 = Condition {
            header: "Any:".to_string(),
            verb: Verb::Contains,
            value: "sender".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond2, &toc, 0, &reader));
    }

    #[test]
    fn test_condition_generic_header() {
        struct TestReader;
        impl MessageReader for TestReader {
            fn read_header_field(&self, _: &Toc, _: usize, field: &str) -> Option<String> {
                if field == "X-Custom" {
                    Some("special-value".to_string())
                } else {
                    None
                }
            }
            fn read_body(&self, _: &Toc, _: usize) -> Option<String> {
                None
            }
        }

        let toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let reader = TestReader;

        let cond = Condition {
            header: "X-Custom:".to_string(),
            verb: Verb::Is,
            value: "special-value".to_string(),
            compiled_regex: None,
        };
        assert!(match_condition(&cond, &toc, 0, &reader));
    }

    // ── Rule matching ──

    #[test]
    fn test_rule_single_condition() {
        let toc = make_toc_with_msgs(vec![make_msg("alice@test.com", "Hello")]);
        let reader = SummaryOnlyReader;

        let rule = Rule {
            conditions: vec![Condition {
                header: "From:".to_string(),
                verb: Verb::Contains,
                value: "alice".to_string(),
                compiled_regex: None,
            }],
            ..Rule::default()
        };
        assert!(rule.matches(&toc, 0, &reader));
    }

    #[test]
    fn test_rule_and_conjunction() {
        let toc = make_toc_with_msgs(vec![make_msg("alice@test.com", "Meeting Tomorrow")]);
        let reader = SummaryOnlyReader;

        let rule = Rule {
            conjunction: Conjunction::And,
            conditions: vec![
                Condition {
                    header: "From:".to_string(),
                    verb: Verb::Contains,
                    value: "alice".to_string(),
                    compiled_regex: None,
                },
                Condition {
                    header: "Subject:".to_string(),
                    verb: Verb::Contains,
                    value: "meeting".to_string(),
                    compiled_regex: None,
                },
            ],
            ..Rule::default()
        };
        assert!(rule.matches(&toc, 0, &reader));

        // Fails if second condition doesn't match
        let rule2 = Rule {
            conjunction: Conjunction::And,
            conditions: vec![
                Condition {
                    header: "From:".to_string(),
                    verb: Verb::Contains,
                    value: "alice".to_string(),
                    compiled_regex: None,
                },
                Condition {
                    header: "Subject:".to_string(),
                    verb: Verb::Contains,
                    value: "deadline".to_string(),
                    compiled_regex: None,
                },
            ],
            ..Rule::default()
        };
        assert!(!rule2.matches(&toc, 0, &reader));
    }

    #[test]
    fn test_rule_or_conjunction() {
        let toc = make_toc_with_msgs(vec![make_msg("bob@test.com", "Meeting")]);
        let reader = SummaryOnlyReader;

        let rule = Rule {
            conjunction: Conjunction::Or,
            conditions: vec![
                Condition {
                    header: "From:".to_string(),
                    verb: Verb::Contains,
                    value: "alice".to_string(), // no match
                    compiled_regex: None,
                },
                Condition {
                    header: "Subject:".to_string(),
                    verb: Verb::Contains,
                    value: "meeting".to_string(), // match
                    compiled_regex: None,
                },
            ],
            ..Rule::default()
        };
        assert!(rule.matches(&toc, 0, &reader));
    }

    #[test]
    fn test_rule_unless_conjunction() {
        let toc = make_toc_with_msgs(vec![make_msg("alice@test.com", "Newsletter")]);
        let reader = SummaryOnlyReader;

        // From contains alice UNLESS Subject contains urgent
        let rule = Rule {
            conjunction: Conjunction::Unless,
            conditions: vec![
                Condition {
                    header: "From:".to_string(),
                    verb: Verb::Contains,
                    value: "alice".to_string(),
                    compiled_regex: None,
                },
                Condition {
                    header: "Subject:".to_string(),
                    verb: Verb::Contains,
                    value: "urgent".to_string(),
                    compiled_regex: None,
                },
            ],
            ..Rule::default()
        };
        // first=true, second=false => true && !false => true
        assert!(rule.matches(&toc, 0, &reader));

        // Now test where "unless" kills it
        let toc2 = make_toc_with_msgs(vec![make_msg("alice@test.com", "Urgent: read now")]);
        // first=true, second=true => true && !true => false
        assert!(!rule.matches(&toc2, 0, &reader));
    }

    #[test]
    fn test_rule_no_conditions() {
        let toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let reader = SummaryOnlyReader;
        let rule = Rule::default();
        assert!(!rule.matches(&toc, 0, &reader));
    }

    #[test]
    fn test_rule_out_of_bounds() {
        let toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let reader = SummaryOnlyReader;
        let rule = Rule {
            conditions: vec![Condition {
                header: "From:".to_string(),
                verb: Verb::Contains,
                value: "x".to_string(),
                compiled_regex: None,
            }],
            ..Rule::default()
        };
        assert!(!rule.matches(&toc, 5, &reader));
    }

    // ── Action execution ──

    #[test]
    fn test_action_status() {
        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();
        let act = Action {
            action_type: ActionType::Status,
            int_value: State::Read as i32,
            str_value: String::new(),
        };
        execute_action(&act, &mut toc, 0, false, &mut result, None);
        assert_eq!(toc.msgs[0].state, State::Read);
        assert_eq!(result.new_state, State::Read as i32);
        assert!(toc.dirty);
    }

    #[test]
    fn test_action_priority() {
        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();
        let act = Action {
            action_type: ActionType::Priority,
            int_value: 1,
            str_value: String::new(),
        };
        execute_action(&act, &mut toc, 0, false, &mut result, None);
        assert_eq!(toc.msgs[0].priority, 1);
        assert_eq!(result.new_priority, 1);
    }

    #[test]
    fn test_action_label() {
        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();
        let act = Action {
            action_type: ActionType::Label,
            int_value: 3,
            str_value: String::new(),
        };
        execute_action(&act, &mut toc, 0, false, &mut result, None);
        assert_eq!(toc.msgs[0].flags.label(), 3);
        assert_eq!(result.new_label, 3);
    }

    #[test]
    fn test_action_delete() {
        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();
        let act = Action {
            action_type: ActionType::Delete,
            int_value: 0,
            str_value: String::new(),
        };
        execute_action(&act, &mut toc, 0, false, &mut result, None);
        assert!(toc.msgs[0].flags.contains(MsgFlags::DELETED));
        assert!(result.deleted);
    }

    #[test]
    fn test_action_delete_no_xfer() {
        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();
        let act = Action {
            action_type: ActionType::Delete,
            int_value: 0,
            str_value: String::new(),
        };
        execute_action(&act, &mut toc, 0, true, &mut result, None);
        assert!(!toc.msgs[0].flags.contains(MsgFlags::DELETED));
        assert!(!result.deleted);
    }

    #[test]
    fn test_action_transfer() {
        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();
        let act = Action {
            action_type: ActionType::Transfer,
            int_value: 0,
            str_value: "Archive".to_string(),
        };
        execute_action(&act, &mut toc, 0, false, &mut result, None);
        assert!(result.transferred);
        assert_eq!(result.transfer_dest, "Archive");
    }

    #[test]
    fn test_action_transfer_no_xfer() {
        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();
        let act = Action {
            action_type: ActionType::Transfer,
            int_value: 0,
            str_value: "Archive".to_string(),
        };
        execute_action(&act, &mut toc, 0, true, &mut result, None);
        assert!(!result.transferred);
    }

    #[test]
    fn test_action_copy() {
        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();
        let act = Action {
            action_type: ActionType::Copy,
            int_value: 0,
            str_value: "Backup".to_string(),
        };
        execute_action(&act, &mut toc, 0, false, &mut result, None);
        assert!(result.copied);
        assert_eq!(result.transfer_dest, "Backup");
    }

    #[test]
    fn test_action_stop() {
        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();
        let act = Action {
            action_type: ActionType::Stop,
            int_value: 0,
            str_value: String::new(),
        };
        execute_action(&act, &mut toc, 0, false, &mut result, None);
        assert!(result.stopped);
    }

    #[test]
    fn test_action_junk() {
        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();
        let act = Action {
            action_type: ActionType::Junk,
            int_value: 90,
            str_value: String::new(),
        };
        execute_action(&act, &mut toc, 0, false, &mut result, None);
        assert_eq!(toc.msgs[0].spam_score, 90);
        assert_eq!(toc.msgs[0].flags.label(), 0);
    }

    #[test]
    fn test_action_callback() {
        use std::sync::{Arc, Mutex};
        let called = Arc::new(Mutex::new(false));
        let called_clone = called.clone();
        let action_fn: ActionFn = Box::new(move |_toc, _idx, act| {
            assert_eq!(act.action_type, ActionType::Sound);
            assert_eq!(act.str_value, "ding");
            *called_clone.lock().unwrap() = true;
        });

        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();
        let act = Action {
            action_type: ActionType::Sound,
            int_value: 0,
            str_value: "ding".to_string(),
        };
        execute_action(&act, &mut toc, 0, false, &mut result, Some(&action_fn));
        assert!(*called.lock().unwrap());
    }

    // ── Multi-pass execution ──

    #[test]
    fn test_run_rule_actions_multipass() {
        let rule = Rule {
            actions: vec![
                // Pass 1 action (transfer)
                Action {
                    action_type: ActionType::Transfer,
                    int_value: 0,
                    str_value: "Archive".to_string(),
                },
                // Pass 0 action (label) — should execute first
                Action {
                    action_type: ActionType::Label,
                    int_value: 2,
                    str_value: String::new(),
                },
            ],
            ..Rule::default()
        };

        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();

        run_rule_actions(&rule, &mut toc, 0, false, &mut result, None);

        // Label (pass 0) should have been applied
        assert_eq!(toc.msgs[0].flags.label(), 2);
        // Transfer (pass 1) should also have been applied
        assert!(result.transferred);
    }

    #[test]
    fn test_run_rule_actions_stop_prevents_transfer() {
        let rule = Rule {
            actions: vec![
                Action {
                    action_type: ActionType::Stop,
                    int_value: 0,
                    str_value: String::new(),
                },
                Action {
                    action_type: ActionType::Transfer,
                    int_value: 0,
                    str_value: "Archive".to_string(),
                },
            ],
            ..Rule::default()
        };

        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let mut result = FilterResult::default();

        run_rule_actions(&rule, &mut toc, 0, false, &mut result, None);
        assert!(result.stopped);
        assert!(!result.transferred); // stop prevented transfer
    }

    // ── FilterSet management ──

    #[test]
    fn test_filterset_add_remove() {
        let mut fs = FilterSet::new();
        assert!(fs.is_empty());

        let rule = Rule {
            name: "Test Rule".to_string(),
            ..Rule::default()
        };
        let idx = fs.add_rule(rule);
        assert_eq!(idx, 0);
        assert_eq!(fs.len(), 1);
        assert_eq!(fs.rules[0].id, 1); // auto-assigned

        fs.remove_rule(0).unwrap();
        assert!(fs.is_empty());
    }

    #[test]
    fn test_filterset_auto_id() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule::default());
        fs.add_rule(Rule::default());
        fs.add_rule(Rule::default());
        assert_eq!(fs.rules[0].id, 1);
        assert_eq!(fs.rules[1].id, 2);
        assert_eq!(fs.rules[2].id, 3);
        assert_eq!(fs.next_id, 4);
    }

    #[test]
    fn test_filterset_explicit_id() {
        let mut fs = FilterSet::new();
        let r = Rule {
            id: 42,
            ..Rule::default()
        };
        fs.add_rule(r);
        assert_eq!(fs.rules[0].id, 42);
        assert_eq!(fs.next_id, 43);

        // Next auto-assigned id starts at 43
        fs.add_rule(Rule::default());
        assert_eq!(fs.rules[1].id, 43);
    }

    #[test]
    fn test_filterset_find_by_id() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule {
            name: "A".to_string(),
            ..Rule::default()
        });
        fs.add_rule(Rule {
            name: "B".to_string(),
            ..Rule::default()
        });
        fs.add_rule(Rule {
            name: "C".to_string(),
            ..Rule::default()
        });

        assert_eq!(fs.find_by_id(2), Some(1));
        assert_eq!(fs.get_by_id(2).unwrap().name, "B");
        assert!(fs.find_by_id(99).is_none());
        assert!(fs.find_by_id(0).is_none());
        assert!(fs.find_by_id(-1).is_none());
    }

    #[test]
    fn test_filterset_remove_by_id() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule {
            name: "A".to_string(),
            ..Rule::default()
        });
        fs.add_rule(Rule {
            name: "B".to_string(),
            ..Rule::default()
        });
        fs.add_rule(Rule {
            name: "C".to_string(),
            ..Rule::default()
        });

        fs.remove_by_id(2).unwrap();
        assert_eq!(fs.len(), 2);
        assert_eq!(fs.rules[0].name, "A");
        assert_eq!(fs.rules[1].name, "C");

        assert!(fs.remove_by_id(99).is_err());
    }

    #[test]
    fn test_filterset_move_rule() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule {
            name: "A".to_string(),
            ..Rule::default()
        });
        fs.add_rule(Rule {
            name: "B".to_string(),
            ..Rule::default()
        });
        fs.add_rule(Rule {
            name: "C".to_string(),
            ..Rule::default()
        });

        fs.move_rule(0, 2).unwrap();
        assert_eq!(fs.rules[0].name, "B");
        assert_eq!(fs.rules[1].name, "C");
        assert_eq!(fs.rules[2].name, "A");

        assert!(fs.move_rule(0, 5).is_err());
    }

    #[test]
    fn test_filterset_compile() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule {
            conditions: vec![Condition {
                header: "From:".to_string(),
                verb: Verb::Regex,
                value: r"\d+".to_string(),
                compiled_regex: None,
            }],
            ..Rule::default()
        });
        assert!(fs.rules[0].conditions[0].compiled_regex.is_none());

        fs.compile();
        assert!(fs.rules[0].conditions[0].compiled_regex.is_some());
    }

    // ── FilterSet apply ──

    #[test]
    fn test_filterset_apply() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule {
            name: "Label Alice".to_string(),
            conditions: vec![Condition {
                header: "From:".to_string(),
                verb: Verb::Contains,
                value: "alice".to_string(),
                compiled_regex: None,
            }],
            actions: vec![Action {
                action_type: ActionType::Label,
                int_value: 5,
                str_value: String::new(),
            }],
            ..Rule::default()
        });

        let mut toc = make_toc_with_msgs(vec![
            make_msg("alice@test.com", "Hello"),
            make_msg("bob@test.com", "Hi"),
        ]);
        let reader = SummaryOnlyReader;

        let r0 = fs.apply(&mut toc, 0, &reader, None);
        assert!(r0.matched);
        assert_eq!(r0.new_label, 5);
        assert_eq!(toc.msgs[0].flags.label(), 5);

        let r1 = fs.apply(&mut toc, 1, &reader, None);
        assert!(!r1.matched);
    }

    #[test]
    fn test_filterset_apply_stop_between_rules() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule {
            name: "Rule 1".to_string(),
            conditions: vec![Condition {
                header: "From:".to_string(),
                verb: Verb::Contains,
                value: "alice".to_string(),
                compiled_regex: None,
            }],
            actions: vec![
                Action {
                    action_type: ActionType::Label,
                    int_value: 1,
                    str_value: String::new(),
                },
                Action {
                    action_type: ActionType::Stop,
                    int_value: 0,
                    str_value: String::new(),
                },
            ],
            ..Rule::default()
        });
        fs.add_rule(Rule {
            name: "Rule 2".to_string(),
            conditions: vec![Condition {
                header: "From:".to_string(),
                verb: Verb::Contains,
                value: "alice".to_string(),
                compiled_regex: None,
            }],
            actions: vec![Action {
                action_type: ActionType::Priority,
                int_value: 1,
                str_value: String::new(),
            }],
            ..Rule::default()
        });

        let mut toc = make_toc_with_msgs(vec![make_msg("alice@test.com", "Test")]);
        let reader = SummaryOnlyReader;

        let result = fs.apply(&mut toc, 0, &reader, None);
        assert!(result.matched);
        assert!(result.stopped);
        assert_eq!(result.new_label, 1);
        // Rule 2 should NOT have fired due to stop
        assert_eq!(result.new_priority, -1);
        assert_eq!(toc.msgs[0].priority, 3); // unchanged default
    }

    #[test]
    fn test_filterset_apply_all() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule {
            name: "Mark alice read".to_string(),
            when: WHEN_INCOMING,
            conditions: vec![Condition {
                header: "From:".to_string(),
                verb: Verb::Contains,
                value: "alice".to_string(),
                compiled_regex: None,
            }],
            actions: vec![Action {
                action_type: ActionType::Status,
                int_value: State::Read as i32,
                str_value: String::new(),
            }],
            ..Rule::default()
        });

        let mut toc = make_toc_with_msgs(vec![
            make_msg("alice@test.com", "Hello"),
            make_msg("bob@test.com", "Hi"),
            make_msg("alice@test.com", "Bye"),
        ]);
        let reader = SummaryOnlyReader;

        let matched = fs.apply_all(&mut toc, WHEN_INCOMING, &reader, None);
        assert_eq!(matched, 2);
        assert_eq!(toc.msgs[0].state, State::Read);
        assert_eq!(toc.msgs[1].state, State::Unread); // bob not matched
        assert_eq!(toc.msgs[2].state, State::Read);
    }

    #[test]
    fn test_filterset_apply_all_skips_deleted() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule {
            when: WHEN_INCOMING,
            conditions: vec![Condition {
                header: "From:".to_string(),
                verb: Verb::Appears,
                value: String::new(),
                compiled_regex: None,
            }],
            actions: vec![Action {
                action_type: ActionType::Label,
                int_value: 7,
                str_value: String::new(),
            }],
            ..Rule::default()
        });

        let mut msg_deleted = make_msg("x@y.com", "Deleted");
        msg_deleted.flags.insert(MsgFlags::DELETED);

        let mut toc = make_toc_with_msgs(vec![make_msg("a@b.com", "Normal"), msg_deleted]);
        let reader = SummaryOnlyReader;

        let matched = fs.apply_all(&mut toc, WHEN_INCOMING, &reader, None);
        assert_eq!(matched, 1);
        assert_eq!(toc.msgs[0].flags.label(), 7);
    }

    #[test]
    fn test_filterset_apply_all_respects_when() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule {
            name: "Incoming only".to_string(),
            when: WHEN_INCOMING,
            conditions: vec![Condition {
                header: "From:".to_string(),
                verb: Verb::Appears,
                value: String::new(),
                compiled_regex: None,
            }],
            actions: vec![Action {
                action_type: ActionType::Label,
                int_value: 2,
                str_value: String::new(),
            }],
            ..Rule::default()
        });

        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let reader = SummaryOnlyReader;

        // Apply as outgoing — should not match
        let matched = fs.apply_all(&mut toc, WHEN_OUTGOING, &reader, None);
        assert_eq!(matched, 0);
        assert_eq!(toc.msgs[0].flags.label(), 0);

        // Apply as incoming — should match
        let matched = fs.apply_all(&mut toc, WHEN_INCOMING, &reader, None);
        assert_eq!(matched, 1);
        assert_eq!(toc.msgs[0].flags.label(), 2);
    }

    #[test]
    fn test_filterset_apply_selected() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule {
            conditions: vec![Condition {
                header: "From:".to_string(),
                verb: Verb::Appears,
                value: String::new(),
                compiled_regex: None,
            }],
            actions: vec![Action {
                action_type: ActionType::Label,
                int_value: 4,
                str_value: String::new(),
            }],
            ..Rule::default()
        });

        let mut toc = make_toc_with_msgs(vec![
            make_msg("a@b.com", "A"),
            make_msg("c@d.com", "B"),
            make_msg("e@f.com", "C"),
        ]);
        let reader = SummaryOnlyReader;

        // Only apply to indices 0 and 2
        let matched = fs.apply_selected(&mut toc, &[0, 2], 0, false, &reader, None);
        assert_eq!(matched, 2);
        assert_eq!(toc.msgs[0].flags.label(), 4);
        assert_eq!(toc.msgs[1].flags.label(), 0); // not selected
        assert_eq!(toc.msgs[2].flags.label(), 4);
    }

    #[test]
    fn test_filterset_apply_ex_no_xfer() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule {
            conditions: vec![Condition {
                header: "From:".to_string(),
                verb: Verb::Appears,
                value: String::new(),
                compiled_regex: None,
            }],
            actions: vec![
                Action {
                    action_type: ActionType::Label,
                    int_value: 3,
                    str_value: String::new(),
                },
                Action {
                    action_type: ActionType::Transfer,
                    int_value: 0,
                    str_value: "Archive".to_string(),
                },
            ],
            ..Rule::default()
        });

        let mut toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let reader = SummaryOnlyReader;

        let result = fs.apply_ex(&mut toc, 0, true, &reader, None);
        assert!(result.matched);
        assert_eq!(result.new_label, 3); // label still applied
        assert!(!result.transferred); // transfer skipped
    }

    // ── File I/O ──

    #[test]
    fn test_load_save_roundtrip() {
        let dir = std::env::temp_dir().join("macmbx_filter_test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("Filters");

        // Create a filter file
        let content = "\
rule Newsletter Filter
id 10
incoming
manual
header From:
verb contains
value newsletter@
conjunction or
header Subject:
verb starts
value [Newsletter]
label 3
transfer Newsletters
rule Spam Catcher
id 20
incoming
header Junk:
verb greater
value 75
junk 100
stop
rule High Priority
id 30
incoming
outgoing
header Priority:
verb priorAbove
value 3
priority 1
sound ding
";
        fs::write(&path, content).unwrap();

        let fs = FilterSet::load(&path).unwrap();
        assert_eq!(fs.rules.len(), 3);

        // Rule 1
        assert_eq!(fs.rules[0].name, "Newsletter Filter");
        assert_eq!(fs.rules[0].id, 10);
        assert_eq!(fs.rules[0].when, WHEN_INCOMING | WHEN_MANUAL);
        assert_eq!(fs.rules[0].conditions.len(), 2);
        assert_eq!(fs.rules[0].conditions[0].header, "From:");
        assert_eq!(fs.rules[0].conditions[0].verb, Verb::Contains);
        assert_eq!(fs.rules[0].conditions[0].value, "newsletter@");
        assert_eq!(fs.rules[0].conjunction, Conjunction::Or);
        assert_eq!(fs.rules[0].conditions[1].header, "Subject:");
        assert_eq!(fs.rules[0].conditions[1].verb, Verb::StartsWith);
        assert_eq!(fs.rules[0].conditions[1].value, "[Newsletter]");
        assert_eq!(fs.rules[0].actions.len(), 2);
        assert_eq!(fs.rules[0].actions[0].action_type, ActionType::Label);
        assert_eq!(fs.rules[0].actions[0].int_value, 3);
        assert_eq!(fs.rules[0].actions[1].action_type, ActionType::Transfer);
        assert_eq!(fs.rules[0].actions[1].str_value, "Newsletters");

        // Rule 2
        assert_eq!(fs.rules[1].name, "Spam Catcher");
        assert_eq!(fs.rules[1].id, 20);
        assert_eq!(fs.rules[1].conditions[0].header, "Junk:");
        assert_eq!(fs.rules[1].conditions[0].verb, Verb::JunkMore);
        assert_eq!(fs.rules[1].conditions[0].value, "75");
        assert_eq!(fs.rules[1].actions.len(), 2);
        assert_eq!(fs.rules[1].actions[0].action_type, ActionType::Junk);
        assert_eq!(fs.rules[1].actions[0].int_value, 100);
        assert_eq!(fs.rules[1].actions[1].action_type, ActionType::Stop);

        // Rule 3
        assert_eq!(fs.rules[2].name, "High Priority");
        assert_eq!(fs.rules[2].id, 30);
        assert_eq!(fs.rules[2].when, WHEN_INCOMING | WHEN_OUTGOING);
        assert_eq!(fs.rules[2].conditions[0].verb, Verb::PriorityAbove);
        assert_eq!(fs.rules[2].actions.len(), 2);
        assert_eq!(fs.rules[2].actions[0].action_type, ActionType::Priority);
        assert_eq!(fs.rules[2].actions[1].action_type, ActionType::Sound);
        assert_eq!(fs.rules[2].actions[1].str_value, "ding");

        // Save and reload
        let path2 = dir.join("Filters2");
        let mut fs2 = fs;
        fs2.path = path2.clone();
        fs2.save().unwrap();

        let fs3 = FilterSet::load(&path2).unwrap();
        assert_eq!(fs3.rules.len(), 3);
        assert_eq!(fs3.rules[0].name, "Newsletter Filter");
        assert_eq!(fs3.rules[0].conditions[0].verb, Verb::Contains);
        assert_eq!(fs3.rules[1].actions[1].action_type, ActionType::Stop);
        assert_eq!(fs3.rules[2].actions[1].str_value, "ding");

        // Cleanup
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_empty_file() {
        let dir = std::env::temp_dir().join("macmbx_filter_empty");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("Filters");
        fs::write(&path, "").unwrap();

        let fs = FilterSet::load(&path).unwrap();
        assert!(fs.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_nonexistent() {
        let result = FilterSet::load("/tmp/nonexistent_macmbx_filter_file_xyz");
        assert!(result.is_err());
    }

    #[test]
    fn test_save_no_path() {
        let fs = FilterSet::new();
        assert!(fs.save().is_err());
    }

    // ── Usage tracking ──

    #[test]
    fn test_usage_roundtrip() {
        let dir = std::env::temp_dir().join("macmbx_filter_usage");
        let _ = fs::create_dir_all(&dir);
        let usage_path = dir.join("filter_usage.bin");

        let mut fs = FilterSet::new();
        fs.add_rule(Rule {
            name: "A".to_string(),
            ..Rule::default()
        });
        fs.add_rule(Rule {
            name: "B".to_string(),
            ..Rule::default()
        });
        fs.add_rule(Rule {
            name: "C".to_string(),
            ..Rule::default()
        });

        // Set some timestamps
        fs.rules[0].last_match = 1000000;
        fs.rules[2].last_match = 2000000;

        fs.save_usage(&usage_path).unwrap();

        // Create a new filter set with same IDs and load usage
        let mut fs2 = FilterSet::new();
        fs2.add_rule(Rule {
            id: 1,
            name: "A".to_string(),
            ..Rule::default()
        });
        fs2.add_rule(Rule {
            id: 2,
            name: "B".to_string(),
            ..Rule::default()
        });
        fs2.add_rule(Rule {
            id: 3,
            name: "C".to_string(),
            ..Rule::default()
        });

        fs2.load_usage(&usage_path).unwrap();
        assert_eq!(fs2.rules[0].last_match, 1000000);
        assert_eq!(fs2.rules[1].last_match, 0);
        assert_eq!(fs2.rules[2].last_match, 2000000);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_usage_missing_file() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule::default());
        // Should not error on missing file
        assert!(fs.load_usage("/tmp/nonexistent_usage_xyz").is_ok());
    }

    #[test]
    fn test_note_match() {
        let mut fs = FilterSet::new();
        fs.add_rule(Rule::default());

        let before = now_secs();
        fs.note_match(0, None).unwrap();
        let after = now_secs();

        assert!(fs.rules[0].last_match >= before);
        assert!(fs.rules[0].last_match <= after);
    }

    #[test]
    fn test_note_match_out_of_range() {
        let mut fs = FilterSet::new();
        assert!(fs.note_match(0, None).is_err());
    }

    // ── Verb parsing ──

    #[test]
    fn test_verb_roundtrip() {
        let verbs = [
            Verb::Contains,
            Verb::NotContains,
            Verb::Is,
            Verb::IsNot,
            Verb::StartsWith,
            Verb::EndsWith,
            Verb::Appears,
            Verb::NotAppears,
            Verb::Regex,
            Verb::JunkLess,
            Verb::JunkMore,
            Verb::DateBefore,
            Verb::DateAfter,
            Verb::DateIs,
            Verb::PriorityIs,
            Verb::PriorityAbove,
            Verb::PriorityBelow,
        ];
        for v in &verbs {
            let s = v.to_str_name();
            let parsed = Verb::from_str_name(s);
            assert_eq!(*v, parsed, "Verb roundtrip failed for {:?} -> {} -> {:?}", v, s, parsed);
        }
    }

    #[test]
    fn test_verb_unknown_defaults_to_contains() {
        assert_eq!(Verb::from_str_name("bogus"), Verb::Contains);
    }

    // ── Integration: load, match, execute ──

    #[test]
    fn test_integration_load_and_apply() {
        let dir = std::env::temp_dir().join("macmbx_filter_integration");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("Filters");

        let content = "\
rule Move newsletters
id 1
incoming
header From:
verb contains
value newsletter@
transfer Newsletters
rule High priority from boss
id 2
incoming
header From:
verb is
value boss@company.com
conjunction and
header Subject:
verb contains
value urgent
priority 1
label 6
";
        fs::write(&path, content).unwrap();

        let fset = FilterSet::load(&path).unwrap();
        let reader = SummaryOnlyReader;

        // Message 1: newsletter
        let mut toc = make_toc_with_msgs(vec![
            make_msg("newsletter@example.com", "Weekly digest"),
            make_msg("boss@company.com", "Urgent: Q4 review"),
            make_msg("random@person.com", "Hi there"),
        ]);

        let r0 = fset.apply(&mut toc, 0, &reader, None);
        assert!(r0.matched);
        assert!(r0.transferred);
        assert_eq!(r0.transfer_dest, "Newsletters");

        let r1 = fset.apply(&mut toc, 1, &reader, None);
        assert!(r1.matched);
        assert_eq!(r1.new_priority, 1);
        assert_eq!(r1.new_label, 6);
        assert_eq!(toc.msgs[1].priority, 1);
        assert_eq!(toc.msgs[1].flags.label(), 6);

        let r2 = fset.apply(&mut toc, 2, &reader, None);
        assert!(!r2.matched);

        let _ = fs::remove_dir_all(&dir);
    }

    // ── Edge cases ──

    #[test]
    fn test_empty_header_condition() {
        let toc = make_toc_with_msgs(vec![make_msg("x@y.com", "Test")]);
        let reader = SummaryOnlyReader;

        let cond = Condition::default(); // empty header
        assert!(!match_condition(&cond, &toc, 0, &reader));
    }

    #[test]
    fn test_action_pass_ordering() {
        assert_eq!(action_pass(ActionType::Status), 0);
        assert_eq!(action_pass(ActionType::Priority), 0);
        assert_eq!(action_pass(ActionType::Label), 0);
        assert_eq!(action_pass(ActionType::Junk), 0);
        assert_eq!(action_pass(ActionType::Subject), 0);
        assert_eq!(action_pass(ActionType::Transfer), 1);
        assert_eq!(action_pass(ActionType::Copy), 1);
        assert_eq!(action_pass(ActionType::Delete), 1);
        assert_eq!(action_pass(ActionType::Stop), 1);
        assert_eq!(action_pass(ActionType::Sound), 1);
        assert_eq!(action_pass(ActionType::Forward), 1);
    }
}
