//! iCalendar (.ics) serialization and parsing.
//!
//! Port of IcsUtils.cpp. Handles VCALENDAR/VEVENT generation and parsing
//! with RRULE, VALARM, ATTENDEE, and ORGANIZER support.

use crate::calendar::CalendarEvent;

// ── Serialization ──────────────────────────────────────────────────────

/// Generate a complete VCALENDAR/VEVENT string from an event.
pub fn event_to_ics(ev: &CalendarEvent) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("BEGIN:VCALENDAR\r\n");
    out.push_str("VERSION:2.0\r\n");
    out.push_str("PRODID:-//Eudora Phoenix//CalendarEvent//EN\r\n");
    out.push_str("CALSCALE:GREGORIAN\r\n");
    if !ev.method.is_empty() {
        fold_line(&mut out, &format!("METHOD:{}", ev.method));
    } else {
        out.push_str("METHOD:REQUEST\r\n");
    }
    out.push_str("BEGIN:VEVENT\r\n");

    let uid = if ev.uid.is_empty() {
        format!("eudora-{}@phoenix.local", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs())
    } else {
        ev.uid.clone()
    };
    fold_line(&mut out, &format!("UID:{}", uid));

    // DTSTAMP in UTC
    let now = now_utc_ics();
    fold_line(&mut out, &format!("DTSTAMP:{}", now));

    // DTSTART / DTEND
    if ev.all_day {
        fold_line(&mut out, &format!("DTSTART;VALUE=DATE:{}", fmt_date_ymd(ev.start_day)));
        fold_line(&mut out, &format!("DTEND;VALUE=DATE:{}", fmt_date_ymd(ev.end_day)));
    } else if !ev.tzid.is_empty() {
        fold_line(&mut out, &format!("DTSTART;TZID={}:{}", ev.tzid, fmt_datetime_ts(ev.dt_start_ts)));
        fold_line(&mut out, &format!("DTEND;TZID={}:{}", ev.tzid, fmt_datetime_ts(ev.dt_end_ts)));
    } else {
        fold_line(&mut out, &format!("DTSTART:{}", fmt_datetime_ts(ev.dt_start_ts)));
        fold_line(&mut out, &format!("DTEND:{}", fmt_datetime_ts(ev.dt_end_ts)));
    }

    fold_line(&mut out, &format!("SUMMARY:{}", escape_ics(&ev.summary)));
    if !ev.location.is_empty() {
        fold_line(&mut out, &format!("LOCATION:{}", escape_ics(&ev.location)));
    }
    if !ev.description.is_empty() {
        fold_line(&mut out, &format!("DESCRIPTION:{}", escape_ics(&ev.description)));
    }

    // ORGANIZER
    if !ev.organizer.is_empty() {
        if !ev.organizer_name.is_empty() {
            fold_line(&mut out, &format!("ORGANIZER;CN={}:mailto:{}", escape_ics(&ev.organizer_name), ev.organizer));
        } else {
            fold_line(&mut out, &format!("ORGANIZER:mailto:{}", ev.organizer));
        }
    }

    // ATTENDEES
    if !ev.attendees.is_empty() {
        for att in ev.attendees.split('\n') {
            let att = att.trim();
            if !att.is_empty() {
                fold_line(&mut out, &format!("ATTENDEE;RSVP=TRUE:mailto:{}", att));
            }
        }
    }

    // STATUS
    {
        let ics_status = match ev.status.as_str() {
            "free" => "TRANSPARENT",
            "tentative" => "TENTATIVE",
            _ => "", // busy is default, no need to emit
        };
        if !ics_status.is_empty() {
            fold_line(&mut out, &format!("STATUS:{}", ics_status));
        }
    }

    // EXDATE
    if !ev.recur_exdates.is_empty() {
        fold_line(&mut out, &format!("EXDATE:{}", ev.recur_exdates));
    }

    // RRULE
    if !ev.rrule.is_empty() {
        let mut rrule = format!("RRULE:FREQ={}", ev.rrule);
        if ev.rrule_interval > 1 {
            rrule.push_str(&format!(";INTERVAL={}", ev.rrule_interval));
        }
        if !ev.rrule_byday.is_empty() {
            rrule.push_str(&format!(";BYDAY={}", ev.rrule_byday));
        }
        if ev.rrule_count > 0 {
            rrule.push_str(&format!(";COUNT={}", ev.rrule_count));
        } else if ev.rrule_until > 0 {
            let y = ev.rrule_until / 10000;
            let m = (ev.rrule_until / 100) % 100;
            let d = ev.rrule_until % 100;
            rrule.push_str(&format!(";UNTIL={:04}{:02}{:02}T000000Z", y, m, d));
        }
        fold_line(&mut out, &rrule);
    }

    // VALARM (reminder)
    if ev.reminder_mins > 0 {
        out.push_str("BEGIN:VALARM\r\n");
        out.push_str("ACTION:DISPLAY\r\n");
        fold_line(&mut out, &format!("DESCRIPTION:Reminder: {}", escape_ics(&ev.summary)));
        if ev.reminder_mins >= 60 && ev.reminder_mins % 60 == 0 {
            fold_line(&mut out, &format!("TRIGGER:-PT{}H", ev.reminder_mins / 60));
        } else {
            fold_line(&mut out, &format!("TRIGGER:-PT{}M", ev.reminder_mins));
        }
        out.push_str("END:VALARM\r\n");
    }

    out.push_str("END:VEVENT\r\n");
    out.push_str("END:VCALENDAR\r\n");
    out
}

// ── Parsing ────────────────────────────────────────────────────────────

/// Parse a VCALENDAR string into a CalendarEvent. Returns None on failure.
pub fn ics_to_event(ics_raw: &str) -> Option<CalendarEvent> {
    let ics = unfold(ics_raw);
    let ics = ics.replace("\r\n", "\n").replace('\r', "\n");

    let mut in_vevent = false;
    let mut in_valarm = false;
    let mut found = false;
    let mut ev = CalendarEvent::default();
    let mut method = String::new();

    // Parse METHOD from the top-level VCALENDAR
    for line in ics.lines() {
        let line = line.trim();
        if line.to_uppercase().starts_with("METHOD:") {
            method = line[7..].trim().to_uppercase();
            break;
        }
        if line.eq_ignore_ascii_case("BEGIN:VEVENT") { break; }
    }

    for line in ics.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }

        if line.eq_ignore_ascii_case("BEGIN:VEVENT") {
            in_vevent = true; in_valarm = false; found = true;
            ev = CalendarEvent::default();
            ev.method = method.clone();
            continue;
        }
        if line.eq_ignore_ascii_case("END:VEVENT") {
            in_vevent = false;
            break; // take first VEVENT only
        }
        if line.eq_ignore_ascii_case("BEGIN:VALARM") { in_valarm = true; continue; }
        if line.eq_ignore_ascii_case("END:VALARM") { in_valarm = false; continue; }

        if !in_vevent { continue; }

        let colon = match line.find(':') {
            Some(p) => p,
            None => continue,
        };
        let key = &line[..colon];
        let value = &line[colon + 1..];
        let base_key = key.split(';').next().unwrap_or("").to_uppercase();

        if in_valarm {
            // Parse TRIGGER for reminder
            if base_key == "TRIGGER" {
                ev = parse_trigger(ev, value);
            }
            continue;
        }

        match base_key.as_str() {
            "UID" => ev.uid = unescape_ics(value),
            "SUMMARY" => ev.summary = unescape_ics(value),
            "LOCATION" => ev.location = unescape_ics(value),
            "DESCRIPTION" => ev.description = unescape_ics(value),
            "DTSTART" => {
                // Check for TZID parameter
                if let Some(tzid) = extract_param(key, "TZID") {
                    ev.tzid = tzid;
                }
                let (ts, day, all_day) = parse_ics_datetime(value);
                ev.dt_start_ts = ts;
                ev.start_day = day;
                ev.all_day = all_day;
            }
            "DTEND" => {
                let (ts, day, _) = parse_ics_datetime(value);
                ev.dt_end_ts = ts;
                ev.end_day = day;
            }
            "RRULE" => parse_rrule(&mut ev, value),
            "EXDATE" => {
                // EXDATE can be comma-separated date values
                let dates: Vec<String> = value.split(',').filter_map(|d| {
                    let d = d.trim().trim_end_matches('Z');
                    if d.len() >= 8 {
                        Some(d[..8].to_string())
                    } else {
                        None
                    }
                }).collect();
                if !dates.is_empty() {
                    if !ev.recur_exdates.is_empty() { ev.recur_exdates.push(','); }
                    ev.recur_exdates.push_str(&dates.join(","));
                }
            }
            "ATTENDEE" => {
                let addr = value.strip_prefix("mailto:").or_else(|| value.strip_prefix("MAILTO:")).unwrap_or(value);
                if !addr.is_empty() {
                    if !ev.attendees.is_empty() { ev.attendees.push('\n'); }
                    ev.attendees.push_str(addr);
                }
            }
            "ORGANIZER" => {
                let addr = value.strip_prefix("mailto:").or_else(|| value.strip_prefix("MAILTO:")).unwrap_or(value);
                ev.organizer = addr.to_string();
                if let Some(cn) = extract_param(key, "CN") {
                    ev.organizer_name = cn;
                }
            }
            "STATUS" => {
                let v = value.to_uppercase();
                ev.status = match v.as_str() {
                    "TENTATIVE" => "tentative".to_string(),
                    "TRANSPARENT" => "free".to_string(),
                    "CANCELLED" => "free".to_string(),
                    _ => "busy".to_string(),
                };
            }
            "URL" => ev.url = unescape_ics(value),
            "CLASS" => {
                ev.is_private = value.eq_ignore_ascii_case("PRIVATE") || value.eq_ignore_ascii_case("CONFIDENTIAL");
            }
            "RECURRENCE-ID" => {
                // This event is a recurrence exception — store the parent UID link
                // The UID itself serves as the link; recur_parent_uid is set externally
            }
            _ => {}
        }
    }

    if found { Some(ev) } else { None }
}

// ── Internal helpers ───────────────────────────────────────────────────

fn escape_ics(s: &str) -> String {
    s.replace('\\', "\\\\")
     .replace(';', "\\;")
     .replace(',', "\\,")
     .replace('\n', "\\n")
     .replace('\r', "")
}

fn unescape_ics(s: &str) -> String {
    s.replace("\\n", "\n")
     .replace("\\N", "\n")
     .replace("\\;", ";")
     .replace("\\,", ",")
     .replace("\\\\", "\\")
}

/// Fold a content line to 75-octet limit per RFC 5545.
fn fold_line(out: &mut String, line: &str) {
    if line.len() <= 75 {
        out.push_str(line);
        out.push_str("\r\n");
        return;
    }
    let mut rest = line;
    let mut first = true;
    while !rest.is_empty() {
        let take = if first { 75 } else { 74 };
        if rest.len() <= take {
            if !first { out.push(' '); }
            out.push_str(rest);
            out.push_str("\r\n");
            break;
        }
        if !first { out.push(' '); }
        out.push_str(&rest[..take]);
        out.push_str("\r\n");
        rest = &rest[take..];
        first = false;
    }
}

/// Unfold continuation lines (CRLF + space/tab → nothing).
fn unfold(raw: &str) -> String {
    raw.replace("\r\n ", "")
       .replace("\r\n\t", "")
       .replace("\n ", "")
       .replace("\n\t", "")
}

/// Parse "YYYYMMDD" or "YYYYMMDDTHHmmSS[Z]" → (unix_ts, YYYYMMDD, all_day).
fn parse_ics_datetime(val: &str) -> (i64, i32, bool) {
    // Strip any TZID= prefix noise — value is after the last ':'
    let v = val.rsplit(':').next().unwrap_or(val);
    let v = v.trim_end_matches('Z');

    if v.len() == 8 {
        // DATE only
        let y = v[0..4].parse::<i32>().unwrap_or(2026);
        let m = v[4..6].parse::<i32>().unwrap_or(1);
        let d = v[6..8].parse::<i32>().unwrap_or(1);
        let ts = crate::calendar::make_local_ts(y, m, d, 0, 0);
        (ts, y * 10000 + m * 100 + d, true)
    } else if v.len() >= 15 {
        let y = v[0..4].parse::<i32>().unwrap_or(2026);
        let mo = v[4..6].parse::<i32>().unwrap_or(1);
        let d = v[6..8].parse::<i32>().unwrap_or(1);
        let h = v[9..11].parse::<i32>().unwrap_or(0);
        let mi = v[11..13].parse::<i32>().unwrap_or(0);
        let ts = crate::calendar::make_local_ts(y, mo, d, h, mi);
        (ts, y * 10000 + mo * 100 + d, false)
    } else {
        (0, 0, false)
    }
}

fn parse_rrule(ev: &mut CalendarEvent, value: &str) {
    for part in value.split(';') {
        if let Some((k, v)) = part.split_once('=') {
            let k = k.to_uppercase();
            let v_upper = v.to_uppercase();
            match k.as_str() {
                "FREQ" => ev.rrule = v_upper,
                "INTERVAL" => ev.rrule_interval = v.parse().unwrap_or(1),
                "COUNT" => ev.rrule_count = v.parse().unwrap_or(0),
                "BYDAY" => ev.rrule_byday = v_upper,
                "UNTIL" => {
                    let d = &v_upper[..8.min(v_upper.len())];
                    ev.rrule_until = d.parse().unwrap_or(0);
                }
                _ => {}
            }
        }
    }
}

fn parse_trigger(mut ev: CalendarEvent, value: &str) -> CalendarEvent {
    let v = value.to_uppercase();
    let neg = v.starts_with('-');
    let v = v.trim_start_matches('-').trim_start_matches('+').trim_start_matches('P').trim_start_matches('T');
    if v.ends_with('M') {
        if let Ok(n) = v.trim_end_matches('M').parse::<i32>() {
            ev.reminder_mins = if neg { n } else { 0 };
        }
    } else if v.ends_with('H') {
        if let Ok(n) = v.trim_end_matches('H').parse::<i32>() {
            ev.reminder_mins = if neg { n * 60 } else { 0 };
        }
    } else if v.ends_with('D') {
        if let Ok(n) = v.trim_end_matches('D').parse::<i32>() {
            ev.reminder_mins = if neg { n * 1440 } else { 0 };
        }
    }
    ev
}

/// Extract a parameter value from a property key string like "DTSTART;TZID=America/New_York".
fn extract_param(key: &str, param_name: &str) -> Option<String> {
    let upper = param_name.to_uppercase();
    for part in key.split(';') {
        if let Some((k, v)) = part.split_once('=') {
            if k.trim().to_uppercase() == upper {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn fmt_date_ymd(ymd: i32) -> String {
    format!("{:04}{:02}{:02}", ymd / 10000, (ymd / 100) % 100, ymd % 100)
}

fn fmt_datetime_ts(ts: i64) -> String {
    let (y, m, d, h, mi) = crate::calendar::split_ts(ts);
    format!("{:04}{:02}{:02}T{:02}{:02}00", y, m, d, h, mi)
}

/// Build a METHOD:REPLY ICS for responding to a meeting invite.
///
/// * `ev` — the original event
/// * `partstat` — "ACCEPTED", "DECLINED", or "TENTATIVE"
/// * `attendee_email` — the replying attendee's email
pub fn build_reply_ics(ev: &CalendarEvent, partstat: &str, attendee_email: &str) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("BEGIN:VCALENDAR\r\n");
    out.push_str("VERSION:2.0\r\n");
    out.push_str("PRODID:-//Eudora Phoenix//CalendarEvent//EN\r\n");
    out.push_str("METHOD:REPLY\r\n");
    out.push_str("BEGIN:VEVENT\r\n");
    fold_line(&mut out, &format!("UID:{}", ev.uid));
    fold_line(&mut out, &format!("DTSTAMP:{}", now_utc_ics()));
    if ev.all_day {
        fold_line(&mut out, &format!("DTSTART;VALUE=DATE:{}", fmt_date_ymd(ev.start_day)));
        fold_line(&mut out, &format!("DTEND;VALUE=DATE:{}", fmt_date_ymd(ev.end_day)));
    } else {
        fold_line(&mut out, &format!("DTSTART:{}", fmt_datetime_ts(ev.dt_start_ts)));
        fold_line(&mut out, &format!("DTEND:{}", fmt_datetime_ts(ev.dt_end_ts)));
    }
    fold_line(&mut out, &format!("SUMMARY:{}", escape_ics(&ev.summary)));
    if !ev.organizer.is_empty() {
        fold_line(&mut out, &format!("ORGANIZER:mailto:{}", ev.organizer));
    }
    fold_line(&mut out, &format!("ATTENDEE;PARTSTAT={}:mailto:{}", partstat, attendee_email));
    out.push_str("END:VEVENT\r\n");
    out.push_str("END:VCALENDAR\r\n");
    out
}

fn now_utc_ics() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    // Use UTC — for simplicity, just use the timestamp directly
    // gmtime equivalent
    let t_c = t as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::gmtime_r(&t_c, &mut tm); }
    format!("{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday,
        tm.tm_hour, tm.tm_min, tm.tm_sec)
}
