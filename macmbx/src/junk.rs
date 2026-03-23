//! Junk mail scoring and management.
//!
//! Ported from macmbx_junk.c. Scoring is pluggable via callback.
//! Core logic: score, mark, move, archive, rescan.

use crate::toc::{MsgFlags, MailboxType, MsgSum, Toc};
use crate::store::Store;
use crate::ops;
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

// ── Spam score sources (matches Eudora spamBecause field) ──

pub const BECAUSE_NOT_JUNK: u8 = 0;
pub const BECAUSE_PLUG: u8 = 1;
pub const BECAUSE_USER: u8 = 2;
pub const BECAUSE_XFER: u8 = 3;
pub const BECAUSE_WHITELIST: u8 = 4;

// ── Score action ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreAction {
    Auto,
    Rescan,
    UserJunk,
    UserNotJunk,
}

// ── Callback types ──

/// Scoring callback: receives (toc, index, headers, body, action) -> score 0..=100 or None.
pub type ScoreFn = Box<dyn Fn(&Toc, usize, Option<&str>, Option<&str>, ScoreAction) -> Option<i8>>;

/// Whitelist callback: receives sender "From" field, returns true if whitelisted.
pub type WhitelistFn = Box<dyn Fn(&str) -> bool>;

// ── Junk configuration ──

pub struct JunkConfig {
    pub threshold: i8,
    pub archive_days: i32,
    pub archive_threshold: i8,
    pub believe_date: bool,
    pub server_delete: bool,
    score_fn: Option<ScoreFn>,
    whitelist_fn: Option<WhitelistFn>,
}

impl Default for JunkConfig {
    fn default() -> Self {
        Self {
            threshold: 50,
            archive_days: 14,
            archive_threshold: 50,
            believe_date: false,
            server_delete: false,
            score_fn: None,
            whitelist_fn: None,
        }
    }
}

impl JunkConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_scorer(&mut self, f: ScoreFn) {
        self.score_fn = Some(f);
    }

    pub fn set_whitelist(&mut self, f: WhitelistFn) {
        self.whitelist_fn = Some(f);
    }
}

// ── Scoring ──

/// Score a single message. Updates spam_score and spam_because.
/// `read_headers` and `read_body` are callbacks to read message content from the mbox.
/// Returns the score (0..=100) or None if not scored.
pub fn score(
    cfg: &JunkConfig,
    toc: &mut Toc,
    index: usize,
    read_headers: impl Fn(&Toc, usize) -> Option<String>,
    read_body: impl Fn(&Toc, usize) -> Option<String>,
) -> Option<i8> {
    if index >= toc.msgs.len() {
        return None;
    }

    // Whitelist check
    if let Some(ref wl) = cfg.whitelist_fn {
        if wl(&toc.msgs[index].from) {
            toc.msgs[index].spam_score = 0;
            toc.msgs[index].spam_because = BECAUSE_WHITELIST;
            toc.dirty = true;
            return Some(0);
        }
    }

    // Call scorer callback
    let score_fn = cfg.score_fn.as_ref()?;

    let headers = read_headers(toc, index);
    let body = read_body(toc, index);

    let raw_score = score_fn(
        toc,
        index,
        headers.as_deref(),
        body.as_deref(),
        ScoreAction::Auto,
    )?;

    let clamped = raw_score.clamp(0, 100);
    toc.msgs[index].spam_score = clamped;
    toc.msgs[index].spam_because = BECAUSE_PLUG;
    toc.dirty = true;
    Some(clamped)
}

/// Score all unscored, non-deleted messages.
/// Returns count scored.
pub fn score_box(
    cfg: &JunkConfig,
    toc: &mut Toc,
    read_headers: impl Fn(&Toc, usize) -> Option<String>,
    read_body: impl Fn(&Toc, usize) -> Option<String>,
) -> usize {
    let mut scored = 0;
    for i in 0..toc.msgs.len() {
        if toc.msgs[i].flags.contains(MsgFlags::DELETED) {
            continue;
        }
        if toc.msgs[i].spam_score != -1 {
            continue;
        }
        if score(cfg, toc, i, &read_headers, &read_body).is_some() {
            scored += 1;
        }
    }
    scored
}

/// Rescore all messages (except user-marked and deleted).
/// Returns count scored.
pub fn rescore_box(
    cfg: &JunkConfig,
    toc: &mut Toc,
    read_headers: impl Fn(&Toc, usize) -> Option<String>,
    read_body: impl Fn(&Toc, usize) -> Option<String>,
) -> usize {
    let mut scored = 0;
    for i in 0..toc.msgs.len() {
        if toc.msgs[i].flags.contains(MsgFlags::DELETED) {
            continue;
        }
        // Skip user-marked messages that already have a score
        if toc.msgs[i].spam_score != -1 && toc.msgs[i].spam_because == BECAUSE_USER {
            continue;
        }
        if score(cfg, toc, i, &read_headers, &read_body).is_some() {
            scored += 1;
        }
    }
    scored
}

// ── Mark as junk / not-junk ──

/// Set spam score and source directly.
pub fn set_score(toc: &mut Toc, index: usize, score: i8, because: u8) -> bool {
    if index >= toc.msgs.len() {
        return false;
    }
    toc.msgs[index].spam_score = score;
    toc.msgs[index].spam_because = because;
    toc.dirty = true;
    true
}

/// Mark a message as junk or not-junk.
/// Trains the scorer if available. For junk, optionally adjusts date.
/// Returns true on success.
pub fn mark(
    cfg: &JunkConfig,
    toc: &mut Toc,
    index: usize,
    is_junk: bool,
    read_headers: impl Fn(&Toc, usize) -> Option<String>,
    read_body: impl Fn(&Toc, usize) -> Option<String>,
) -> bool {
    if index >= toc.msgs.len() {
        return false;
    }

    if is_junk {
        // Train scorer if available
        if let Some(ref score_fn) = cfg.score_fn {
            let headers = read_headers(toc, index);
            let body = read_body(toc, index);
            let _ = score_fn(
                toc,
                index,
                headers.as_deref(),
                body.as_deref(),
                ScoreAction::UserJunk,
            );
        }

        // Set score high
        let s = if toc.msgs[index].spam_score > 0 {
            toc.msgs[index].spam_score
        } else {
            100
        };
        toc.msgs[index].spam_score = s;
        toc.msgs[index].spam_because = BECAUSE_USER;

        // Don't trust date on junk
        if !cfg.believe_date && toc.msgs[index].arrival != 0 {
            toc.msgs[index].seconds = toc.msgs[index].arrival;
        }
    } else {
        // Train as not-junk
        if let Some(ref score_fn) = cfg.score_fn {
            let headers = read_headers(toc, index);
            let body = read_body(toc, index);
            let _ = score_fn(
                toc,
                index,
                headers.as_deref(),
                body.as_deref(),
                ScoreAction::UserNotJunk,
            );
        }

        toc.msgs[index].spam_score = 0;
        toc.msgs[index].spam_because = BECAUSE_USER;
    }

    toc.dirty = true;
    true
}

// ── Move spam to Junk mailbox ──

/// Collect indices of messages above threshold (for moving to Junk).
/// Returns indices in reverse order (backwards to avoid index shift).
pub fn collect_spam_indices(cfg: &JunkConfig, toc: &Toc) -> Vec<usize> {
    let mut indices = Vec::new();
    for i in (0..toc.msgs.len()).rev() {
        let msg = &toc.msgs[i];
        if msg.flags.contains(MsgFlags::DELETED) {
            continue;
        }
        if msg.spam_score < cfg.threshold {
            continue;
        }
        indices.push(i);
    }
    indices
}

/// Adjust date for junk messages before transfer (if !believe_date).
pub fn prepare_for_junk_transfer(cfg: &JunkConfig, msg: &mut MsgSum) {
    if !cfg.believe_date && msg.arrival != 0 {
        msg.seconds = msg.arrival;
    }
}

/// Move messages with spam_score >= threshold from `toc` to the Junk mailbox.
///
/// Finds the Junk mailbox path via the store, opens its TOC, then transfers
/// each spam message (iterating backwards to avoid index shift).
/// Returns the number of messages moved, or an error.
pub fn move_spam(cfg: &JunkConfig, toc: &mut Toc, store: &Store) -> io::Result<usize> {
    // Find the Junk mailbox node in the store
    let junk_node = match store.find_special(MailboxType::Junk) {
        Some(node) => node,
        None => return Ok(0),
    };

    // Don't move from Junk to itself
    if junk_node.path == toc.mbox_path {
        return Ok(0);
    }

    let mut junk_toc = Toc::open_or_create(&junk_node.path)?;

    let mut moved = 0usize;
    for i in (0..toc.msgs.len()).rev() {
        let msg = &toc.msgs[i];
        if msg.flags.contains(MsgFlags::DELETED) {
            continue;
        }
        if msg.spam_score < cfg.threshold {
            continue;
        }

        // Don't trust date on junk
        if !cfg.believe_date && toc.msgs[i].arrival != 0 {
            toc.msgs[i].seconds = toc.msgs[i].arrival;
        }

        if ops::transfer(toc, i, &mut junk_toc, false).is_ok() {
            moved += 1;
        }
    }

    if moved > 0 {
        junk_toc.save()?;
    }
    Ok(moved)
}

// ── Archive old junk ──

fn now_secs() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

/// Collect indices of old junk messages to delete.
/// Returns indices of messages older than `archive_days` with score >= `archive_threshold`.
pub fn collect_archive_indices(cfg: &JunkConfig, toc: &Toc) -> Vec<usize> {
    if cfg.archive_days <= 0 {
        return Vec::new();
    }
    let now = now_secs();
    let thresh_time = now.saturating_sub((cfg.archive_days as u32) * 86400);
    let mut indices = Vec::new();

    for i in 0..toc.msgs.len() {
        let msg = &toc.msgs[i];
        if msg.flags.contains(MsgFlags::DELETED) {
            continue;
        }
        let msg_time = if msg.arrival != 0 {
            msg.arrival
        } else {
            msg.seconds
        };
        if msg_time < thresh_time && msg.spam_score >= cfg.archive_threshold {
            indices.push(i);
        }
    }
    indices
}

/// Mark messages at given indices as deleted.
/// Returns count deleted.
pub fn archive(cfg: &JunkConfig, toc: &mut Toc) -> usize {
    let indices = collect_archive_indices(cfg, toc);
    let count = indices.len();
    for i in indices {
        toc.msgs[i].flags.insert(MsgFlags::DELETED);
    }
    if count > 0 {
        toc.dirty = true;
    }
    count
}

// ── Utilities ──

/// Check if a TOC is the Junk mailbox.
pub fn is_junk_mailbox(toc: &Toc) -> bool {
    toc.which == MailboxType::Junk
}

/// Cleanse junk TOC on load: set transfer scores for unscored messages
/// that are in the Junk mailbox.
pub fn toc_cleanse(cfg: Option<&JunkConfig>, toc: &mut Toc) {
    let is_junk = toc.which == MailboxType::Junk;
    if !is_junk {
        return;
    }
    let default_score: i8 = cfg.map(|c| c.threshold).unwrap_or(50);

    for msg in toc.msgs.iter_mut() {
        if msg.spam_because == BECAUSE_NOT_JUNK {
            msg.spam_score = default_score;
            msg.spam_because = BECAUSE_XFER;
            toc.dirty = true;
        }
    }
}

// ================================================================
// Tests
// ================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toc::{MsgSum, Toc};

    fn make_toc_with_msgs(n: usize) -> Toc {
        let mut toc = Toc::new("/tmp/test_junk");
        for i in 0..n {
            let mut msg = MsgSum::default();
            msg.from = format!("sender{}@example.com", i);
            msg.subject = format!("Test message {}", i);
            msg.serial_num = (i + 1) as i64;
            msg.seconds = 1700000000 + (i as u32) * 100;
            msg.arrival = 1700000000 + (i as u32) * 100;
            toc.msgs.push(msg);
        }
        toc
    }

    fn no_headers(_toc: &Toc, _idx: usize) -> Option<String> {
        Some("From: test@example.com\nSubject: test\n".to_string())
    }

    fn no_body(_toc: &Toc, _idx: usize) -> Option<String> {
        Some("Hello world".to_string())
    }

    #[test]
    fn config_defaults() {
        let cfg = JunkConfig::new();
        assert_eq!(cfg.threshold, 50);
        assert_eq!(cfg.archive_days, 14);
        assert_eq!(cfg.archive_threshold, 50);
        assert!(!cfg.believe_date);
        assert!(!cfg.server_delete);
    }

    #[test]
    fn score_no_scorer_returns_none() {
        let cfg = JunkConfig::new();
        let mut toc = make_toc_with_msgs(1);
        let result = score(&cfg, &mut toc, 0, no_headers, no_body);
        assert!(result.is_none());
    }

    #[test]
    fn score_with_whitelist() {
        let mut cfg = JunkConfig::new();
        cfg.set_whitelist(Box::new(|from: &str| from.contains("sender0")));

        let mut toc = make_toc_with_msgs(2);
        let result = score(&cfg, &mut toc, 0, no_headers, no_body);
        assert_eq!(result, Some(0));
        assert_eq!(toc.msgs[0].spam_score, 0);
        assert_eq!(toc.msgs[0].spam_because, BECAUSE_WHITELIST);
    }

    #[test]
    fn score_with_scorer() {
        let mut cfg = JunkConfig::new();
        cfg.set_scorer(Box::new(
            |_toc: &Toc, _idx: usize, _h: Option<&str>, _b: Option<&str>, _a: ScoreAction| {
                Some(75i8)
            },
        ));

        let mut toc = make_toc_with_msgs(1);
        let result = score(&cfg, &mut toc, 0, no_headers, no_body);
        assert_eq!(result, Some(75));
        assert_eq!(toc.msgs[0].spam_score, 75);
        assert_eq!(toc.msgs[0].spam_because, BECAUSE_PLUG);
        assert!(toc.dirty);
    }

    #[test]
    fn score_clamps_to_100() {
        let mut cfg = JunkConfig::new();
        cfg.set_scorer(Box::new(
            |_toc: &Toc, _idx: usize, _h: Option<&str>, _b: Option<&str>, _a: ScoreAction| {
                Some(127i8) // over 100
            },
        ));

        let mut toc = make_toc_with_msgs(1);
        let result = score(&cfg, &mut toc, 0, no_headers, no_body);
        assert_eq!(result, Some(100));
    }

    #[test]
    fn score_invalid_index() {
        let cfg = JunkConfig::new();
        let mut toc = make_toc_with_msgs(1);
        assert!(score(&cfg, &mut toc, 5, no_headers, no_body).is_none());
    }

    #[test]
    fn score_box_skips_deleted_and_scored() {
        let mut cfg = JunkConfig::new();
        cfg.set_scorer(Box::new(
            |_toc: &Toc, _idx: usize, _h: Option<&str>, _b: Option<&str>, _a: ScoreAction| {
                Some(60i8)
            },
        ));

        let mut toc = make_toc_with_msgs(4);
        // Mark one deleted
        toc.msgs[1].flags.insert(MsgFlags::DELETED);
        // Mark one already scored
        toc.msgs[2].spam_score = 30;

        let count = score_box(&cfg, &mut toc, no_headers, no_body);
        assert_eq!(count, 2); // msg 0 and msg 3 scored
        assert_eq!(toc.msgs[0].spam_score, 60);
        assert_eq!(toc.msgs[1].spam_score, -1); // deleted, untouched
        assert_eq!(toc.msgs[2].spam_score, 30); // already scored, untouched
        assert_eq!(toc.msgs[3].spam_score, 60);
    }

    #[test]
    fn rescore_box_skips_user_marked() {
        let mut cfg = JunkConfig::new();
        cfg.set_scorer(Box::new(
            |_toc: &Toc, _idx: usize, _h: Option<&str>, _b: Option<&str>, _a: ScoreAction| {
                Some(80i8)
            },
        ));

        let mut toc = make_toc_with_msgs(3);
        // Mark one as user-scored
        toc.msgs[1].spam_score = 100;
        toc.msgs[1].spam_because = BECAUSE_USER;
        // Mark one as plugin-scored (should be rescored)
        toc.msgs[2].spam_score = 30;
        toc.msgs[2].spam_because = BECAUSE_PLUG;

        let count = rescore_box(&cfg, &mut toc, no_headers, no_body);
        assert_eq!(count, 2); // msgs 0 and 2 rescored
        assert_eq!(toc.msgs[0].spam_score, 80);
        assert_eq!(toc.msgs[1].spam_score, 100); // user-marked, untouched
        assert_eq!(toc.msgs[2].spam_score, 80); // plugin-scored, rescored
    }

    #[test]
    fn set_score_direct() {
        let mut toc = make_toc_with_msgs(1);
        assert!(set_score(&mut toc, 0, 99, BECAUSE_USER));
        assert_eq!(toc.msgs[0].spam_score, 99);
        assert_eq!(toc.msgs[0].spam_because, BECAUSE_USER);
        assert!(toc.dirty);
    }

    #[test]
    fn set_score_invalid_index() {
        let mut toc = make_toc_with_msgs(1);
        assert!(!set_score(&mut toc, 5, 99, BECAUSE_USER));
    }

    #[test]
    fn mark_as_junk() {
        let mut cfg = JunkConfig::new();
        let train_action = std::cell::Cell::new(ScoreAction::Auto);
        cfg.set_scorer(Box::new(
            move |_toc: &Toc, _idx: usize, _h: Option<&str>, _b: Option<&str>, a: ScoreAction| {
                train_action.set(a);
                Some(90i8)
            },
        ));

        let mut toc = make_toc_with_msgs(1);
        toc.msgs[0].arrival = 1700000000;
        toc.msgs[0].seconds = 1700050000;

        assert!(mark(&cfg, &mut toc, 0, true, no_headers, no_body));
        assert_eq!(toc.msgs[0].spam_score, 100); // was -1, so set to 100
        assert_eq!(toc.msgs[0].spam_because, BECAUSE_USER);
        // With believe_date=false, seconds should be set to arrival
        assert_eq!(toc.msgs[0].seconds, 1700000000);
    }

    #[test]
    fn mark_as_not_junk() {
        let cfg = JunkConfig::new();
        let mut toc = make_toc_with_msgs(1);
        toc.msgs[0].spam_score = 90;

        assert!(mark(&cfg, &mut toc, 0, false, no_headers, no_body));
        assert_eq!(toc.msgs[0].spam_score, 0);
        assert_eq!(toc.msgs[0].spam_because, BECAUSE_USER);
    }

    #[test]
    fn mark_preserves_high_score() {
        let cfg = JunkConfig::new();
        let mut toc = make_toc_with_msgs(1);
        toc.msgs[0].spam_score = 85;

        assert!(mark(&cfg, &mut toc, 0, true, no_headers, no_body));
        assert_eq!(toc.msgs[0].spam_score, 85); // preserved existing high score
    }

    #[test]
    fn collect_spam_indices_basic() {
        let cfg = JunkConfig::new(); // threshold 50
        let mut toc = make_toc_with_msgs(5);
        toc.msgs[0].spam_score = 10; // below threshold
        toc.msgs[1].spam_score = 60; // above
        toc.msgs[2].spam_score = 50; // at threshold
        toc.msgs[3].flags.insert(MsgFlags::DELETED); // deleted
        toc.msgs[3].spam_score = 90;
        toc.msgs[4].spam_score = 99; // above

        let indices = collect_spam_indices(&cfg, &toc);
        // Should contain 4, 2, 1 (reverse order), not 3 (deleted) or 0 (below)
        assert_eq!(indices, vec![4, 2, 1]);
    }

    #[test]
    fn prepare_for_junk_transfer_resets_date() {
        let cfg = JunkConfig::new(); // believe_date=false
        let mut msg = MsgSum::default();
        msg.arrival = 1700000000;
        msg.seconds = 1700050000;

        prepare_for_junk_transfer(&cfg, &mut msg);
        assert_eq!(msg.seconds, 1700000000);
    }

    #[test]
    fn prepare_for_junk_transfer_keeps_date_when_believe() {
        let mut cfg = JunkConfig::new();
        cfg.believe_date = true;
        let mut msg = MsgSum::default();
        msg.arrival = 1700000000;
        msg.seconds = 1700050000;

        prepare_for_junk_transfer(&cfg, &mut msg);
        assert_eq!(msg.seconds, 1700050000); // unchanged
    }

    #[test]
    fn archive_deletes_old_junk() {
        let mut cfg = JunkConfig::new();
        cfg.archive_days = 7;
        cfg.archive_threshold = 50;

        let mut toc = make_toc_with_msgs(4);
        let now = now_secs();
        let old_time = now - 8 * 86400; // 8 days ago
        let recent_time = now - 3 * 86400; // 3 days ago

        toc.msgs[0].arrival = old_time;
        toc.msgs[0].spam_score = 70; // old + above threshold -> archive
        toc.msgs[1].arrival = old_time;
        toc.msgs[1].spam_score = 30; // old + below threshold -> keep
        toc.msgs[2].arrival = recent_time;
        toc.msgs[2].spam_score = 90; // recent + above threshold -> keep
        toc.msgs[3].arrival = old_time;
        toc.msgs[3].spam_score = 90;
        toc.msgs[3].flags.insert(MsgFlags::DELETED); // already deleted

        let count = archive(&cfg, &mut toc);
        assert_eq!(count, 1); // only msg 0
        assert!(toc.msgs[0].flags.contains(MsgFlags::DELETED));
        assert!(!toc.msgs[1].flags.contains(MsgFlags::DELETED));
        assert!(!toc.msgs[2].flags.contains(MsgFlags::DELETED));
    }

    #[test]
    fn archive_zero_days_does_nothing() {
        let mut cfg = JunkConfig::new();
        cfg.archive_days = 0;

        let mut toc = make_toc_with_msgs(2);
        toc.msgs[0].spam_score = 90;
        toc.msgs[0].arrival = 1000; // very old

        assert_eq!(archive(&cfg, &mut toc), 0);
    }

    #[test]
    fn is_junk_mailbox_check() {
        let mut toc = Toc::new("/tmp/test_junk");
        assert!(!is_junk_mailbox(&toc));
        toc.which = MailboxType::Junk;
        assert!(is_junk_mailbox(&toc));
    }

    #[test]
    fn toc_cleanse_sets_xfer_scores() {
        let cfg = JunkConfig::new();
        let mut toc = Toc::new("/tmp/test_junk");
        toc.which = MailboxType::Junk;

        let mut msg1 = MsgSum::default();
        msg1.spam_because = BECAUSE_NOT_JUNK; // unscored in junk box
        msg1.spam_score = -1;

        let mut msg2 = MsgSum::default();
        msg2.spam_because = BECAUSE_PLUG; // already scored
        msg2.spam_score = 70;

        toc.msgs.push(msg1);
        toc.msgs.push(msg2);

        toc_cleanse(Some(&cfg), &mut toc);
        assert_eq!(toc.msgs[0].spam_score, 50); // set to threshold
        assert_eq!(toc.msgs[0].spam_because, BECAUSE_XFER);
        assert_eq!(toc.msgs[1].spam_score, 70); // unchanged
        assert_eq!(toc.msgs[1].spam_because, BECAUSE_PLUG);
    }

    #[test]
    fn toc_cleanse_noop_for_non_junk() {
        let cfg = JunkConfig::new();
        let mut toc = Toc::new("/tmp/test_normal");
        toc.which = MailboxType::In;

        let mut msg = MsgSum::default();
        msg.spam_because = BECAUSE_NOT_JUNK;
        msg.spam_score = -1;
        toc.msgs.push(msg);

        toc_cleanse(Some(&cfg), &mut toc);
        assert_eq!(toc.msgs[0].spam_score, -1); // unchanged
    }

    #[test]
    fn toc_cleanse_no_config() {
        let mut toc = Toc::new("/tmp/test_junk");
        toc.which = MailboxType::Junk;

        let mut msg = MsgSum::default();
        msg.spam_because = BECAUSE_NOT_JUNK;
        toc.msgs.push(msg);

        toc_cleanse(None, &mut toc);
        assert_eq!(toc.msgs[0].spam_score, 50); // default threshold
        assert_eq!(toc.msgs[0].spam_because, BECAUSE_XFER);
    }

    #[test]
    fn whitelist_overrides_scorer() {
        let mut cfg = JunkConfig::new();
        cfg.set_whitelist(Box::new(|_from: &str| true)); // everyone whitelisted
        cfg.set_scorer(Box::new(
            |_toc: &Toc, _idx: usize, _h: Option<&str>, _b: Option<&str>, _a: ScoreAction| {
                Some(100i8) // would score as max spam
            },
        ));

        let mut toc = make_toc_with_msgs(1);
        let result = score(&cfg, &mut toc, 0, no_headers, no_body);
        assert_eq!(result, Some(0)); // whitelist wins
        assert_eq!(toc.msgs[0].spam_because, BECAUSE_WHITELIST);
    }

    #[test]
    fn mark_invalid_index() {
        let cfg = JunkConfig::new();
        let mut toc = make_toc_with_msgs(1);
        assert!(!mark(&cfg, &mut toc, 5, true, no_headers, no_body));
    }

    #[test]
    fn collect_archive_uses_seconds_when_no_arrival() {
        let mut cfg = JunkConfig::new();
        cfg.archive_days = 7;
        cfg.archive_threshold = 50;

        let mut toc = make_toc_with_msgs(1);
        let now = now_secs();
        toc.msgs[0].arrival = 0; // no arrival time
        toc.msgs[0].seconds = now - 8 * 86400; // 8 days ago
        toc.msgs[0].spam_score = 60;

        let indices = collect_archive_indices(&cfg, &toc);
        assert_eq!(indices, vec![0]);
    }
}
