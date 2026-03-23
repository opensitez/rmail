//! Statistics tab — email activity dashboard.
//!
//! Port of statwin.c CreateStatisticsPanel / build_stats.
//! Layout: hero header, period selector (Day/Week/Month/Year),
//! 6 stat pills, 3-column layout (Received Breakdown, Message Activity, Junk Mail).

use gtk4::prelude::*;
use macmbx::toc::{Toc, State};
use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

// ── CSS (port of theme.c stat-* classes) ──

const STAT_CSS: &str = "
.stat-bg {
    background: @theme_bg_color;
}
.stat-hero {
    background: linear-gradient(135deg, #1a365d 0%, #2a4a7f 100%);
    padding: 14px 20px; border-radius: 0 0 10px 10px;
}
.stat-hero-title { font-size: 1.1em; font-weight: 700; color: #ffffff; }
.stat-hero-sub { font-size: 0.78em; color: rgba(255,255,255,0.7); }
.s-pill {
    background: alpha(@theme_fg_color, 0.06);
    border-radius: 10px; padding: 10px 14px;
    min-width: 100px;
}
.s-pill-num { font-size: 1.5em; font-weight: 800; }
.s-pill-lbl { font-size: 0.82em; font-weight: 600; color: alpha(@theme_fg_color, 0.7); }
.s-pill-sub { font-size: 0.72em; color: alpha(@theme_fg_color, 0.45); }
.s-blue .s-pill-num { color: #3182ce; }
.s-green .s-pill-num { color: #38a169; }
.s-purple .s-pill-num { color: #805ad5; }
.s-cyan .s-pill-num { color: #0694a2; }
.s-red .s-pill-num { color: #e53e3e; }
.s-sect {
    font-weight: 700; font-size: 0.78em;
    color: alpha(@theme_fg_color, 0.50);
    letter-spacing: 0.5px;
}
.s-card {
    background: alpha(@theme_fg_color, 0.04);
    border-radius: 8px; padding: 10px 12px;
}
.s-card-title {
    font-weight: 700; font-size: 0.85em;
    color: alpha(@theme_fg_color, 0.7);
}
.s-bar-bg {
    background: alpha(@theme_fg_color, 0.08);
    border-radius: 4px; min-height: 14px;
}
.s-bar-fill {
    background: #3182ce; border-radius: 4px; min-height: 14px;
}
.s-bar-fill.s-bar-green { background: #38a169; }
.s-bar-fill.s-bar-amber { background: #d69e2e; }
.s-bar-fill.s-bar-purple { background: #805ad5; }
.s-bar-fill.s-bar-red { background: #e53e3e; }
.s-bar-lbl {
    font-size: 0.78em; color: alpha(@theme_fg_color, 0.65);
}
.s-bar-val {
    font-size: 0.78em; font-weight: 600;
    color: alpha(@theme_fg_color, 0.8);
}
.s-period-on {
    background: #3182ce; color: #ffffff;
    border-radius: 6px; padding: 4px 14px;
    font-weight: 700; font-size: 0.82em;
}
.s-period-off {
    background: alpha(@theme_fg_color, 0.06);
    color: alpha(@theme_fg_color, 0.6);
    border-radius: 6px; padding: 4px 14px;
    font-size: 0.82em;
}
.s-junk-good { color: #38a169; font-weight: 700; font-size: 0.85em; }
.s-junk-bad  { color: #e53e3e; font-weight: 700; font-size: 0.85em; }
";

fn install_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_data(STAT_CSS);
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

// ── Period enum (port of StatTimePeriod) ──

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Period {
    Day = 0,
    Week = 1,
    Month = 2,
    Year = 3,
}

impl Period {
    fn last_name(self) -> &'static str {
        match self {
            Period::Day => "yesterday",
            Period::Week => "last week",
            Period::Month => "last month",
            Period::Year => "last year",
        }
    }
}

// ── Mailbox counts from real TOC data ──

struct MailboxCounts {
    received: i64,
    sent: i64,
    read: i64,
    replied: i64,
    forwarded: i64,
    redirected: i64,
    junk: i64,
}

fn count_mailboxes(data_dir: &Path) -> MailboxCounts {
    let mut counts = MailboxCounts {
        received: 0, sent: 0, read: 0,
        replied: 0, forwarded: 0, redirected: 0, junk: 0,
    };

    // Try to open In mailbox
    let in_path = data_dir.join("In");
    if let Ok(toc) = Toc::open(&in_path) {
        counts.received = toc.msgs.len() as i64;
        for m in &toc.msgs {
            match m.state {
                State::Read => counts.read += 1,
                State::Replied => { counts.read += 1; counts.replied += 1; }
                State::Forwarded => { counts.read += 1; counts.forwarded += 1; }
                State::Redirected => { counts.read += 1; counts.redirected += 1; }
                _ => {}
            }
        }
    }

    // Try to open Out mailbox
    let out_path = data_dir.join("Out");
    if let Ok(toc) = Toc::open(&out_path) {
        counts.sent = toc.msgs.len() as i64;
    }

    // Try to open Junk mailbox
    let junk_path = data_dir.join("Junk");
    if let Ok(toc) = Toc::open(&junk_path) {
        counts.junk = toc.msgs.len() as i64;
    }

    counts
}

// ── Duration formatter (port of fmt_dur) ──

fn fmt_dur(s: i64) -> String {
    if s < 60 {
        format!("{}s", s)
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else if s < 86400 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d {}h", s / 86400, (s % 86400) / 3600)
    }
}

// ── Widget builders (port of s_pill, s_bar, s_sect, s_card_title) ──

fn s_pill(num: &str, lbl: &str, sub: &str, accent: &str) -> gtk4::Box {
    let bx = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    bx.add_css_class("s-pill");
    if !accent.is_empty() {
        bx.add_css_class(accent);
    }
    bx.set_hexpand(true);

    let n = gtk4::Label::new(Some(num));
    n.add_css_class("s-pill-num");
    n.set_xalign(0.0);
    bx.append(&n);

    let l = gtk4::Label::new(Some(lbl));
    l.add_css_class("s-pill-lbl");
    l.set_xalign(0.0);
    bx.append(&l);

    if !sub.is_empty() {
        let s = gtk4::Label::new(Some(sub));
        s.add_css_class("s-pill-sub");
        s.set_xalign(0.0);
        bx.append(&s);
    }

    bx
}

fn s_bar(label: &str, val: i64, mx: i64, fill_cls: Option<&str>) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    row.set_margin_top(1);
    row.set_margin_bottom(1);

    let lb = gtk4::Label::new(Some(label));
    lb.add_css_class("s-bar-lbl");
    lb.set_size_request(90, -1);
    lb.set_xalign(1.0);
    row.append(&lb);

    let bg = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    bg.add_css_class("s-bar-bg");
    bg.set_hexpand(true);

    let frac = if mx > 0 { (val as f64) / (mx as f64) } else { 0.0 };
    let frac = frac.min(1.0);

    let fill = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    fill.add_css_class("s-bar-fill");
    if let Some(cls) = fill_cls {
        fill.add_css_class(cls);
    }
    let pct = (frac * 100.0) as i32;
    if pct > 0 {
        fill.set_size_request((2.5 * pct as f64) as i32, -1);
    }
    bg.append(&fill);
    row.append(&bg);

    let v = gtk4::Label::new(Some(&format!("{}", val)));
    v.add_css_class("s-bar-val");
    v.set_size_request(45, -1);
    v.set_xalign(1.0);
    row.append(&v);

    row
}

fn s_sect(title: &str) -> gtk4::Label {
    let l = gtk4::Label::new(Some(title));
    l.add_css_class("s-sect");
    l.set_xalign(0.0);
    l.set_margin_top(14);
    l.set_margin_bottom(5);
    l
}

fn s_card_title(title: &str) -> gtk4::Label {
    let l = gtk4::Label::new(Some(title));
    l.add_css_class("s-card-title");
    l.set_xalign(0.0);
    l.set_margin_bottom(4);
    l
}

// ── Build stats content (port of build_stats) ──

fn build_stats(container: &gtk4::Box, period: Period, data_dir: &Path) {
    let last_n = period.last_name();
    let counts = count_mailboxes(data_dir);

    // ── Period selector ──
    let prow = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    prow.set_halign(gtk4::Align::Center);
    prow.set_margin_top(6);
    prow.set_margin_bottom(10);

    let periods = ["Day", "Week", "Month", "Year"];
    for (i, &plbl) in periods.iter().enumerate() {
        let btn = gtk4::Button::with_label(plbl);
        btn.set_has_frame(false);
        let cls = if i == period as usize { "s-period-on" } else { "s-period-off" };
        btn.add_css_class(cls);
        prow.append(&btn);
    }
    container.append(&prow);

    // ── Data (placeholder zeros for StatGet*, real counts from TOC) ──
    let rc: i64 = 0; // StatGetCurrentPeriod — not available yet
    let rl: i64 = 0; // StatGetLastPeriod — not available yet
    let sc: i64 = 0;
    let sl: i64 = 0;
    let rt = counts.received;
    let st = counts.sent;
    let rdt = counts.read;
    let ft: i64 = 0; // face time — not available yet
    let jt = counts.junk;
    let njt: i64 = 0; // scored not junk — not available yet

    // ── Top pills ──
    let s1 = format!("{}", rc);
    let u1 = format!("{} {} / {} total", rl, last_n, rt);
    let s2 = format!("{}", sc);
    let u2 = format!("{} {} / {} total", sl, last_n, st);
    let s3 = format!("{}", rdt);
    let rpct = if rt > 0 { 100 * rdt / rt } else { 0 };
    let u3 = format!("{}% of received", rpct);
    let s4 = fmt_dur(ft);
    let u4 = "face time total".to_string();
    let jtotal = jt + njt;
    let s5 = format!("{}", jt);
    let jpct = if jtotal > 0 { 100 * jt / jtotal } else { 0 };
    let u5 = format!("{}% of scored ({} total)", jpct, jtotal);
    let s6 = format!("{}", njt);
    let u6 = "scored not junk".to_string();

    let pills = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    pills.append(&s_pill(&s1, "Received", &u1, "s-blue"));
    pills.append(&s_pill(&s2, "Sent", &u2, "s-green"));
    pills.append(&s_pill(&s3, "Read", &u3, "s-purple"));
    pills.append(&s_pill(&s4, "Usage", &u4, "s-cyan"));
    pills.append(&s_pill(&s5, "Junk", &u5, "s-red"));
    pills.append(&s_pill(&s6, "Not Junk", &u6, "s-green"));
    container.append(&pills);

    // ── Three columns ──
    let cols = gtk4::Box::new(gtk4::Orientation::Horizontal, 14);
    cols.set_margin_top(4);

    // ── Col 1: Received Breakdown ──
    let c1 = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    c1.set_hexpand(true);

    c1.append(&s_sect("RECEIVED BREAKDOWN"));
    let rc1 = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    rc1.add_css_class("s-card");

    match period {
        Period::Day => {
            // Hourly breakdown — placeholder zeros (no StatGetHourlyData yet)
            let hrs: [i64; 24] = [0; 24];
            let mx = hrs.iter().copied().max().unwrap_or(0).max(1);
            for i in 0..24 {
                if hrs[i] > 0 || (i >= 7 && i <= 21) {
                    let lb = format!("{:02}:00", i);
                    rc1.append(&s_bar(&lb, hrs[i], mx, None));
                }
            }
        }
        Period::Week => {
            // Weekly breakdown — placeholder zeros
            let day_names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
            let wk: [i64; 7] = [0; 7];
            let mx = wk.iter().copied().max().unwrap_or(0).max(1);
            for i in 0..7 {
                rc1.append(&s_bar(day_names[i], wk[i], mx, None));
            }
        }
        _ => {
            // Month/Year — summary text
            let b1 = format!("This period: {} received", rc);
            let b2 = format!("Last period: {} received", rl);
            let l1 = gtk4::Label::new(Some(&b1));
            l1.add_css_class("s-bar-lbl");
            l1.set_xalign(0.0);
            rc1.append(&l1);
            let l2 = gtk4::Label::new(Some(&b2));
            l2.add_css_class("s-bar-lbl");
            l2.set_xalign(0.0);
            rc1.append(&l2);
        }
    }

    if rc == 0 && rl == 0 {
        let e = gtk4::Label::new(Some("No activity recorded yet"));
        e.add_css_class("s-pill-sub");
        e.set_margin_top(6);
        e.set_margin_bottom(6);
        rc1.append(&e);
    }
    c1.append(&rc1);
    cols.append(&c1);

    // ── Col 2: Message Activity ──
    let c2 = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    c2.set_hexpand(true);

    c2.append(&s_sect("MESSAGE ACTIVITY"));
    let ac = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    ac.add_css_class("s-card");

    let fwd = counts.forwarded;
    let rply = counts.replied;
    let rdir = counts.redirected;
    let ratt: i64 = 0; // StatGetTotal(kStatReceivedAttach) — not available
    let satt: i64 = 0; // StatGetTotal(kStatSentAttach) — not available
    let mx = rt.max(st).max(1);

    ac.append(&s_bar("Received", rt, mx, None));
    ac.append(&s_bar("Sent", st, mx, Some("s-bar-green")));
    ac.append(&s_bar("Read", rdt, mx, Some("s-bar-amber")));
    ac.append(&s_bar("Replied", rply, mx, Some("s-bar-purple")));
    ac.append(&s_bar("Forwarded", fwd, mx, Some("s-bar-green")));
    ac.append(&s_bar("Redirected", rdir, mx, None));
    ac.append(&s_bar("Attach recv", ratt, mx, Some("s-bar-amber")));
    ac.append(&s_bar("Attach sent", satt, mx, Some("s-bar-green")));
    c2.append(&ac);

    // Face time
    c2.append(&s_sect("FACE TIME"));
    let fc = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    fc.add_css_class("s-card");

    let fr: i64 = 0; // StatGetTotal(kStatFaceTimeRead)
    let fw: i64 = 0; // StatGetTotal(kStatFaceTimeCompose)
    let fo: i64 = 0; // StatGetTotal(kStatFaceTimeOther)
    let fm = fr.max(fw).max(fo).max(1);

    fc.append(&s_bar("Reading", fr, fm, None));
    fc.append(&s_bar("Composing", fw, fm, Some("s-bar-green")));
    fc.append(&s_bar("Other", fo, fm, Some("s-bar-amber")));

    // Percentages
    let ftot = fr + fw + fo;
    if ftot > 0 {
        let pctbuf = format!(
            "Reading {}%   Composing {}%   Other {}%",
            100 * fr / ftot,
            100 * fw / ftot,
            100 * fo / ftot,
        );
        let pl = gtk4::Label::new(Some(&pctbuf));
        pl.add_css_class("s-pill-sub");
        pl.set_margin_top(4);
        fc.append(&pl);
    }
    c2.append(&fc);
    cols.append(&c2);

    // ── Col 3: Junk Mail ──
    let c3 = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    c3.set_hexpand(true);

    c3.append(&s_sect("JUNK MAIL"));
    let jc = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    jc.add_css_class("s-card");

    let wl: i64 = 0;  // StatGetTotal(kStatWhiteList)
    let fp: i64 = 0;  // StatGetTotal(kStatFalsePositives)
    let fnn: i64 = 0; // StatGetTotal(kStatFalseNegatives)
    let fwl: i64 = 0; // StatGetTotal(kStatFalseWhiteList)

    let jmx = jt.max(njt).max(1);
    jc.append(&s_bar("Scored Junk", jt, jmx, Some("s-bar-red")));
    jc.append(&s_bar("Scored OK", njt, jmx, Some("s-bar-green")));
    jc.append(&s_bar("Whitelist", wl, jmx, None));
    jc.append(&s_bar("false Pos", fp, jmx, Some("s-bar-amber")));
    jc.append(&s_bar("false Neg", fnn, jmx, Some("s-bar-red")));
    if fwl > 0 {
        jc.append(&s_bar("false WL", fwl, jmx, Some("s-bar-amber")));
    }

    // Accuracy
    if jtotal > 0 {
        let correct = jtotal - fp - fnn;
        let accuracy = if jtotal > fwl {
            (100 * correct / (jtotal - fwl)).max(0)
        } else {
            0
        };
        let abuf = format!("Accuracy: {}%", accuracy);
        let al = gtk4::Label::new(Some(&abuf));
        al.add_css_class(if accuracy >= 90 { "s-junk-good" } else { "s-junk-bad" });
        al.set_margin_top(6);
        al.set_xalign(0.0);
        jc.append(&al);

        let detail = format!(
            "{} scored / {} false pos / {} false neg",
            jtotal, fp, fnn
        );
        let dl = gtk4::Label::new(Some(&detail));
        dl.add_css_class("s-pill-sub");
        dl.set_xalign(0.0);
        jc.append(&dl);
    } else {
        let nj = gtk4::Label::new(Some("No junk scoring data yet"));
        nj.add_css_class("s-pill-sub");
        nj.set_margin_top(4);
        jc.append(&nj);
    }
    c3.append(&jc);

    // Collection period
    c3.append(&s_sect("COLLECTION PERIOD"));
    let cp = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    cp.add_css_class("s-card");

    // No StatGetStartTime yet — show placeholder
    let ns = gtk4::Label::new(Some("No data collected yet"));
    ns.add_css_class("s-pill-sub");
    cp.append(&ns);

    c3.append(&cp);
    cols.append(&c3);
    container.append(&cols);
}

// ── Public entry point (port of CreateStatisticsPanel) ──

pub fn build_statistics_tab(data_dir: &Path) -> gtk4::Widget {
    install_css();

    let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    outer.add_css_class("stat-bg");
    outer.add_css_class("panel-bg");
    outer.set_hexpand(true);
    outer.set_vexpand(true);

    // Hero
    let hero = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    hero.add_css_class("stat-hero");
    let title = gtk4::Label::new(Some("Email Statistics"));
    title.add_css_class("stat-hero-title");
    title.set_xalign(0.0);
    hero.append(&title);
    let sub = gtk4::Label::new(Some("Your email activity at a glance"));
    sub.add_css_class("stat-hero-sub");
    sub.set_xalign(0.0);
    hero.append(&sub);
    outer.append(&hero);

    // Scrollable body
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scroll.set_vexpand(true);

    let body_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    body_box.set_margin_start(24);
    body_box.set_margin_end(24);
    body_box.set_margin_top(6);
    body_box.set_margin_bottom(24);

    // Build initial stats (Day period)
    let data_dir_owned = data_dir.to_path_buf();
    build_stats(&body_box, Period::Day, &data_dir_owned);

    // Wire up period buttons for rebuild
    let body_ref = body_box.clone();
    let data_dir_rc = Rc::new(data_dir_owned);
    let current_period = Rc::new(Cell::new(Period::Day));

    // Walk the period row to connect signals
    // The period row is the first child of body_box
    if let Some(prow) = body_box.first_child() {
        let mut btn_widget = prow.first_child();
        let mut idx = 0u8;
        while let Some(w) = btn_widget {
            if let Ok(btn) = w.clone().downcast::<gtk4::Button>() {
                let body = body_ref.clone();
                let dd = data_dir_rc.clone();
                let cp = current_period.clone();
                let period_idx = idx;
                btn.connect_clicked(move |_| {
                    let new_period = match period_idx {
                        0 => Period::Day,
                        1 => Period::Week,
                        2 => Period::Month,
                        _ => Period::Year,
                    };
                    cp.set(new_period);
                    // Remove all children
                    while let Some(child) = body.first_child() {
                        body.remove(&child);
                    }
                    build_stats(&body, new_period, &dd);
                });
            }
            btn_widget = w.next_sibling();
            idx += 1;
        }
    }

    scroll.set_child(Some(&body_box));
    outer.append(&scroll);

    outer.upcast()
}
