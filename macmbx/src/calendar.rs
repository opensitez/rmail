use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct CalendarEvent {
    pub uid: String,
    pub summary: String,
    pub description: String,
    pub location: String,
    pub dt_start_ts: i64,
    pub dt_end_ts: i64,
    pub start_day: i32,   // YYYYMMDD
    pub end_day: i32,     // YYYYMMDD
    pub all_day: bool,
    pub calendar_id: String,
    pub color: String,
    // Recurrence (stored in DB but only used by master events)
    pub rrule: String,          // "" | "DAILY" | "WEEKLY" | "MONTHLY" | "YEARLY"
    pub rrule_interval: i32,    // repeat every N units (>=1)
    pub rrule_until: i32,       // 0 = no limit, else YYYYMMDD
    pub rrule_count: i32,       // 0 = unlimited, else max occurrences
    pub rrule_byday: String,    // "MO,WE,FR" for weekly
    pub recur_parent_uid: String, // non-empty = virtual occurrence or exception
    pub attendees: String,      // "\n"-separated list
    pub status: String,         // "busy"|"free"|"tentative"
    pub url: String,
    pub is_private: bool,
    pub label: String,
    pub reminder_mins: i32,
    pub method: String,         // "REQUEST"|"REPLY"|"CANCEL"|""
    pub organizer: String,      // email
    pub organizer_name: String,
    pub recur_exdates: String,  // comma-separated YYYYMMDD
    pub tzid: String,           // timezone identifier, e.g. "America/New_York"
}

impl Default for CalendarEvent {
    fn default() -> Self {
        Self {
            uid: String::new(), summary: String::new(), description: String::new(),
            location: String::new(), dt_start_ts: 0, dt_end_ts: 0,
            start_day: 0, end_day: 0, all_day: false,
            calendar_id: "local".to_string(), color: "#4A90D9".to_string(),
            rrule: String::new(), rrule_interval: 1, rrule_until: 0,
            rrule_count: 0, rrule_byday: String::new(), recur_parent_uid: String::new(),
            attendees: String::new(), status: "busy".to_string(), url: String::new(),
            is_private: false, label: String::new(), reminder_mins: 0,
            method: String::new(), organizer: String::new(), organizer_name: String::new(),
            recur_exdates: String::new(), tzid: String::new(),
        }
    }
}

/// 0 = Local, 1 = CalDAV
#[derive(Clone, Debug, PartialEq)]
pub enum CalSourceType { Local = 0, CalDAV = 1 }

#[derive(Clone, Debug)]
pub struct CalendarInfo {
    pub id: String,
    pub name: String,
    pub color: String,
    pub enabled: bool,
    pub source: CalSourceType,
    // CalDAV-specific
    pub server_url: String,
    pub username: String,
    pub password: String,
    pub sync_token: String,
    pub last_sync_time: i64,
}

impl Default for CalendarInfo {
    fn default() -> Self {
        Self {
            id: String::new(), name: "Calendar".to_string(),
            color: "#4A90D9".to_string(), enabled: true,
            source: CalSourceType::Local,
            server_url: String::new(), username: String::new(),
            password: String::new(), sync_token: String::new(),
            last_sync_time: 0,
        }
    }
}

/// A reminder that has fired.
#[derive(Clone, Debug)]
pub struct CalendarReminder {
    pub uid: String,
    pub summary: String,
    pub dt_start_ts: i64,
    pub reminder_mins: i32,
}

pub struct CalendarStore {
    db_path: PathBuf,
}

impl CalendarStore {
    pub fn db_path(&self) -> &Path { &self.db_path }

    pub fn new(data_dir: &Path) -> Self {
        let db_path = data_dir.join("phoenix.db");
        if let Ok(conn) = Connection::open(&db_path) {
            let _ = conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS calendar_events (
                    uid              TEXT    PRIMARY KEY,
                    summary          TEXT    NOT NULL DEFAULT '',
                    description      TEXT    NOT NULL DEFAULT '',
                    location         TEXT    NOT NULL DEFAULT '',
                    dtstart_ts       INTEGER,
                    dtend_ts         INTEGER,
                    dtstart_day      INTEGER NOT NULL DEFAULT 0,
                    dtend_day        INTEGER NOT NULL DEFAULT 0,
                    all_day          INTEGER NOT NULL DEFAULT 0,
                    attendees        TEXT    NOT NULL DEFAULT '',
                    status           TEXT    NOT NULL DEFAULT 'busy',
                    url              TEXT    NOT NULL DEFAULT '',
                    private          INTEGER NOT NULL DEFAULT 0,
                    label            TEXT    NOT NULL DEFAULT '',
                    rrule            TEXT    NOT NULL DEFAULT '',
                    rrule_interval   INTEGER NOT NULL DEFAULT 1,
                    rrule_until      INTEGER NOT NULL DEFAULT 0,
                    rrule_count      INTEGER NOT NULL DEFAULT 0,
                    rrule_byday      TEXT    NOT NULL DEFAULT '',
                    recur_parent_uid TEXT    NOT NULL DEFAULT '',
                    recur_exdates    TEXT    NOT NULL DEFAULT '',
                    reminder_mins    INTEGER NOT NULL DEFAULT 0,
                    calendar_id      TEXT    NOT NULL DEFAULT 'local'
                );
                CREATE TABLE IF NOT EXISTS calendars (
                    id             TEXT    PRIMARY KEY,
                    name           TEXT    NOT NULL DEFAULT 'Calendar',
                    color          TEXT    NOT NULL DEFAULT '#4A90D9',
                    source         INTEGER NOT NULL DEFAULT 0,
                    enabled        INTEGER NOT NULL DEFAULT 1,
                    server_url     TEXT    NOT NULL DEFAULT '',
                    username       TEXT    NOT NULL DEFAULT '',
                    password       TEXT    NOT NULL DEFAULT '',
                    sync_token     TEXT    NOT NULL DEFAULT '',
                    last_sync_time INTEGER NOT NULL DEFAULT 0
                );
                INSERT OR IGNORE INTO calendars (id, name, color) VALUES ('local', 'Calendar', '#4A90D9');
                "
            );
        }
        Self { db_path }
    }

    pub fn get_calendars(&self) -> Vec<CalendarInfo> {
        let mut result = Vec::new();
        if let Ok(conn) = Connection::open(&self.db_path) {
            let sql = "SELECT id, name, color, enabled, source, server_url, username, password, sync_token, last_sync_time FROM calendars ORDER BY rowid ASC;";
            if let Ok(mut stmt) = conn.prepare(sql) {
                if let Ok(rows) = stmt.query_map([], |row| {
                    Ok(CalendarInfo {
                        id: row.get(0)?, name: row.get(1)?,
                        color: row.get(2)?, enabled: row.get::<_, i32>(3)? != 0,
                        source: if row.get::<_, i32>(4).unwrap_or(0) == 1 { CalSourceType::CalDAV } else { CalSourceType::Local },
                        server_url: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                        username: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                        password: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                        sync_token: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                        last_sync_time: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
                    })
                }) {
                    for row in rows.flatten() { result.push(row); }
                }
            }
        }
        result
    }

    /// Add or update a calendar record.
    pub fn upsert_calendar(&self, cal: &CalendarInfo) {
        if let Ok(conn) = Connection::open(&self.db_path) {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO calendars (id, name, color, enabled, source, server_url, username, password, sync_token, last_sync_time) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    &cal.id, &cal.name, &cal.color, if cal.enabled { 1 } else { 0 },
                    cal.source.clone() as i32,
                    &cal.server_url, &cal.username, &cal.password,
                    &cal.sync_token, cal.last_sync_time,
                ],
            );
        }
    }

    /// Update just the sync token and last-sync-time for a calendar.
    pub fn update_sync_token(&self, calendar_id: &str, token: &str, time: i64) {
        if let Ok(conn) = Connection::open(&self.db_path) {
            let _ = conn.execute(
                "UPDATE calendars SET sync_token=?2, last_sync_time=?3 WHERE id=?1",
                rusqlite::params![calendar_id, token, time],
            );
        }
    }

    /// Check for reminders that should fire now.
    /// Returns events whose reminder time is within [now-90s, now].
    /// `fired_keys` is a set of already-fired keys to avoid duplicates.
    pub fn check_reminders(&self, fired_keys: &mut std::collections::HashSet<String>) -> Vec<CalendarReminder> {
        let mut reminders = Vec::new();
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // Scan today and tomorrow
        let (y, m, d, _, _) = split_ts(now_ts);
        let today_int = y * 10000 + m * 100 + d;
        let tomorrow_int = {
            let ts = make_local_ts(y, m, d, 12, 0) + 86400;
            let (ny, nm, nd, _, _) = split_ts(ts);
            ny * 10000 + nm * 100 + nd
        };

        for date_int in [today_int, tomorrow_int] {
            // Query events with reminder_mins > 0
            if let Ok(conn) = Connection::open(&self.db_path) {
                let sql = "SELECT uid, summary, dtstart_ts, reminder_mins FROM calendar_events \
                           WHERE dtstart_day = ?1 AND reminder_mins > 0 AND rrule = '' AND recur_parent_uid = ''";
                if let Ok(mut stmt) = conn.prepare(sql) {
                    if let Ok(rows) = stmt.query_map([date_int], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i32>(3)?,
                        ))
                    }) {
                        for row in rows.flatten() {
                            let (uid, summary, dt_start_ts, reminder_mins) = row;
                            let trigger_ts = dt_start_ts - (reminder_mins as i64 * 60);
                            let delta = now_ts - trigger_ts;

                            // Fire if within 0..90s past trigger
                            if delta < 0 || delta > 90 { continue; }

                            let key = format!("{}|{}|{}", uid, dt_start_ts, reminder_mins);
                            if fired_keys.contains(&key) { continue; }
                            fired_keys.insert(key.clone());

                            reminders.push(CalendarReminder {
                                uid, summary, dt_start_ts, reminder_mins,
                            });
                        }
                    }
                }
            }
        }
        reminders
    }

    /// Get all events for a date, including expanded recurring occurrences.
    /// Port of CalendarEventStore::GetEventsForDate.
    /// Only returns events from enabled calendars.
    pub fn get_events_for_date(&self, date_int: i32) -> Vec<CalendarEvent> {
        let mut result = Vec::new();
        let conn = match Connection::open(&self.db_path) {
            Ok(c) => c,
            Err(_) => return result,
        };

        // 1. Non-recurring events that directly span this date (enabled calendars only)
        let sql_direct = "\
            SELECT e.uid, e.summary, e.description, e.location, \
                   e.dtstart_ts, e.dtend_ts, e.dtstart_day, e.dtend_day, e.all_day, \
                   e.calendar_id, c.color, \
                   e.rrule, e.rrule_interval, e.rrule_until, e.rrule_count, e.rrule_byday, e.recur_parent_uid, \
                   e.attendees, e.status, e.url, e.private, e.label, e.reminder_mins, e.recur_exdates \
            FROM calendar_events e \
            LEFT JOIN calendars c ON e.calendar_id = c.id \
            WHERE e.dtstart_day <= ?1 AND e.dtend_day >= ?1 \
              AND e.rrule = '' AND e.recur_parent_uid = '' \
              AND COALESCE(c.enabled, 1) = 1 \
            ORDER BY e.all_day DESC, e.dtstart_ts ASC;";

        if let Ok(mut stmt) = conn.prepare(sql_direct) {
            if let Ok(rows) = stmt.query_map([date_int], |row| row_to_event(row)) {
                for row in rows.flatten() { result.push(row); }
            }
        }

        // 2. Recurrence exception overrides for this date
        let mut overridden_uids = std::collections::HashSet::new();
        {
            let sql_exceptions = "\
                SELECT e.uid, e.summary, e.description, e.location, \
                       e.dtstart_ts, e.dtend_ts, e.dtstart_day, e.dtend_day, e.all_day, \
                       e.calendar_id, c.color, \
                       e.rrule, e.rrule_interval, e.rrule_until, e.rrule_count, e.rrule_byday, e.recur_parent_uid, \
                       e.attendees, e.status, e.url, e.private, e.label, e.reminder_mins, e.recur_exdates \
                FROM calendar_events e \
                LEFT JOIN calendars c ON e.calendar_id = c.id \
                WHERE e.dtstart_day = ?1 AND e.recur_parent_uid != '' \
                  AND COALESCE(c.enabled, 1) = 1;";
            if let Ok(mut stmt) = conn.prepare(sql_exceptions) {
                if let Ok(rows) = stmt.query_map([date_int], |row| row_to_event(row)) {
                    for exc in rows.flatten() {
                        overridden_uids.insert(exc.recur_parent_uid.clone());
                        result.push(exc);
                    }
                }
            }
        }

        // 3. Recurring master events — expand virtual occurrences (enabled calendars only)
        let sql_master = "\
            SELECT e.uid, e.summary, e.description, e.location, \
                   e.dtstart_ts, e.dtend_ts, e.dtstart_day, e.dtend_day, e.all_day, \
                   e.calendar_id, c.color, \
                   e.rrule, e.rrule_interval, e.rrule_until, e.rrule_count, e.rrule_byday, e.recur_parent_uid, \
                   e.attendees, e.status, e.url, e.private, e.label, e.reminder_mins, e.recur_exdates \
            FROM calendar_events e \
            LEFT JOIN calendars c ON e.calendar_id = c.id \
            WHERE e.rrule != '' AND e.recur_parent_uid = '' \
              AND COALESCE(c.enabled, 1) = 1;";

        if let Ok(mut stmt) = conn.prepare(sql_master) {
            if let Ok(rows) = stmt.query_map([], |row| row_to_event(row)) {
                for master in rows.flatten() {
                    // Skip if overridden by an exception for this date
                    if overridden_uids.contains(&master.uid) { continue; }
                    // Skip if this date is in recur_exdates
                    if !master.recur_exdates.is_empty() {
                        let date_str = format!("{}", date_int);
                        let is_excluded = master.recur_exdates.split(',')
                            .any(|d| d.trim() == date_str);
                        if is_excluded { continue; }
                    }
                    if !recur_hits_date(&master, date_int) { continue; }
                    // Build virtual occurrence: shift to queried date, preserve time-of-day + duration
                    let mut occ = master.clone();
                    let duration = occ.dt_end_ts - occ.dt_start_ts;
                    let (y, m, d) = split_ymd(date_int);
                    let (_, _, _, sh, sm) = split_ts(occ.dt_start_ts);
                    // Reconstruct start timestamp at the queried date with original time
                    occ.dt_start_ts = make_local_ts(y, m, d, sh, sm);
                    occ.dt_end_ts = occ.dt_start_ts + duration;
                    occ.start_day = date_int;
                    occ.end_day = date_int;
                    occ.recur_parent_uid = master.uid.clone();
                    result.push(occ);
                }
            }
        }

        // Sort: all-day first, then by start time
        result.sort_by(|a, b| {
            b.all_day.cmp(&a.all_day).then(a.dt_start_ts.cmp(&b.dt_start_ts))
        });
        result
    }

    pub fn add_event(&self, ev: &CalendarEvent) {
        if let Ok(conn) = Connection::open(&self.db_path) {
            let sql = "\
                INSERT OR REPLACE INTO calendar_events \
                (uid, summary, description, location, dtstart_ts, dtend_ts, \
                 dtstart_day, dtend_day, all_day, calendar_id, \
                 rrule, rrule_interval, rrule_until, rrule_count, rrule_byday, recur_parent_uid, \
                 attendees, status, url, private, label, reminder_mins, recur_exdates) \
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
                        ?17, ?18, ?19, ?20, ?21, ?22, ?23)";
            let _ = conn.execute(sql, rusqlite::params![
                &ev.uid, &ev.summary, &ev.description, &ev.location,
                ev.dt_start_ts, ev.dt_end_ts, ev.start_day, ev.end_day,
                if ev.all_day { 1 } else { 0 }, &ev.calendar_id,
                &ev.rrule, ev.rrule_interval, ev.rrule_until, ev.rrule_count,
                &ev.rrule_byday, &ev.recur_parent_uid,
                &ev.attendees, &ev.status, &ev.url, if ev.is_private { 1 } else { 0 },
                &ev.label, ev.reminder_mins, &ev.recur_exdates,
            ]);
        }
    }

    pub fn update_event(&self, ev: &CalendarEvent) {
        if let Ok(conn) = Connection::open(&self.db_path) {
            let sql = "\
                UPDATE calendar_events SET \
                summary=?2, description=?3, location=?4, \
                dtstart_ts=?5, dtend_ts=?6, dtstart_day=?7, dtend_day=?8, \
                all_day=?9, calendar_id=?10, \
                rrule=?11, rrule_interval=?12, rrule_until=?13, rrule_count=?14, rrule_byday=?15, \
                attendees=?16, status=?17, url=?18, private=?19, label=?20, reminder_mins=?21, recur_exdates=?22 \
                WHERE uid=?1";
            let _ = conn.execute(sql, rusqlite::params![
                &ev.uid, &ev.summary, &ev.description, &ev.location,
                ev.dt_start_ts, ev.dt_end_ts, ev.start_day, ev.end_day,
                if ev.all_day { 1 } else { 0 }, &ev.calendar_id,
                &ev.rrule, ev.rrule_interval, ev.rrule_until, ev.rrule_count, &ev.rrule_byday,
                &ev.attendees, &ev.status, &ev.url, if ev.is_private { 1 } else { 0 },
                &ev.label, ev.reminder_mins, &ev.recur_exdates,
            ]);
        }
    }

    /// Toggle a calendar's enabled state.
    pub fn toggle_calendar_enabled(&self, calendar_id: &str, enabled: bool) {
        if let Ok(conn) = Connection::open(&self.db_path) {
            let _ = conn.execute(
                "UPDATE calendars SET enabled=?2 WHERE id=?1",
                rusqlite::params![calendar_id, if enabled { 1 } else { 0 }],
            );
        }
    }

    /// Rename a calendar.
    pub fn rename_calendar(&self, calendar_id: &str, new_name: &str) {
        if let Ok(conn) = Connection::open(&self.db_path) {
            let _ = conn.execute(
                "UPDATE calendars SET name = ?2 WHERE id = ?1",
                rusqlite::params![calendar_id, new_name],
            );
        }
    }

    /// Delete a calendar and all its events.
    pub fn delete_calendar(&self, calendar_id: &str) {
        if let Ok(conn) = Connection::open(&self.db_path) {
            let _ = conn.execute("DELETE FROM calendar_events WHERE calendar_id = ?1", [calendar_id]);
            let _ = conn.execute("DELETE FROM calendars WHERE id = ?1", [calendar_id]);
        }
    }

    pub fn delete_event(&self, uid: &str) {
        if let Ok(conn) = Connection::open(&self.db_path) {
            let _ = conn.execute("DELETE FROM calendar_events WHERE uid = ?1", [uid]);
        }
    }

    pub fn get_event_by_uid(&self, uid: &str) -> Option<CalendarEvent> {
        let conn = Connection::open(&self.db_path).ok()?;
        let sql = "\
            SELECT e.uid, e.summary, e.description, e.location, \
                   e.dtstart_ts, e.dtend_ts, e.dtstart_day, e.dtend_day, e.all_day, \
                   e.calendar_id, c.color, \
                   e.rrule, e.rrule_interval, e.rrule_until, e.rrule_count, e.rrule_byday, e.recur_parent_uid, \
                   e.attendees, e.status, e.url, e.private, e.label, e.reminder_mins, e.recur_exdates \
            FROM calendar_events e \
            LEFT JOIN calendars c ON e.calendar_id = c.id \
            WHERE e.uid = ?1";
        conn.query_row(sql, [uid], |row| row_to_event(row))
            .optional().unwrap_or(None)
    }
}

// ── Row mapping ────────────────────────────────────────────────────────

fn row_to_event(row: &rusqlite::Row) -> rusqlite::Result<CalendarEvent> {
    Ok(CalendarEvent {
        uid: row.get(0)?,
        summary: row.get(1)?,
        description: row.get(2)?,
        location: row.get(3)?,
        dt_start_ts: row.get(4)?,
        dt_end_ts: row.get(5)?,
        start_day: row.get(6)?,
        end_day: row.get(7)?,
        all_day: row.get::<_, i32>(8)? != 0,
        calendar_id: row.get(9)?,
        color: row.get::<_, Option<String>>(10)?.unwrap_or_else(|| "#4A90D9".to_string()),
        rrule: row.get::<_, Option<String>>(11)?.unwrap_or_default(),
        rrule_interval: row.get::<_, Option<i32>>(12)?.unwrap_or(1),
        rrule_until: row.get::<_, Option<i32>>(13)?.unwrap_or(0),
        rrule_count: row.get::<_, Option<i32>>(14)?.unwrap_or(0),
        rrule_byday: row.get::<_, Option<String>>(15)?.unwrap_or_default(),
        recur_parent_uid: row.get::<_, Option<String>>(16)?.unwrap_or_default(),
        attendees: row.get::<_, Option<String>>(17)?.unwrap_or_default(),
        status: row.get::<_, Option<String>>(18)?.unwrap_or_else(|| "busy".to_string()),
        url: row.get::<_, Option<String>>(19)?.unwrap_or_default(),
        is_private: row.get::<_, Option<i32>>(20)?.unwrap_or(0) != 0,
        label: row.get::<_, Option<String>>(21)?.unwrap_or_default(),
        reminder_mins: row.get::<_, Option<i32>>(22)?.unwrap_or(0),
        recur_exdates: row.get::<_, Option<String>>(23)?.unwrap_or_default(),
        // Fields not stored in DB
        method: String::new(),
        organizer: String::new(),
        organizer_name: String::new(),
        tzid: String::new(),
    })
}

// ── Recurrence expansion (port of CalendarEventStore.cpp RecurHitsDate) ──

fn split_ymd(ymd: i32) -> (i32, i32, i32) {
    (ymd / 10000, (ymd / 100) % 100, ymd % 100)
}

/// Approximate local timestamp → (y, m, d, h, min).
/// Uses libc localtime for correctness.
pub fn split_ts(ts: i64) -> (i32, i32, i32, i32, i32) {
    let t = ts as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm); }
    (tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
}

/// Build a local Unix timestamp from components.
pub fn make_local_ts(y: i32, m: i32, d: i32, h: i32, min: i32) -> i64 {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = y - 1900;
    tm.tm_mon = m - 1;
    tm.tm_mday = d;
    tm.tm_hour = h;
    tm.tm_min = min;
    tm.tm_isdst = -1;
    unsafe { libc::mktime(&mut tm) as i64 }
}

/// Day-of-week for a YYYYMMDD int. 0=Sun, 1=Mon, ..., 6=Sat.
fn day_of_week(ymd: i32) -> i32 {
    let (y, m, d) = split_ymd(ymd);
    let ts = make_local_ts(y, m, d, 12, 0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = ts as libc::time_t;
    unsafe { libc::localtime_r(&t, &mut tm); }
    tm.tm_wday
}

/// Days between two YYYYMMDD ints (approximate via timestamps).
fn days_between(a: i32, b: i32) -> i64 {
    let (ay, am, ad) = split_ymd(a);
    let (by, bm, bd) = split_ymd(b);
    let ta = make_local_ts(ay, am, ad, 12, 0);
    let tb = make_local_ts(by, bm, bd, 12, 0);
    (tb - ta) / 86400
}

/// ISO Monday-anchored offset: Mon=0, Tue=1, ..., Sat=5, Sun=6.
fn iso_offset(wx_dow: i32) -> i32 {
    if wx_dow == 0 { 6 } else { wx_dow - 1 }
}

/// YYYYMMDD + N days → new YYYYMMDD.
fn add_days_ymd(ymd: i32, n: i64) -> i32 {
    let (y, m, d) = split_ymd(ymd);
    let ts = make_local_ts(y, m, d, 12, 0) + n * 86400;
    let (ny, nm, nd, _, _) = split_ts(ts);
    ny * 10000 + nm * 100 + nd
}

const DAY_CODES: &[&str] = &["SU", "MO", "TU", "WE", "TH", "FR", "SA"];

/// Does a recurring master event hit a particular date?
/// Port of C++ RecurHitsDate.
fn recur_hits_date(m: &CalendarEvent, date_int: i32) -> bool {
    if m.rrule.is_empty() { return false; }
    let s_day = m.start_day;
    if date_int < s_day { return false; }

    // Check until
    let passes_until = || -> bool {
        if m.rrule_until <= 0 { return true; }
        date_int <= m.rrule_until
    };

    let interval = m.rrule_interval.max(1);

    match m.rrule.as_str() {
        "DAILY" => {
            let days = days_between(s_day, date_int);
            if days < 0 || days % interval as i64 != 0 { return false; }
            if !passes_until() { return false; }
            if m.rrule_count > 0 && (days / interval as i64) >= m.rrule_count as i64 { return false; }
            true
        }
        "WEEKLY" => {
            let dow = day_of_week(date_int);

            // Parse BYDAY or default to start day's DOW
            let by_days: Vec<i32> = if !m.rrule_byday.is_empty() {
                m.rrule_byday.split(',').filter_map(|code| {
                    let code = code.trim().to_uppercase();
                    DAY_CODES.iter().position(|&c| c == code).map(|i| i as i32)
                }).collect()
            } else {
                vec![day_of_week(s_day)]
            };

            if !by_days.contains(&dow) { return false; }

            // Compute weeks between ISO-Monday-anchored week starts
            let start_mon = add_days_ymd(s_day, -(iso_offset(day_of_week(s_day)) as i64));
            let date_mon = add_days_ymd(date_int, -(iso_offset(dow) as i64));
            let weeks = days_between(start_mon, date_mon) / 7;
            if weeks < 0 || weeks % interval as i64 != 0 { return false; }
            if !passes_until() { return false; }

            if m.rrule_count > 0 {
                // Count occurrences up to this date
                let mut occ_count = 0;
                let mut w = 0i64;
                while w <= weeks {
                    let wk_mon = add_days_ymd(start_mon, w * 7);
                    for &di in &by_days {
                        let occ = add_days_ymd(wk_mon, iso_offset(di) as i64);
                        if occ < s_day { continue; }
                        if occ > date_int { break; }
                        occ_count += 1;
                    }
                    w += interval as i64;
                }
                if occ_count > m.rrule_count { return false; }
            }
            true
        }
        "MONTHLY" => {
            let (sy, sm, sd) = split_ymd(s_day);
            let (dy, dm, dd) = split_ymd(date_int);
            if dd != sd { return false; }
            let months = (dy - sy) * 12 + (dm - sm);
            if months < 0 || months % interval != 0 { return false; }
            if !passes_until() { return false; }
            if m.rrule_count > 0 && (months / interval + 1) > m.rrule_count { return false; }
            true
        }
        "YEARLY" => {
            let (sy, sm, sd) = split_ymd(s_day);
            let (dy, dm, dd) = split_ymd(date_int);
            if dd != sd || dm != sm { return false; }
            let years = dy - sy;
            if years < 0 || years % interval != 0 { return false; }
            if !passes_until() { return false; }
            if m.rrule_count > 0 && (years / interval + 1) > m.rrule_count { return false; }
            true
        }
        _ => false,
    }
}
