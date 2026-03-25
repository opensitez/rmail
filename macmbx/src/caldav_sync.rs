//! CalDAV synchronization — ties crispy::caldav with CalendarStore.
//!
//! Port of CalDAVEngine::FullSync + CalDAVSyncThread.

use crate::calendar::{CalendarStore, CalendarInfo, CalSourceType};
use crate::ics;
use crispy::caldav::CalDAVEngine;
use crispy::transport::Transport;
use std::collections::HashSet;

/// Run a full bidirectional sync for one CalDAV calendar.
///
/// 1. Fetches all events from the server via REPORT.
/// 2. Parses each ICS and upserts locally.
/// 3. Pushes locally-created events not yet on server.
/// 4. Updates sync token timestamp.
///
/// Returns (imported, pushed) counts, or error.
pub fn full_sync(
    tp: &mut dyn Transport,
    store: &CalendarStore,
    cal: &CalendarInfo,
    log: &mut dyn FnMut(&str),
) -> Result<(usize, usize), String> {
    log(&format!("Syncing \"{}\" …", cal.name));

    let mut eng = CalDAVEngine::new(tp);

    // 1. Fetch all events from server
    let server_ics = eng.fetch_events(&cal.server_url, &cal.username, &cal.password)
        .map_err(|e| format!("fetch_events: {}", e))?;

    // 2. Parse and upsert
    let mut imported = 0;
    let mut server_uids = HashSet::new();
    for ics_str in &server_ics {
        if let Some(mut ev) = ics::ics_to_event(ics_str) {
            ev.calendar_id = cal.id.clone();
            server_uids.insert(ev.uid.clone());
            store.add_event(&ev);
            imported += 1;
        }
    }
    log(&format!("Imported/updated {} event(s) from server", imported));

    // 3. Push local events not on server
    // Scan ±5 years of events for this calendar
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let (cy, cm, cd, _, _) = crate::calendar::split_ts(now_ts);

    let mut pushed = 0;
    for year_off in -5..=5 {
        let y = cy + year_off;
        // Check Jan 1 of that year as a representative date
        let date_int = y * 10000 + 1 * 100 + 1;
        // Actually we need to scan every day — too expensive.
        // Instead, query the DB directly for this calendar's events.
        // A simpler approach: query all events for this calendar_id.
    }

    // Direct DB query for all non-recurring events belonging to this calendar
    if let Ok(conn) = rusqlite::Connection::open(store.db_path()) {
        let sql = "SELECT uid, summary, description, location, dtstart_ts, dtend_ts, \
                   dtstart_day, dtend_day, all_day, rrule, rrule_interval, rrule_until, \
                   rrule_count, rrule_byday, recur_parent_uid \
                   FROM calendar_events WHERE calendar_id = ?1 AND recur_parent_uid = ''";
        if let Ok(mut stmt) = conn.prepare(sql) {
            if let Ok(rows) = stmt.query_map([&cal.id], |row| {
                Ok(crate::calendar::CalendarEvent {
                    uid: row.get(0)?,
                    summary: row.get(1)?,
                    description: row.get(2)?,
                    location: row.get(3)?,
                    dt_start_ts: row.get(4)?,
                    dt_end_ts: row.get(5)?,
                    start_day: row.get(6)?,
                    end_day: row.get(7)?,
                    all_day: row.get::<_, i32>(8)? != 0,
                    calendar_id: cal.id.clone(),
                    color: cal.color.clone(),
                    rrule: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
                    rrule_interval: row.get::<_, Option<i32>>(10)?.unwrap_or(1),
                    rrule_until: row.get::<_, Option<i32>>(11)?.unwrap_or(0),
                    rrule_count: row.get::<_, Option<i32>>(12)?.unwrap_or(0),
                    rrule_byday: row.get::<_, Option<String>>(13)?.unwrap_or_default(),
                    recur_parent_uid: row.get::<_, Option<String>>(14)?.unwrap_or_default(),
                    ..Default::default()
                })
            }) {
                // Reconnect transport for PUT operations
                let mut eng2 = CalDAVEngine::new(tp);
                for ev in rows.flatten() {
                    if !ev.recur_parent_uid.is_empty() { continue; }
                    if server_uids.contains(&ev.uid) { continue; }

                    let ics_data = ics::event_to_ics(&ev);
                    match eng2.put_event(&cal.server_url, &ev.uid, &ics_data,
                                         &cal.username, &cal.password) {
                        Ok(_) => { pushed += 1; }
                        Err(e) => { log(&format!("PUT failed for {}: {}", ev.uid, e)); }
                    }
                }
            }
        }
    }

    if pushed > 0 {
        log(&format!("Pushed {} local event(s) to server", pushed));
    }

    // 4. Update sync token
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    store.update_sync_token(&cal.id, "", now);

    Ok((imported, pushed))
}

/// Run sync for all enabled CalDAV calendars.
pub fn sync_all(
    store: &CalendarStore,
    tp_factory: &mut dyn FnMut() -> Box<dyn Transport>,
    log: &mut dyn FnMut(&str),
) -> bool {
    let cals = store.get_calendars();
    let mut any_error = false;

    for cal in &cals {
        if cal.source != CalSourceType::CalDAV || !cal.enabled { continue; }

        let mut tp = tp_factory();
        match full_sync(tp.as_mut(), store, cal, log) {
            Ok((imported, pushed)) => {
                log(&format!("Sync done: \"{}\" ({} imported, {} pushed)", cal.name, imported, pushed));
            }
            Err(e) => {
                log(&format!("Sync error for \"{}\": {}", cal.name, e));
                any_error = true;
            }
        }
    }

    !any_error
}
