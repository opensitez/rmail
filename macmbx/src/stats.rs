//! Email statistics collection and persistence.
//!
//! Port of Eudora's statmng.c data model and storage.
//! Tracks received/sent/read counts, face time, junk scoring,
//! with day/week/month/year period rotation and running averages.
//!
//! Disk format: XML file "eudora_stats.xml" in the data directory.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// ─── Stat types (port of Eudora StatType enum) ──────────────────────────────

/// Numeric stat types (hourly granularity, stored as time-series arrays).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NumStat {
    ReceivedMail = 0,
    SentMail = 1,
    FaceTime = 2,
    ScoredJunk = 3,
    ScoredNotJunk = 4,
    WhiteList = 5,
    FalsePositives = 6,
    FalseNegatives = 7,
    FalseWhiteList = 8,
}

const NUM_STAT_COUNT: usize = 9;

/// Short stat types (simple scalar counters per period).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ShortStat {
    ReceivedAttach = 0,
    SentAttach = 1,
    ReadMsg = 2,
    Unused = 3,
    ForwardMsg = 4,
    ReplyMsg = 5,
    RedirectMsg = 6,
    FaceTimeRead = 7,
    FaceTimeCompose = 8,
    FaceTimeOther = 9,
}

const SHORT_STAT_COUNT: usize = 10;

/// Time period for queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Day,
    Week,
    Month,
    Year,
}

// ─── Data structures (port of PeriodicStats, NumericStats, etc.) ─────────────

#[derive(Debug, Clone, Default)]
pub struct PeriodicStats {
    pub day: [i64; 24],   // hours 0-23
    pub week: [i64; 7],   // Sun(0)-Sat(6)
    pub month: [i64; 31], // days 0-30
    pub year: [i64; 12],  // Jan(0)-Dec(11)
}

#[derive(Debug, Clone, Default)]
pub struct NumericStats {
    pub current: PeriodicStats,
    pub last: PeriodicStats,
    pub average: PeriodicStats,
    pub total: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ShortPeriodic {
    pub day: i64,
    pub week: i64,
    pub month: i64,
    pub year: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ShortNumericStats {
    pub current: ShortPeriodic,
    pub last: ShortPeriodic,
    pub average: ShortPeriodic,
    pub total: i64,
}

// ─── Calendar snapshot ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct CalSnap {
    year: i32,
    mon: u32,   // 0-11
    mday: u32,  // 1-31
    hour: u32,  // 0-23
    wday: u32,  // 0=Sun
    yday: u32,  // 0-365
}

fn cal_now() -> (i64, CalSnap) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    (secs, cal_from_epoch(secs))
}

fn cal_from_epoch(secs: i64) -> CalSnap {
    // Pure arithmetic — no libc, works on all platforms.
    // Uses UTC (consistent time slotting across sessions).
    let day_secs = secs.rem_euclid(86400);
    let hour = (day_secs / 3600) as u32;
    let total_days = secs.div_euclid(86400);

    // Day of week: Jan 1 1970 was Thursday (4)
    let wday = ((total_days % 7 + 4) % 7) as u32;

    // Year, month, day from days since epoch
    let (year, mon, mday, yday) = days_to_ymd_yday(total_days);

    CalSnap {
        year: year as i32,
        mon: mon as u32,
        mday: mday as u32,
        hour,
        wday,
        yday: yday as u32,
    }
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

/// Convert days since epoch to (year, month 0-11, day 1-31, yday 0-based).
fn days_to_ymd_yday(mut days: i64) -> (i64, i64, i64, i64) {
    let mut year = 1970i64;
    loop {
        let diy = if is_leap(year) { 366 } else { 365 };
        if days < diy { break; }
        days -= diy;
        year += 1;
    }
    let yday = days;
    let mdays: [i64; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 0i64;
    for m in 0..12 {
        if days < mdays[m] { month = m as i64; break; }
        days -= mdays[m];
        if m == 11 { month = 11; }
    }
    (year, month, days + 1, yday)
}

fn week_num(snap: &CalSnap) -> i32 {
    (snap.yday as i32 + 7 - snap.wday as i32) / 7
}

// ─── Main Stats structure ────────────────────────────────────────────────────

pub struct Stats {
    pub start_time: i64,
    pub junk_start_time: i64,
    pub current_time: i64,
    pub num: [NumericStats; NUM_STAT_COUNT],
    pub short: [ShortNumericStats; SHORT_STAT_COUNT],
    last_check: CalSnap,
    dirty: bool,
    path: PathBuf,
}

const NUM_STAT_NAMES: [&str; NUM_STAT_COUNT] = [
    "ReceivedMail", "SentMail", "FaceTime",
    "ScoredJunk", "ScoredNotJunk", "WhiteList",
    "FalsePositives", "FalseNegatives", "FalseWhiteList",
];

const SHORT_STAT_NAMES: [&str; SHORT_STAT_COUNT] = [
    "ReceivedAttach", "SentAttach", "ReadMsg", "Unused",
    "ForwardMsg", "ReplyMsg", "RedirectMsg",
    "FaceTimeRead", "FaceTimeCompose", "FaceTimeOther",
];

impl Stats {
    /// Open (or create) stats from data_dir/eudora_stats.xml.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("eudora_stats.xml");
        let (now, snap) = cal_now();
        let mut s = Stats {
            start_time: now,
            junk_start_time: now,
            current_time: now,
            num: Default::default(),
            short: Default::default(),
            last_check: snap,
            dirty: false,
            path,
        };
        s.load();
        s
    }

    /// Save if dirty.
    pub fn save(&mut self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.save_force()
    }

    /// Force save to disk.
    pub fn save_force(&mut self) -> std::io::Result<()> {
        let tmp = self.path.with_extension("xml.tmp");
        let mut f = fs::File::create(&tmp)?;

        writeln!(f, "<?xml version=\"1.0\"?>")?;
        writeln!(f, "<Statistics>")?;
        writeln!(f, " <Version>2</Version>")?;
        writeln!(f, " <StartTime>{}</StartTime>", self.start_time)?;
        writeln!(f, " <JunkStartTime>{}</JunkStartTime>", self.junk_start_time)?;
        let (now, _) = cal_now();
        writeln!(f, " <CurrentTime>{}</CurrentTime>", now)?;

        for i in 0..NUM_STAT_COUNT {
            Self::write_numeric(&mut f, NUM_STAT_NAMES[i], &self.num[i])?;
        }
        for i in 0..SHORT_STAT_COUNT {
            Self::write_short(&mut f, SHORT_STAT_NAMES[i], &self.short[i])?;
        }
        writeln!(f, "</Statistics>")?;
        drop(f);

        fs::rename(&tmp, &self.path)?;
        self.dirty = false;
        Ok(())
    }

    // ── Update ──

    /// Increment a numeric stat by `value` using current time.
    pub fn update_num(&mut self, which: NumStat, value: i64) {
        self.check_time();
        self.update_num_lo(which, value);
    }

    /// Increment a short stat by `value` using current time.
    pub fn update_short(&mut self, which: ShortStat, value: i64) {
        self.check_time();
        self.update_short_lo(which, value);
    }

    /// Increment a numeric stat at a specific epoch timestamp.
    pub fn update_num_at(&mut self, which: NumStat, value: i64, when: i64) {
        if value == 0 { return; }
        self.check_time();
        let (now, _) = cal_now();
        if when > now {
            self.update_num_lo(which, value);
            return;
        }
        let dt = cal_from_epoch(when);
        let cur = &self.last_check;
        let d_day = cur.yday as i32 - dt.yday as i32;
        let d_week = week_num(cur) - week_num(&dt);
        let d_month = cur.mon as i32 - dt.mon as i32;
        let d_year = cur.year - dt.year;

        let ns = &mut self.num[which as usize];
        if d_year == 0 {
            ns.current.year[dt.mon as usize] += value;
            if d_month == 0 {
                ns.current.month[dt.mday as usize - 1] += value;
            } else {
                if d_month == 1 { ns.last.month[dt.mday as usize - 1] += value; }
                ns.average.month[dt.mday as usize - 1] += value;
            }
            if d_week == 0 {
                ns.current.week[dt.wday as usize] += value;
            } else {
                if d_week == 1 { ns.last.week[dt.wday as usize] += value; }
                ns.average.week[dt.wday as usize] += value;
            }
            if d_day == 0 {
                ns.current.day[dt.hour as usize] += value;
            } else {
                if d_day == 1 { ns.last.day[dt.hour as usize] += value; }
                ns.average.day[dt.hour as usize] += value;
            }
        } else {
            if d_year == 1 {
                ns.last.year[dt.mon as usize] += value;
                if dt.mon == 11 && cur.mon == 0 {
                    ns.last.month[dt.mday as usize - 1] += value;
                }
            }
            ns.average.year[dt.mon as usize] += value;
        }
        ns.total += value;
        self.dirty = true;
    }

    // ── Query ──

    pub fn is_loaded(&self) -> bool { true }

    pub fn get_num_total(&self, which: NumStat) -> i64 {
        self.num[which as usize].total
    }

    pub fn get_short_total(&self, which: ShortStat) -> i64 {
        self.short[which as usize].total
    }

    pub fn get_num_current(&self, which: NumStat, period: Period) -> i64 {
        let ns = &self.num[which as usize];
        match period {
            Period::Day => ns.current.day.iter().sum(),
            Period::Week => ns.current.week.iter().sum(),
            Period::Month => ns.current.month.iter().sum(),
            Period::Year => ns.current.year.iter().sum(),
        }
    }

    pub fn get_num_last(&self, which: NumStat, period: Period) -> i64 {
        let ns = &self.num[which as usize];
        match period {
            Period::Day => ns.last.day.iter().sum(),
            Period::Week => ns.last.week.iter().sum(),
            Period::Month => ns.last.month.iter().sum(),
            Period::Year => ns.last.year.iter().sum(),
        }
    }

    pub fn get_short_current(&self, which: ShortStat, period: Period) -> i64 {
        let p = &self.short[which as usize].current;
        match period {
            Period::Day => p.day,
            Period::Week => p.week,
            Period::Month => p.month,
            Period::Year => p.year,
        }
    }

    pub fn get_short_last(&self, which: ShortStat, period: Period) -> i64 {
        let p = &self.short[which as usize].last;
        match period {
            Period::Day => p.day,
            Period::Week => p.week,
            Period::Month => p.month,
            Period::Year => p.year,
        }
    }

    pub fn get_hourly(&self, which: NumStat) -> [i64; 24] {
        self.num[which as usize].current.day
    }

    pub fn get_weekly(&self, which: NumStat) -> [i64; 7] {
        self.num[which as usize].current.week
    }

    pub fn get_monthly(&self, which: NumStat) -> [i64; 31] {
        self.num[which as usize].current.month
    }

    pub fn get_yearly(&self, which: NumStat) -> [i64; 12] {
        self.num[which as usize].current.year
    }

    // ── Time period rotation (port of CheckTimeChange) ──

    pub fn check_time(&mut self) {
        let (now, dt) = cal_now();
        let prev = &self.last_check;

        const NEW_YEAR: u32 = 0x01;
        const SKIP_YEAR: u32 = 0x02;
        const NEW_MONTH: u32 = 0x04;
        const SKIP_MONTH: u32 = 0x08;
        const NEW_WEEK: u32 = 0x10;
        const SKIP_WEEK: u32 = 0x20;
        const NEW_DAY: u32 = 0x40;
        const SKIP_DAY: u32 = 0x80;

        let mut flags: u32 = 0;

        if dt.year > prev.year {
            flags |= NEW_YEAR | NEW_MONTH | NEW_DAY;
            if dt.year - prev.year > 1 {
                flags |= SKIP_YEAR | SKIP_MONTH | SKIP_WEEK | SKIP_DAY;
            }
        }
        if dt.mon > prev.mon || dt.year > prev.year {
            flags |= NEW_MONTH | NEW_DAY;
            if dt.mon as i32 - prev.mon as i32 > 1 {
                flags |= SKIP_MONTH | SKIP_WEEK | SKIP_DAY;
            }
        }
        let cur_wk = week_num(&dt);
        let prev_wk = week_num(prev);
        if cur_wk > prev_wk || dt.year > prev.year {
            flags |= NEW_WEEK | NEW_DAY;
            if cur_wk - prev_wk > 1 {
                flags |= SKIP_WEEK | SKIP_DAY;
            }
        }
        if dt.yday > prev.yday || dt.year > prev.year {
            flags |= NEW_DAY;
            if dt.yday as i32 - prev.yday as i32 > 1 {
                flags |= SKIP_DAY;
            }
        }

        if flags != 0 {
            for ns in &mut self.num {
                if flags & NEW_YEAR != 0 {
                    ns.last.year = ns.current.year;
                    add_to_ave_arr(&mut ns.average.year, &ns.current.year);
                    ns.current.year = [0; 12];
                    if flags & SKIP_YEAR != 0 { ns.last.year = [0; 12]; }
                }
                if flags & NEW_MONTH != 0 {
                    ns.last.month = ns.current.month;
                    add_to_ave_arr(&mut ns.average.month, &ns.current.month);
                    ns.current.month = [0; 31];
                    if flags & SKIP_MONTH != 0 { ns.last.month = [0; 31]; }
                }
                if flags & NEW_WEEK != 0 {
                    ns.last.week = ns.current.week;
                    add_to_ave_arr(&mut ns.average.week, &ns.current.week);
                    ns.current.week = [0; 7];
                    if flags & SKIP_WEEK != 0 { ns.last.week = [0; 7]; }
                }
                if flags & NEW_DAY != 0 {
                    ns.last.day = ns.current.day;
                    add_to_ave_arr(&mut ns.average.day, &ns.current.day);
                    ns.current.day = [0; 24];
                    if flags & SKIP_DAY != 0 { ns.last.day = [0; 24]; }
                }
            }
            for ss in &mut self.short {
                if flags & NEW_YEAR != 0 {
                    if flags & SKIP_YEAR != 0 { ss.current.year = 0; }
                    ss.last.year = ss.current.year;
                    ss.current.year = 0;
                }
                if flags & NEW_MONTH != 0 {
                    if flags & SKIP_MONTH != 0 { ss.current.month = 0; }
                    ss.last.month = ss.current.month;
                    ss.current.month = 0;
                }
                if flags & NEW_WEEK != 0 {
                    if flags & SKIP_WEEK != 0 { ss.current.week = 0; }
                    ss.last.week = ss.current.week;
                    ss.current.week = 0;
                }
                if flags & NEW_DAY != 0 {
                    if flags & SKIP_DAY != 0 { ss.current.day = 0; }
                    ss.last.day = ss.current.day;
                    ss.current.day = 0;
                }
            }
            self.dirty = true;
        }

        self.last_check = dt;
        self.current_time = now;
    }

    // ── Private helpers ──

    fn update_num_lo(&mut self, which: NumStat, value: i64) {
        if value == 0 { return; }
        let tm = &self.last_check;
        let ns = &mut self.num[which as usize];
        ns.current.day[tm.hour as usize] += value;
        ns.current.week[tm.wday as usize] += value;
        ns.current.month[tm.mday as usize - 1] += value;
        ns.current.year[tm.mon as usize] += value;
        ns.total += value;
        self.dirty = true;
    }

    fn update_short_lo(&mut self, which: ShortStat, value: i64) {
        if value == 0 { return; }
        let ss = &mut self.short[which as usize];
        ss.current.day += value;
        ss.current.week += value;
        ss.current.month += value;
        ss.current.year += value;
        ss.total += value;
        self.dirty = true;
    }

    // ── XML I/O ──

    fn load(&mut self) {
        let xml = match fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(_) => return, // No file — fresh stats
        };

        if let Some(v) = tag_int(&xml, "StartTime") { self.start_time = v; }
        if let Some(v) = tag_int(&xml, "JunkStartTime") { self.junk_start_time = v; }
        if let Some(v) = tag_int(&xml, "CurrentTime") { self.current_time = v; }

        for i in 0..NUM_STAT_COUNT {
            if let Some(block) = tag_content(&xml, NUM_STAT_NAMES[i]) {
                parse_numeric(&block, &mut self.num[i]);
            }
        }
        for i in 0..SHORT_STAT_COUNT {
            if let Some(block) = tag_content(&xml, SHORT_STAT_NAMES[i]) {
                parse_short_stat(&block, &mut self.short[i]);
            }
        }

        // Update last_check from current_time
        self.last_check = cal_from_epoch(self.current_time);
    }

    fn write_numeric(f: &mut fs::File, name: &str, ns: &NumericStats) -> std::io::Result<()> {
        writeln!(f, " <{}>", name)?;
        Self::write_periodic(f, "Current", &ns.current)?;
        Self::write_periodic(f, "Last", &ns.last)?;
        Self::write_periodic(f, "Average", &ns.average)?;
        writeln!(f, "  <Total>{}</Total>", ns.total)?;
        writeln!(f, " </{}>", name)
    }

    fn write_periodic(f: &mut fs::File, tag: &str, p: &PeriodicStats) -> std::io::Result<()> {
        writeln!(f, "  <{}>", tag)?;
        write_arr(f, "Day", &p.day)?;
        write_arr(f, "Week", &p.week)?;
        write_arr(f, "Month", &p.month)?;
        write_arr(f, "Year", &p.year)?;
        writeln!(f, "  </{}>", tag)
    }

    fn write_short(f: &mut fs::File, name: &str, ss: &ShortNumericStats) -> std::io::Result<()> {
        writeln!(f, " <{}>", name)?;
        writeln!(f, "  <Current>{} {} {} {}</Current>",
            ss.current.day, ss.current.week, ss.current.month, ss.current.year)?;
        writeln!(f, "  <Last>{} {} {} {}</Last>",
            ss.last.day, ss.last.week, ss.last.month, ss.last.year)?;
        writeln!(f, "  <Average>{} {} {} {}</Average>",
            ss.average.day, ss.average.week, ss.average.month, ss.average.year)?;
        writeln!(f, "  <Total>{}</Total>", ss.total)?;
        writeln!(f, " </{}>", name)
    }
}

impl Drop for Stats {
    fn drop(&mut self) {
        if self.dirty {
            let _ = self.save_force();
        }
    }
}

// ─── XML helpers ─────────────────────────────────────────────────────────────

fn tag_content(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

fn tag_int(xml: &str, tag: &str) -> Option<i64> {
    let s = tag_content(xml, tag)?;
    s.trim().parse().ok()
}

fn parse_int_array(text: &str, out: &mut [i64]) {
    for (i, tok) in text.split_whitespace().enumerate() {
        if i >= out.len() { break; }
        out[i] = tok.parse().unwrap_or(0);
    }
}

fn parse_periodic(xml: &str, tag: &str, p: &mut PeriodicStats) {
    if let Some(block) = tag_content(xml, tag) {
        if let Some(s) = tag_content(&block, "Day") { parse_int_array(&s, &mut p.day); }
        if let Some(s) = tag_content(&block, "Week") { parse_int_array(&s, &mut p.week); }
        if let Some(s) = tag_content(&block, "Month") { parse_int_array(&s, &mut p.month); }
        if let Some(s) = tag_content(&block, "Year") { parse_int_array(&s, &mut p.year); }
    }
}

fn parse_numeric(xml: &str, ns: &mut NumericStats) {
    parse_periodic(xml, "Current", &mut ns.current);
    parse_periodic(xml, "Last", &mut ns.last);
    parse_periodic(xml, "Average", &mut ns.average);
    if let Some(v) = tag_int(xml, "Total") { ns.total = v; }
}

fn parse_short_periodic(text: &str, p: &mut ShortPeriodic) {
    let mut vals = [0i64; 4];
    parse_int_array(text, &mut vals);
    p.day = vals[0];
    p.week = vals[1];
    p.month = vals[2];
    p.year = vals[3];
}

fn parse_short_stat(xml: &str, ss: &mut ShortNumericStats) {
    if let Some(s) = tag_content(xml, "Current") { parse_short_periodic(&s, &mut ss.current); }
    if let Some(s) = tag_content(xml, "Last") { parse_short_periodic(&s, &mut ss.last); }
    if let Some(s) = tag_content(xml, "Average") { parse_short_periodic(&s, &mut ss.average); }
    if let Some(v) = tag_int(xml, "Total") { ss.total = v; }
}

fn write_arr(f: &mut fs::File, tag: &str, arr: &[i64]) -> std::io::Result<()> {
    write!(f, "    <{}>", tag)?;
    for (i, v) in arr.iter().enumerate() {
        if i > 0 { write!(f, " ")?; }
        write!(f, "{}", v)?;
    }
    writeln!(f, "</{}>", tag)
}

fn add_to_ave_arr(to: &mut [i64], from: &[i64]) {
    for (t, f) in to.iter_mut().zip(from.iter()) {
        *t += *f;
    }
}
