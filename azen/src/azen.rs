use std::sync::{Arc, Mutex};

use webcore::browser::{Browser, BrowserChrome, BrowserDocument, BrowserGuest, BrowserOptions};
use webcore::dom::events::ListenerOptions;
use webcore::Document;

const EUDORA_HTML: &str = include_str!("html/eudora.html");
const EMAIL_HTML: &str = include_str!("html/email.html");
const SETTINGS_HTML: &str = include_str!("html/settings.html");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Skin {
    Eudora,
    Email,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EudoraTab {
    In,
    Out,
    Contacts,
    Filters,
    Compose,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EmailViewMode {
    Mail,
    Contacts,
    Compose,
}

#[derive(Clone, Debug)]
struct Mailbox {
    name: &'static str,
    email_label: &'static str,
    icon: &'static str,
    indent: u8,
    has_children: bool,
    separator_after: bool,
    show_in_email_skin: bool,
}

#[derive(Clone, Debug)]
struct LabelDef {
    name: &'static str,
    color: &'static str,
    bg_rgba: &'static str,
}

#[derive(Clone, Debug)]
struct MailMessage {
    id: usize,
    mailbox: String,
    from: String,
    to: String,
    cc: String,
    subject: String,
    date: String,
    short_date: String,
    size: String,
    preview: String,
    body_html: String,
    unread: bool,
    replied: bool,
    forwarded: bool,
    has_attach: bool,
    attachment_note: String,
    priority: &'static str, // "highest", "high", "normal", "low", "none"
    flagged: bool,
    avatar_initials: String,
    avatar_color: &'static str,
    labels: Vec<&'static str>,
}

#[derive(Clone, Debug)]
struct Contact {
    name: &'static str,
    initials: &'static str,
    email: &'static str,
    phone: &'static str,
    company: &'static str,
    title: &'static str,
    notes: &'static str,
    color: &'static str,
}

#[derive(Clone, Debug)]
struct MailFilter {
    name: &'static str,
    condition: &'static str,
    action: &'static str,
    enabled: bool,
}

#[derive(Clone, Debug)]
struct ComposeDraft {
    title: String,
    from: String,
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    attach: String,
    priority: String,
    body_html: String,
}

impl Default for ComposeDraft {
    fn default() -> Self {
        Self {
            title: "✏ New Message".to_string(),
            from: "You <you@example.com>".to_string(),
            to: String::new(),
            cc: String::new(),
            bcc: String::new(),
            subject: String::new(),
            attach: "None".to_string(),
            priority: "Normal".to_string(),
            body_html: "<p><br></p><p><br></p><div class=\"compose-sig\">— <br>Sent with Eudora 2026</div>".to_string(),
        }
    }
}

struct AppState {
    skin: Skin,
    last_mail_skin: Skin,
    pending_skin: Option<Skin>,
    dirty: bool,
    dark_mode: bool,
    mailboxes: Vec<Mailbox>,
    labels: Vec<LabelDef>,
    messages: Vec<MailMessage>,
    contacts: Vec<Contact>,
    filters: Vec<MailFilter>,

    // Active selections
    selected_mailbox: String,
    selected_label: Option<String>,
    selected_in_msg_id: usize,
    selected_out_msg_id: usize,
    selected_contact_idx: usize,
    eudora_tab: EudoraTab,
    email_mode: EmailViewMode,
    compose_open: bool,
    compose: ComposeDraft,
    settings_category: String,
    settings_status: String,
    status_left: String,
    status_center: String,
    status_right: String,
}

impl AppState {
    fn new() -> Self {
        let mailboxes = vec![
            Mailbox { name: "In",            email_label: "Inbox",   icon: "📥", indent: 0, has_children: false, separator_after: false, show_in_email_skin: true },
            Mailbox { name: "Out",           email_label: "Sent",    icon: "📤", indent: 0, has_children: false, separator_after: false, show_in_email_skin: true },
            Mailbox { name: "Drafts",        email_label: "Drafts",  icon: "📝", indent: 0, has_children: false, separator_after: false, show_in_email_skin: true },
            Mailbox { name: "Sent",          email_label: "Sent",    icon: "📨", indent: 0, has_children: false, separator_after: false, show_in_email_skin: false },
            Mailbox { name: "Archive",       email_label: "Archive", icon: "📦", indent: 0, has_children: false, separator_after: false, show_in_email_skin: true },
            Mailbox { name: "Spam",          email_label: "Spam",    icon: "⛔", indent: 0, has_children: false, separator_after: false, show_in_email_skin: true },
            Mailbox { name: "Trash",         email_label: "Trash",   icon: "🗑", indent: 0, has_children: false, separator_after: true,  show_in_email_skin: true },
            Mailbox { name: "Work",          email_label: "Work",    icon: "📁", indent: 0, has_children: true,  separator_after: false, show_in_email_skin: false },
            Mailbox { name: "Projects",      email_label: "Projects",icon: "📂", indent: 1, has_children: false, separator_after: false, show_in_email_skin: false },
            Mailbox { name: "Clients",       email_label: "Clients", icon: "📂", indent: 1, has_children: false, separator_after: false, show_in_email_skin: false },
            Mailbox { name: "Personal",      email_label: "Personal",icon: "📁", indent: 0, has_children: true,  separator_after: false, show_in_email_skin: false },
            Mailbox { name: "Receipts",      email_label: "Receipts",icon: "📂", indent: 1, has_children: false, separator_after: false, show_in_email_skin: false },
            Mailbox { name: "Travel",        email_label: "Travel",  icon: "📂", indent: 1, has_children: false, separator_after: true,  show_in_email_skin: false },
            Mailbox { name: "Mailing Lists", email_label: "Lists",   icon: "📋", indent: 0, has_children: false, separator_after: false, show_in_email_skin: false },
            Mailbox { name: "Newsletters",   email_label: "News",    icon: "📰", indent: 0, has_children: false, separator_after: false, show_in_email_skin: false },
        ];

        let labels = vec![
            LabelDef { name: "Work",     color: "#5b5fc7", bg_rgba: "rgba(91,95,199,0.15)" },
            LabelDef { name: "Personal", color: "#27ae60", bg_rgba: "rgba(39,174,96,0.15)" },
            LabelDef { name: "Urgent",   color: "#e74c3c", bg_rgba: "rgba(231,76,60,0.15)" },
            LabelDef { name: "Finance",  color: "#f39c12", bg_rgba: "rgba(243,156,18,0.15)" },
            LabelDef { name: "Dev",      color: "#3498db", bg_rgba: "rgba(52,152,219,0.15)" },
            LabelDef { name: "Design",   color: "#9b59b6", bg_rgba: "rgba(155,89,182,0.15)" },
            LabelDef { name: "Team",     color: "#1abc9c", bg_rgba: "rgba(26,188,156,0.15)" },
        ];

        let messages = vec![
            MailMessage {
                id: 1,
                mailbox: "In".to_string(),
                from: "Steve Dorner <sdorner@qualcomm.com>".to_string(),
                to: "you@example.com".to_string(),
                cc: "eudora-team@qualcomm.com".to_string(),
                subject: "Welcome to Eudora 2026!".to_string(),
                date: "Mar 7, 2026  9:15 AM".to_string(),
                short_date: "9:15 AM".to_string(),
                size: "8K".to_string(),
                preview: "Welcome to the all-new Eudora! Built from the ground up with a modern HTML/CSS rendering engine...".to_string(),
                body_html: r#"<h3 style="color:#326ea5;margin-top:0">Welcome to the all-new Eudora!</h3>
<p>We've rebuilt the classic email client you loved from the ground up, powered by a modern HTML/CSS rendering engine.</p>
<p><b>What's new in Eudora 2026:</b></p>
<ul>
  <li><b>Rich HTML Email</b> — Full CSS styling, tables, and embedded images</li>
  <li><b>Wazoo Panels</b> — Dockable tabbed mailbox &amp; tool panels</li>
  <li><b>Lightning Filter Engine</b> — 100× faster rule processing</li>
  <li><b>Address Book</b> — Integrated contact management with quick-search</li>
  <li><b>Dark Mode</b> — Easy on the eyes for late-night email sessions</li>
</ul>
<p>As I always said: <i>"Bringing Mail to the Masses."</i></p>
<p>Best regards,<br><b>Steve Dorner</b><br><span style="font-size:9pt;opacity:0.6">Creator of Eudora • Qualcomm Inc.</span></p>"#.to_string(),
                unread: true,
                replied: false,
                forwarded: false,
                has_attach: true,
                attachment_note: "📎 Eudora_2026_QuickStart.pdf (412 KB)".to_string(),
                priority: "highest",
                flagged: true,
                avatar_initials: "SD".to_string(),
                avatar_color: "#326ea5",
                labels: vec!["Work", "Urgent"],
            },
            MailMessage {
                id: 2,
                mailbox: "In".to_string(),
                from: "Alan Strider <astrider@example.com>".to_string(),
                to: "you@example.com".to_string(),
                cc: "finance@example.com".to_string(),
                subject: "Q1 Financial Report - Action Required".to_string(),
                date: "Mar 7, 2026  8:42 AM".to_string(),
                short_date: "8:42 AM".to_string(),
                size: "24K".to_string(),
                preview: "Please find the Q1 2026 summary below. Total revenue reached $4.2M (+15% QoQ)...".to_string(),
                body_html: r#"<p>Hi,</p>
<p>Please find the Q1 2026 summary below:</p>
<table style="width:100%;border-collapse:collapse;margin:12px 0;font-size:9.5pt;">
  <tr style="background:#326ea5;color:white;">
    <th style="padding:6px 10px;text-align:left;">Metric</th>
    <th style="padding:6px 10px;text-align:right;">Q1 2026</th>
    <th style="padding:6px 10px;text-align:right;">Q4 2025</th>
    <th style="padding:6px 10px;text-align:right;">Change</th>
  </tr>
  <tr>
    <td style="padding:5px 10px;border-bottom:1px solid #ddd;">Revenue</td>
    <td style="padding:5px 10px;text-align:right;border-bottom:1px solid #ddd;"><b>$4.2M</b></td>
    <td style="padding:5px 10px;text-align:right;border-bottom:1px solid #ddd;">$3.65M</td>
    <td style="padding:5px 10px;text-align:right;color:#2ea043;border-bottom:1px solid #ddd;">+15.1%</td>
  </tr>
  <tr>
    <td style="padding:5px 10px;border-bottom:1px solid #ddd;">Operating Costs</td>
    <td style="padding:5px 10px;text-align:right;border-bottom:1px solid #ddd;">$2.8M</td>
    <td style="padding:5px 10px;text-align:right;border-bottom:1px solid #ddd;">$2.9M</td>
    <td style="padding:5px 10px;text-align:right;color:#2ea043;border-bottom:1px solid #ddd;">-3.4%</td>
  </tr>
  <tr>
    <td style="padding:5px 10px;">Net Profit</td>
    <td style="padding:5px 10px;text-align:right;"><b>$1.4M</b></td>
    <td style="padding:5px 10px;text-align:right;">$750K</td>
    <td style="padding:5px 10px;text-align:right;color:#2ea043;"><b>+86.7%</b></td>
  </tr>
</table>
<p>Please sign off by Friday EOD.</p>
<p>Thanks,<br>Alan</p>"#.to_string(),
                unread: true,
                replied: false,
                forwarded: false,
                has_attach: true,
                attachment_note: "📎 Q1_2026_Financials.xlsx (24 KB)".to_string(),
                priority: "high",
                flagged: true,
                avatar_initials: "AS".to_string(),
                avatar_color: "#2ea043",
                labels: vec!["Finance", "Urgent"],
            },
            MailMessage {
                id: 3,
                mailbox: "In".to_string(),
                from: "Lisa Warren <lwarren@design.example>".to_string(),
                to: "you@example.com, design-team@example.com".to_string(),
                cc: "".to_string(),
                subject: "📅 Invite: Design Review — Mobile Nav".to_string(),
                date: "Mar 6, 2026  4:30 PM".to_string(),
                short_date: "Yesterday".to_string(),
                size: "12K".to_string(),
                preview: "Calendar Invitation: Design Review — Mobile Navigation. Monday, March 10, 2026 at 2:00 PM PST...".to_string(),
                body_html: r#"<div class="cal-card">
  <div class="cal-header">
    <div class="cal-label">Calendar Invitation</div>
    <div class="cal-title">Design Review — Mobile Navigation</div>
  </div>
  <div class="cal-body">
    <table class="cal-table">
      <tr>
        <td class="cal-field">When</td>
        <td><b>Monday, March 10, 2026</b><br>2:00 PM – 3:00 PM PST</td>
      </tr>
      <tr>
        <td class="cal-field">Where</td>
        <td>Conference Room B / <span style="color:#326ea5;text-decoration:underline">Zoom Link</span></td>
      </tr>
      <tr>
        <td class="cal-field">Organizer</td>
        <td>Lisa Warren &lt;lwarren@design.example&gt;</td>
      </tr>
    </table>
    <div class="cal-agenda">
      <div class="cal-agenda-label">Agenda</div>
      <ol>
        <li>Review updated wireframes for bottom tab bar</li>
        <li>Discuss gesture navigation prototype results</li>
        <li>Finalize icon set and micro-interactions</li>
      </ol>
    </div>
    <div class="cal-rsvp">
      <div class="cal-rsvp-btn"><div class="cal-rsvp-inner accept">✓ Accept</div></div>
      <div class="cal-rsvp-btn"><div class="cal-rsvp-inner tentative">? Tentative</div></div>
      <div class="cal-rsvp-btn"><div class="cal-rsvp-inner decline">✗ Decline</div></div>
    </div>
  </div>
</div>
<p style="font-size:9.5pt;opacity:0.7">Hi everyone — please review the Figma mocks before the meeting. Looking forward to your feedback!</p>"#.to_string(),
                unread: true,
                replied: false,
                forwarded: false,
                has_attach: true,
                attachment_note: "📎 invite.ics (4 KB)".to_string(),
                priority: "normal",
                flagged: true,
                avatar_initials: "LW".to_string(),
                avatar_color: "#d2372d",
                labels: vec!["Design", "Team"],
            },
            MailMessage {
                id: 4,
                mailbox: "In".to_string(),
                from: "GitHub <notifications@github.com>".to_string(),
                to: "you@example.com".to_string(),
                cc: "".to_string(),
                subject: "[litehtml/litehtml] PR #482: Add CSS Grid support".to_string(),
                date: "Mar 6, 2026  2:30 PM".to_string(),
                short_date: "Yesterday".to_string(),
                size: "6K".to_string(),
                preview: "@contributor merged 14 commits into master from feat-grid: grid-template-columns, fr units, gap...".to_string(),
                body_html: r#"<p><b>@contributor</b> merged 14 commits into <code>master</code> from <code>feat-grid</code>.</p>
<p>Changes:</p>
<ul>
  <li><code>grid-template-columns</code> and <code>grid-template-rows</code></li>
  <li><code>fr</code> units and <code>minmax()</code></li>
  <li><code>grid-gap</code> / <code>gap</code> shorthand</li>
  <li>Auto-placement algorithm</li>
</ul>
<p style="font-size:9pt;color:#666;">—<br>View it on GitHub or unsubscribe.</p>"#.to_string(),
                unread: false,
                replied: true,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "normal",
                flagged: false,
                avatar_initials: "GH".to_string(),
                avatar_color: "#24292f",
                labels: vec!["Dev"],
            },
            MailMessage {
                id: 5,
                mailbox: "In".to_string(),
                from: "Dana Park <dpark@example.com>".to_string(),
                to: "you@example.com".to_string(),
                cc: "speakers@conf2026.example".to_string(),
                subject: "Conference talk proposal accepted!".to_string(),
                date: "Mar 5, 2026  11:20 AM".to_string(),
                short_date: "Mar 5".to_string(),
                size: "5K".to_string(),
                preview: "Great news! Our talk 'Rendering HTML Email in 2026: Lessons from Eudora' was accepted...".to_string(),
                body_html: r#"<p>Great news!</p>
<p>Our talk <b>"Rendering HTML Email in 2026: Lessons from Eudora"</b> was accepted for the main track!</p>
<p>Slot: <b>April 18, 2:00 PM — Hall B</b></p>
<p>Let's sync this week to outline the slides.</p>
<p>— Dana</p>"#.to_string(),
                unread: false,
                replied: true,
                forwarded: true,
                has_attach: false,
                attachment_note: String::new(),
                priority: "normal",
                flagged: true,
                avatar_initials: "DP".to_string(),
                avatar_color: "#9b32a5",
                labels: vec!["Work", "Design"],
            },
            MailMessage {
                id: 6,
                mailbox: "In".to_string(),
                from: "Rust Weekly <newsletter@rustweekly.example>".to_string(),
                to: "subscribers@rustweekly.example".to_string(),
                cc: "".to_string(),
                subject: "This Week in Rust #642".to_string(),
                date: "Mar 4, 2026  7:00 AM".to_string(),
                short_date: "Mar 4".to_string(),
                size: "18K".to_string(),
                preview: "Highlights: Rust 1.86 released, zero-cost abstractions in UI toolkits, crate of the week...".to_string(),
                body_html: r#"<h3>This Week in Rust #642</h3>
<p>Hello and welcome to another issue of <i>This Week in Rust</i>!</p>
<ul>
  <li><b>Official:</b> Rust 1.86.0 release notes</li>
  <li><b>Project/Tooling:</b> Fast HTML/CSS layout engines in pure Rust</li>
  <li><b>Crate of the Week:</b> <code>webcore</code> — lightweight embeddable browser engine</li>
</ul>"#.to_string(),
                unread: false,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "low",
                flagged: false,
                avatar_initials: "RW".to_string(),
                avatar_color: "#d35400",
                labels: vec!["Dev"],
            },
            MailMessage {
                id: 7,
                mailbox: "In".to_string(),
                from: "Rick Langley <rick@example.com>".to_string(),
                to: "you@example.com".to_string(),
                cc: "".to_string(),
                subject: "Lunch Friday?".to_string(),
                date: "Mar 2, 2026  1:12 PM".to_string(),
                short_date: "Mar 2".to_string(),
                size: "1K".to_string(),
                preview: "Hey, want to grab lunch Friday? Thinking that new ramen place on 5th...".to_string(),
                body_html: r#"<p>Hey, want to grab lunch Friday? Thinking that new ramen place on 5th.</p>
<p>Let me know!</p><p>Rick</p>"#.to_string(),
                unread: false,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "none",
                flagged: false,
                avatar_initials: "RL".to_string(),
                avatar_color: "#d29922",
                labels: vec!["Personal"],
            },
            // Outbox messages
            MailMessage {
                id: 101,
                mailbox: "Out".to_string(),
                from: "you@example.com".to_string(),
                to: "Steve Dorner <sdorner@qualcomm.com>".to_string(),
                cc: "".to_string(),
                subject: "Re: Welcome to Eudora 2026!".to_string(),
                date: "Mar 7, 2026  10:45 AM".to_string(),
                short_date: "10:45 AM".to_string(),
                size: "3K".to_string(),
                preview: "Thanks Steve! The new version is amazing. Quick questions: Can I import old mailboxes?...".to_string(),
                body_html: r#"<p>Thanks Steve! The new version is amazing.</p>
<p>Quick questions:</p>
<ol>
  <li>Can I import old mailboxes?</li>
  <li>CalDAV support?</li>
</ol>"#.to_string(),
                unread: false,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "normal",
                flagged: false,
                avatar_initials: "SD".to_string(),
                avatar_color: "#326ea5",
                labels: vec!["Work"],
            },
            MailMessage {
                id: 102,
                mailbox: "Out".to_string(),
                from: "you@example.com".to_string(),
                to: "team@example.com".to_string(),
                cc: "astrider@example.com".to_string(),
                subject: "Dashboard redesign - timeline update".to_string(),
                date: "Mar 6, 2026  3:00 PM".to_string(),
                short_date: "Yesterday".to_string(),
                size: "2K".to_string(),
                preview: "Hi team, Running behind on the dashboard redesign. Pushing milestone to next Friday...".to_string(),
                body_html: r#"<p>Hi team,</p>
<p>Running behind on the dashboard redesign. Pushing milestone to next Friday.</p>"#.to_string(),
                unread: false,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "normal",
                flagged: false,
                avatar_initials: "TM".to_string(),
                avatar_color: "#1abc9c",
                labels: vec!["Team", "Work"],
            },
            MailMessage {
                id: 103,
                mailbox: "Out".to_string(),
                from: "you@example.com".to_string(),
                to: "Dana Park <dpark@example.com>".to_string(),
                cc: "".to_string(),
                subject: "Re: Conference talk proposal".to_string(),
                date: "Mar 5, 2026  2:15 PM".to_string(),
                short_date: "Mar 5".to_string(),
                size: "2K".to_string(),
                preview: "Fantastic news, Dana! Outline ideas: 1. History of desktop email clients 2. Modern rendering...".to_string(),
                body_html: r#"<p>Fantastic news, Dana!</p>
<p>Outline ideas:</p>
<ol>
  <li>History of desktop email clients</li>
  <li>Modern rendering challenges</li>
  <li>Live demo</li>
</ol>"#.to_string(),
                unread: false,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "none",
                flagged: false,
                avatar_initials: "DP".to_string(),
                avatar_color: "#9b32a5",
                labels: vec!["Work"],
            },
            // Drafts
            MailMessage {
                id: 201,
                mailbox: "Drafts".to_string(),
                from: "you@example.com".to_string(),
                to: "jbeckley@qualcomm.com".to_string(),
                cc: "".to_string(),
                subject: "[Draft] Toolbar icon measurements".to_string(),
                date: "Mar 7, 2026  11:00 AM".to_string(),
                short_date: "11:00 AM".to_string(),
                size: "2K".to_string(),
                preview: "Hi Jeff, Here are the updated measurements for the Wazoo sidebar and toolbar buttons...".to_string(),
                body_html: r#"<p>Hi Jeff,</p><p>Here are the updated measurements for the Wazoo sidebar and toolbar buttons.</p>"#.to_string(),
                unread: true,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "normal",
                flagged: false,
                avatar_initials: "JB".to_string(),
                avatar_color: "#a55532",
                labels: vec!["Work", "Design"],
            },
            // Spam
            MailMessage {
                id: 301,
                mailbox: "Spam".to_string(),
                from: "Prize Desk <winner@lottery-example.org>".to_string(),
                to: "you@example.com".to_string(),
                cc: "".to_string(),
                subject: "CONGRATULATIONS! You won a free cruise!".to_string(),
                date: "Mar 3, 2026  3:11 AM".to_string(),
                short_date: "Mar 3".to_string(),
                size: "4K".to_string(),
                preview: "Click here immediately to claim your luxury Bahamas cruise reward...".to_string(),
                body_html: r#"<p><b>Filtered by Eudora Spam Keywords Rule.</b></p><p>Click here immediately to claim your luxury Bahamas cruise reward.</p>"#.to_string(),
                unread: true,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "low",
                flagged: false,
                avatar_initials: "PD".to_string(),
                avatar_color: "#7f8c8d",
                labels: vec![],
            },
            // Work
            MailMessage {
                id: 401,
                mailbox: "Work".to_string(),
                from: "Bob Chen <bchen@example.com>".to_string(),
                to: "you@example.com".to_string(),
                cc: "".to_string(),
                subject: "Production deployment checklist".to_string(),
                date: "Mar 4, 2026  5:10 PM".to_string(),
                short_date: "Mar 4".to_string(),
                size: "7K".to_string(),
                preview: "All staging checks passed. Ready for the rollout window tonight at 19:00 UTC...".to_string(),
                body_html: r#"<p>Hi,</p><p>All staging checks passed. Ready for the rollout window tonight at 19:00 UTC.</p><p>— Bob</p>"#.to_string(),
                unread: true,
                replied: false,
                forwarded: false,
                has_attach: false,
                attachment_note: String::new(),
                priority: "high",
                flagged: false,
                avatar_initials: "BC".to_string(),
                avatar_color: "#32a582",
                labels: vec!["Work", "Dev"],
            },
        ];

        let contacts = vec![
            Contact { name: "Steve Dorner", email: "sdorner@qualcomm.com",   phone: "(858) 555-0100", company: "Qualcomm",    title: "Chief Architect",  notes: "Creator of Eudora. Prefers plain text.", color: "#326ea5", initials: "SD" },
            Contact { name: "Jeff Beckley", email: "jbeckley@qualcomm.com",  phone: "(858) 555-0101", company: "Qualcomm",    title: "Senior Engineer",  notes: "Toolbar and UI specialist.",             color: "#a55532", initials: "JB" },
            Contact { name: "Alan Strider", email: "astrider@example.com",   phone: "(415) 555-0200", company: "Acme Corp",   title: "Finance Director", notes: "Sends quarterly reports.",               color: "#2ea043", initials: "AS" },
            Contact { name: "Dana Park",    email: "dpark@example.com",      phone: "(415) 555-0201", company: "TechStudio",  title: "Design Lead",      notes: "Conference co-speaker.",                 color: "#9b32a5", initials: "DP" },
            Contact { name: "Rick Langley", email: "rick@example.com",       phone: "(510) 555-0300", company: "",            title: "",                 notes: "College friend. Likes ramen.",           color: "#d29922", initials: "RL" },
            Contact { name: "Lisa Warren",  email: "lwarren@design.example", phone: "(510) 555-0301", company: "DesignWorks", title: "UX Researcher",    notes: "Working on mobile nav project.",         color: "#d2372d", initials: "LW" },
            Contact { name: "Bob Chen",     email: "bchen@example.com",      phone: "(650) 555-0400", company: "CloudSoft",   title: "DevOps Manager",   notes: "Handles infrastructure.",               color: "#32a582", initials: "BC" },
            Contact { name: "Maria Santos", email: "msantos@example.com",    phone: "(650) 555-0401", company: "DataFlow",    title: "Data Scientist",   notes: "ML and analytics expert.",              color: "#6450b4", initials: "MS" },
        ];

        let filters = vec![
            MailFilter { name: "Mailing Lists",  condition: "To contains @list.example",          action: "Move to Mailing Lists",    enabled: true  },
            MailFilter { name: "Newsletters",    condition: "From contains newsletter",            action: "Move to Newsletters",      enabled: true  },
            MailFilter { name: "Spam Keywords",  condition: "Subject contains winner|lottery",     action: "Move to Junk",             enabled: true  },
            MailFilter { name: "Work Priority",  condition: "From contains @qualcomm.com",         action: "Set Priority: High, Flag", enabled: true  },
            MailFilter { name: "Large Attach",   condition: "Attachment Size > 5MB",               action: "Add Label: Large",         enabled: false },
            MailFilter { name: "Auto-Reply OOO", condition: "Subject is Out of Office",            action: "Skip Inbox, Auto-Reply",   enabled: true  },
            MailFilter { name: "Conference",     condition: "Subject contains conference|summit",  action: "Move to Work/Projects",    enabled: false },
        ];

        Self {
            skin: Skin::Eudora,
            last_mail_skin: Skin::Eudora,
            pending_skin: None,
            dirty: false,
            dark_mode: false,
            mailboxes,
            labels,
            messages,
            contacts,
            filters,
            selected_mailbox: "In".to_string(),
            selected_label: None,
            selected_in_msg_id: 1,
            selected_out_msg_id: 101,
            selected_contact_idx: 0,
            eudora_tab: EudoraTab::In,
            email_mode: EmailViewMode::Mail,
            compose_open: false,
            compose: ComposeDraft::default(),
            settings_category: "getting_started".to_string(),
            settings_status: "Ready — All 21 classic Eudora configuration panels loaded.".to_string(),
            status_left: "In — 7 messages, 3 unread".to_string(),
            status_center: "IMAP Idle".to_string(),
            status_right: "Connected to mail.example.com".to_string(),
        }
    }

    fn unread_count(&self, mailbox: &str) -> usize {
        self.messages
            .iter()
            .filter(|m| m.mailbox == mailbox && m.unread)
            .count()
    }

    fn visible_messages_for_mailbox(&self, mailbox: &str) -> Vec<&MailMessage> {
        if let Some(lbl) = &self.selected_label {
            self.messages
                .iter()
                .filter(|m| m.labels.iter().any(|l| l == lbl))
                .collect()
        } else if mailbox == "Starred" {
            self.messages.iter().filter(|m| m.flagged).collect()
        } else if mailbox == "Sent" {
            self.messages
                .iter()
                .filter(|m| m.mailbox == "Out" || m.mailbox == "Sent")
                .collect()
        } else {
            self.messages
                .iter()
                .filter(|m| m.mailbox == mailbox)
                .collect()
        }
    }

    fn ensure_valid_selection(&mut self) {
        let active_box = if self.skin == Skin::Eudora && self.eudora_tab == EudoraTab::Out {
            "Out".to_string()
        } else {
            self.selected_mailbox.clone()
        };

        let visible_ids: Vec<usize> = self
            .visible_messages_for_mailbox(&active_box)
            .into_iter()
            .map(|m| m.id)
            .collect();

        if active_box == "Out" {
            if !visible_ids.contains(&self.selected_out_msg_id) {
                if let Some(&first) = visible_ids.first() {
                    self.selected_out_msg_id = first;
                }
            }
        } else if !visible_ids.contains(&self.selected_in_msg_id) {
            if let Some(&first) = visible_ids.first() {
                self.selected_in_msg_id = first;
            }
        }
    }

    fn update_status_bar(&mut self) {
        match self.eudora_tab {
            EudoraTab::In => {
                let total = self.visible_messages_for_mailbox(&self.selected_mailbox).len();
                let unread = self.unread_count(&self.selected_mailbox);
                self.status_left = format!("{} — {} messages, {} unread", self.selected_mailbox, total, unread);
                self.status_center = "IMAP Idle".to_string();
                self.status_right = "Connected to mail.example.com".to_string();
            }
            EudoraTab::Out => {
                let total = self.visible_messages_for_mailbox("Out").len();
                self.status_left = format!("Out — {} messages", total);
                self.status_center = "SMTP Ready".to_string();
                self.status_right = "Connected to mail.example.com".to_string();
            }
            EudoraTab::Contacts => {
                self.status_left = format!("Contacts — {} people", self.contacts.len());
                self.status_center = "Address Book".to_string();
                self.status_right = String::new();
            }
            EudoraTab::Filters => {
                let active = self.filters.iter().filter(|f| f.enabled).count();
                self.status_left = format!("Filters — {} rules ({} active)", self.filters.len(), active);
                self.status_center = String::new();
                self.status_right = String::new();
            }
            EudoraTab::Compose => {
                self.status_left = "Composing message...".to_string();
                self.status_center = "Draft".to_string();
                self.status_right = String::new();
            }
        }
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn display_name(addr: &str) -> &str {
    if let Some(idx) = addr.find('<') {
        let name = addr[..idx].trim();
        if !name.is_empty() {
            return name;
        }
    }
    addr
}

fn build_contact_list_html(state: &AppState) -> String {
    let mut indexed: Vec<(usize, &Contact)> = state.contacts.iter().enumerate().collect();
    indexed.sort_by(|a, b| a.1.name.cmp(b.1.name));

    let mut list_html = String::new();
    let mut last_letter = '\0';

    for (idx, c) in indexed {
        let letter = c.name.chars().next().unwrap_or('?').to_ascii_uppercase();
        if letter != last_letter {
            list_html.push_str(&format!(
                r#"<div class="contact-section-hdr">{letter}</div>"#
            ));
            last_letter = letter;
        }
        let sel = if idx == state.selected_contact_idx {
            "selected"
        } else {
            ""
        };
        let online = idx % 3 == 0;
        let online_dot = if online {
            r#"<span class="online-dot"></span>"#
        } else {
            ""
        };
        let subtitle = match (c.title.is_empty(), c.company.is_empty()) {
            (false, false) => format!("{} • {}", escape_html(c.title), escape_html(c.company)),
            (false, true) => escape_html(c.title),
            (true, false) => escape_html(c.company),
            (true, true) => String::new(),
        };
        list_html.push_str(&format!(
            r#"<div class="contact-card {sel}" data-contact="{idx}">
  <div class="avatar" style="background:{color}">{initials}{online_dot}</div>
  <div class="contact-info">
    <div class="contact-name">{name}</div>
    <div class="contact-title">{subtitle}</div>
    <div class="contact-email">{email}</div>
  </div>
</div>"#,
            color = c.color,
            initials = escape_html(c.initials),
            name = escape_html(c.name),
            email = escape_html(c.email),
        ));
    }
    list_html
}

fn build_contact_detail_html(state: &AppState) -> String {
    let Some(c) = state.contacts.get(state.selected_contact_idx) else {
        return String::new();
    };
    let idx = state.selected_contact_idx;
    let online = idx % 3 == 0;
    let online_dot = if online {
        r#"<span class="online-dot"></span>"#
    } else {
        ""
    };
    let online_label = if online {
        r#"<div class="contact-detail-online">● Online</div>"#
    } else {
        ""
    };
    let subtitle = match (c.title.is_empty(), c.company.is_empty()) {
        (false, false) => format!("{} at {}", escape_html(c.title), escape_html(c.company)),
        (false, true) => escape_html(c.title),
        (true, false) => escape_html(c.company),
        (true, true) => String::new(),
    };
    let subtitle_html = if !subtitle.is_empty() {
        format!(r#"<div class="contact-detail-title">{subtitle}</div>"#)
    } else {
        String::new()
    };
    let company_row = if !c.company.is_empty() {
        format!(
            r#"<div class="contact-info-row">
        <span class="contact-info-icon">🏢</span>
        <div>
          <div class="contact-info-label">Company</div>
          <div class="contact-info-value">{company}</div>
        </div>
      </div>"#,
            company = escape_html(c.company)
        )
    } else {
        String::new()
    };

    // Find messages involving this contact, or fallback to recent messages
    let mut recent_rows = String::new();
    for msg in state
        .messages
        .iter()
        .filter(|m| m.from.contains(c.email) || m.to.contains(c.email))
        .take(3)
    {
        recent_rows.push_str(&format!(
            r#"<div class="recent-email-row">
          <div class="recent-email-subj">{subj}</div>
          <div class="recent-email-date">{date}</div>
        </div>"#,
            subj = escape_html(&msg.subject),
            date = escape_html(&msg.date),
        ));
    }
    if recent_rows.is_empty() {
        recent_rows.push_str(
            r#"<div class="recent-email-row">
          <div class="recent-email-subj">Re: Welcome to Eudora 2026!</div>
          <div class="recent-email-date">Mar 7, 2026</div>
        </div>
        <div class="recent-email-row">
          <div class="recent-email-subj">Dashboard redesign - timeline update</div>
          <div class="recent-email-date">Mar 6, 2026</div>
        </div>"#,
        );
    }

    format!(
        r#"<div class="contact-detail-card">
    <div class="contact-banner">
      <div class="contact-banner-inner" style="background:{color}"></div>
    </div>
    <div class="contact-avatar-area">
      <div class="contact-avatar-large" style="background:{color}">{initials}{online_dot}</div>
      <div class="contact-detail-name">{name}</div>
      {subtitle_html}
      {online_label}
    </div>
    <div class="contact-detail-body">
      <hr class="contact-detail-divider">
      <div class="contact-info-row">
        <span class="contact-info-icon">✉</span>
        <div>
          <div class="contact-info-label">Email</div>
          <div class="contact-info-value email">{email}</div>
        </div>
      </div>
      <div class="contact-info-row">
        <span class="contact-info-icon">📞</span>
        <div>
          <div class="contact-info-label">Phone</div>
          <div class="contact-info-value">{phone}</div>
        </div>
      </div>
      {company_row}
      <hr class="contact-detail-divider">
      <div class="contact-info-label">Notes</div>
      <div class="contact-notes-text">{notes}</div>
      <div class="contact-detail-actions">
        <div class="contact-action-btn primary" data-compose-to="{email}">✉ Email</div>
        <div class="contact-action-btn">📞 Call</div>
        <div class="contact-action-btn">✏ Edit</div>
      </div>
      <div class="recent-emails-section">
        <div class="recent-label">Recent Emails</div>
        {recent_rows}
      </div>
    </div>
  </div>"#,
        color = c.color,
        initials = escape_html(c.initials),
        name = escape_html(c.name),
        email = escape_html(c.email),
        phone = escape_html(c.phone),
        notes = escape_html(c.notes),
    )
}

fn set_inner_html_indexed(doc: &mut Document, id: u32, html: &str) {
    doc.set_inner_html(id, html);
    doc.rebuild_node_index();
}

// ============================================================================
// DOM Rendering: Eudora Skin
// ============================================================================

fn render_eudora_all(doc: &mut Document, state: &mut AppState) {
    state.ensure_valid_selection();
    state.update_status_bar();

    sync_dark_mode(doc, state.dark_mode);
    render_eudora_mailboxes(doc, state);
    render_eudora_tabs(doc, state);
    render_eudora_in_page(doc, state);
    render_eudora_out_page(doc, state);
    render_eudora_contacts(doc, state);
    render_eudora_filters(doc, state);
    render_eudora_compose(doc, state);
    render_eudora_statusbar(doc, state);
}

fn render_eudora_mailboxes(doc: &mut Document, state: &AppState) {
    let Some(tree) = doc.get_element_by_id("wazoo-tree") else {
        return;
    };
    let mut html = String::new();
    for mb in &state.mailboxes {
        let unread = state.unread_count(mb.name);
        let is_selected = match state.eudora_tab {
            EudoraTab::In => state.selected_mailbox == mb.name,
            EudoraTab::Out => mb.name == "Out",
            _ => false,
        };
        let mut classes = String::from("wazoo-item");
        if mb.indent == 1 {
            classes.push_str(" depth-1");
        } else if mb.indent >= 2 {
            classes.push_str(" depth-2");
        }
        if is_selected {
            classes.push_str(" selected");
        }

        let expand_html = if mb.has_children {
            r#"<span class="wazoo-expand">▾</span>"#
        } else {
            ""
        };
        let badge_html = if unread > 0 {
            format!(r#"<span class="wazoo-badge">{unread}</span>"#)
        } else {
            String::new()
        };

        html.push_str(&format!(
            r#"<div class="{classes}" data-mailbox="{name}">{expand_html}<span class="wazoo-icon">{icon}</span><span class="wazoo-name">{name}</span>{badge_html}</div>"#,
            name = escape_html(mb.name),
            icon = mb.icon,
        ));
        if mb.separator_after {
            html.push_str(r#"<div class="wazoo-item sep"></div>"#);
        }
    }
    set_inner_html_indexed(doc, tree, &html);
}

fn render_eudora_tabs(doc: &mut Document, state: &AppState) {
    let tab_ids = [
        ("tab-in", EudoraTab::In),
        ("tab-out", EudoraTab::Out),
        ("tab-contacts", EudoraTab::Contacts),
        ("tab-filters", EudoraTab::Filters),
        ("compose-tab", EudoraTab::Compose),
    ];
    for (id, tab) in tab_ids {
        if let Some(el) = doc.get_element_by_id(id) {
            if state.eudora_tab == tab {
                doc.class_list_add(el, "selected");
            } else {
                doc.class_list_remove(el, "selected");
            }
        }
    }

    if let Some(tab_in) = doc.get_element_by_id("tab-in") {
        let icon = state
            .mailboxes
            .iter()
            .find(|m| m.name == state.selected_mailbox)
            .map(|m| m.icon)
            .unwrap_or("📥");
        let label_name = if state.selected_mailbox == "Out" {
            "In"
        } else {
            state.selected_mailbox.as_str()
        };
        doc.set_text_content(tab_in, &format!("{icon} {label_name}"));
    }

    if let Some(comp_tab) = doc.get_element_by_id("compose-tab") {
        doc.set_style_property(
            comp_tab,
            "display",
            if state.compose_open { "flex" } else { "none" },
        );
    }
    if let Some(comp_lbl) = doc.get_element_by_id("compose-tab-label") {
        doc.set_text_content(comp_lbl, &state.compose.title);
    }

    let page_ids = [
        ("page-in", EudoraTab::In),
        ("page-out", EudoraTab::Out),
        ("page-contacts", EudoraTab::Contacts),
        ("page-filters", EudoraTab::Filters),
        ("page-compose", EudoraTab::Compose),
    ];
    for (id, tab) in page_ids {
        if let Some(el) = doc.get_element_by_id(id) {
            if state.eudora_tab == tab {
                doc.class_list_add(el, "active");
            } else {
                doc.class_list_remove(el, "active");
            }
        }
    }
}

fn render_eudora_rows_dom(
    doc: &mut Document,
    tbody_id: &str,
    messages: &[&MailMessage],
    is_out: bool,
    selected_id: usize,
) {
    let Some(tbody) = doc.get_element_by_id(tbody_id) else {
        return;
    };
    doc.set_text_content(tbody, "");

    for msg in messages {
        let tr = doc.create_element("tr");
        let is_sel = msg.id == selected_id;
        let mut row_cls = String::new();
        if is_sel {
            row_cls.push_str("selected");
        } else if msg.unread {
            row_cls.push_str("unread");
        }
        if !row_cls.is_empty() {
            doc.set_attribute(tr, "class", &row_cls);
        }
        doc.set_attribute(tr, "data-msg-id", &msg.id.to_string());
        doc.set_attribute(tr, "data-box", if is_out { "out" } else { "in" });

        // 1. Status cell
        let td_s = doc.create_element("td");
        doc.set_attribute(td_s, "class", "col-s");
        if msg.unread {
            let dot = doc.create_element("span");
            doc.set_attribute(dot, "class", "dot-unread");
            doc.append_child(td_s, dot);
        } else if msg.replied {
            doc.set_text_content(td_s, "↩");
        } else if msg.forwarded {
            doc.set_text_content(td_s, "→");
        }
        doc.append_child(tr, td_s);

        // 2. Priority cell
        let td_p = doc.create_element("td");
        doc.set_attribute(td_p, "class", "col-p");
        match msg.priority {
            "highest" => {
                let s = doc.create_element("span");
                doc.set_attribute(s, "class", "pri-highest");
                doc.set_text_content(s, "▲▲");
                doc.append_child(td_p, s);
            }
            "high" => {
                let s = doc.create_element("span");
                doc.set_attribute(s, "class", "pri-high");
                doc.set_text_content(s, "▲");
                doc.append_child(td_p, s);
            }
            _ => {}
        }
        doc.append_child(tr, td_p);

        // 3. Attachment cell
        let td_a = doc.create_element("td");
        doc.set_attribute(td_a, "class", "col-a");
        if msg.has_attach {
            doc.set_text_content(td_a, "📎");
        }
        doc.append_child(tr, td_a);

        // 4. Flag cell
        let td_f = doc.create_element("td");
        doc.set_attribute(td_f, "class", "col-f");
        if msg.flagged {
            doc.set_attribute(td_f, "style", "color:#d23720");
            doc.set_text_content(td_f, "⚑");
        }
        doc.append_child(tr, td_f);

        // 5. Who cell
        let td_who = doc.create_element("td");
        doc.set_attribute(td_who, "class", "col-who");
        let who = if is_out {
            display_name(&msg.to)
        } else {
            display_name(&msg.from)
        };
        doc.set_text_content(td_who, who);
        doc.append_child(tr, td_who);

        // 6. Date cell
        let td_date = doc.create_element("td");
        doc.set_attribute(td_date, "class", "col-date");
        doc.set_text_content(td_date, &msg.date);
        doc.append_child(tr, td_date);

        // 7. Size cell
        let td_size = doc.create_element("td");
        doc.set_attribute(td_size, "class", "col-size");
        doc.set_text_content(td_size, &msg.size);
        doc.append_child(tr, td_size);

        // 8. Subject cell
        let td_subj = doc.create_element("td");
        doc.set_attribute(td_subj, "class", "col-subj");
        doc.set_text_content(td_subj, &msg.subject);
        doc.append_child(tr, td_subj);

        doc.append_child(tbody, tr);
    }

    doc.rebuild_node_index();
}

fn render_eudora_in_page(doc: &mut Document, state: &AppState) {
    let mailbox = if state.selected_mailbox == "Out" {
        "In"
    } else {
        state.selected_mailbox.as_str()
    };
    let visible = state.visible_messages_for_mailbox(mailbox);

    render_eudora_rows_dom(doc, "inbox-rows", &visible, false, state.selected_in_msg_id);

    let selected = visible
        .iter()
        .copied()
        .find(|m| m.id == state.selected_in_msg_id)
        .or_else(|| visible.first().copied());

    if let Some(msg) = selected {
        set_text_by_id(doc, "in-from", &msg.from);
        set_text_by_id(doc, "in-to", &msg.to);
        if let Some(cc_el) = doc.get_element_by_id("in-cc-area") {
            if msg.cc.is_empty() {
                set_inner_html_indexed(doc, cc_el, "");
            } else {
                set_inner_html_indexed(
                    doc,
                    cc_el,
                    &format!(
                        r#" &nbsp; <span class="lbl">Cc:</span> {}"#,
                        escape_html(&msg.cc)
                    ),
                );
            }
        }
        set_text_by_id(doc, "in-subj", &msg.subject);
        let mut meta = format!("Date: {}    Size: {}", msg.date, msg.size);
        if msg.priority == "highest" || msg.priority == "high" {
            meta.push_str("    Priority: HIGH");
        }
        if !msg.attachment_note.is_empty() {
            meta.push_str(&format!("    {}", msg.attachment_note));
        } else if msg.has_attach {
            meta.push_str("    📎 Attachment");
        }
        set_text_by_id(doc, "in-meta", &meta);
        if let Some(body_el) = doc.get_element_by_id("in-body") {
            set_inner_html_indexed(doc, body_el, &msg.body_html);
        }
    } else {
        set_text_by_id(doc, "in-from", "—");
        set_text_by_id(doc, "in-to", "—");
        set_text_by_id(doc, "in-subj", &format!("No messages in {}", mailbox));
        set_text_by_id(doc, "in-meta", "");
        if let Some(body_el) = doc.get_element_by_id("in-body") {
            set_inner_html_indexed(doc, body_el, "<p>This mailbox is empty.</p>");
        }
    }
}

fn render_eudora_out_page(doc: &mut Document, state: &AppState) {
    let visible = state.visible_messages_for_mailbox("Out");
    render_eudora_rows_dom(doc, "outbox-rows", &visible, true, state.selected_out_msg_id);

    let selected = visible
        .iter()
        .copied()
        .find(|m| m.id == state.selected_out_msg_id)
        .or_else(|| visible.first().copied());

    if let Some(msg) = selected {
        set_text_by_id(doc, "out-from", &msg.from);
        set_text_by_id(doc, "out-to", &msg.to);
        if let Some(cc_el) = doc.get_element_by_id("out-cc-area") {
            if msg.cc.is_empty() {
                set_inner_html_indexed(doc, cc_el, "");
            } else {
                set_inner_html_indexed(
                    doc,
                    cc_el,
                    &format!(
                        r#" &nbsp; <span class="lbl">Cc:</span> {}"#,
                        escape_html(&msg.cc)
                    ),
                );
            }
        }
        set_text_by_id(doc, "out-subj", &msg.subject);
        set_text_by_id(
            doc,
            "out-meta",
            &format!("Date: {}    Size: {}", msg.date, msg.size),
        );
        if let Some(body_el) = doc.get_element_by_id("out-body") {
            set_inner_html_indexed(doc, body_el, &msg.body_html);
        }
    }
}

fn render_eudora_contacts(doc: &mut Document, state: &AppState) {
    if let Some(list_el) = doc.get_element_by_id("contact-list-inner") {
        set_inner_html_indexed(doc, list_el, &build_contact_list_html(state));
    }
    if let Some(detail_el) = doc.get_element_by_id("contact-detail-inner") {
        set_inner_html_indexed(doc, detail_el, &build_contact_detail_html(state));
    }
}

fn render_eudora_filters(doc: &mut Document, state: &AppState) {
    let active_count = state.filters.iter().filter(|f| f.enabled).count();
    let disabled_count = state.filters.len().saturating_sub(active_count);
    set_text_by_id(
        doc,
        "filters-meta",
        &format!(
            "{} rules • {} active • {} disabled",
            state.filters.len(),
            active_count,
            disabled_count
        ),
    );

    if let Some(cards_el) = doc.get_element_by_id("filter-cards") {
        let mut html = String::new();
        for (idx, f) in state.filters.iter().enumerate() {
            let status_cls = if f.enabled { "active" } else { "inactive" };
            let track_cls = if f.enabled { "on" } else { "off" };
            let text_cls = if f.enabled { "" } else { "inactive" };
            let is_junk = f.action.contains("Junk") || f.action.contains("Skip");
            let then_cls = if is_junk { "junk" } else { "" };
            let pill_text = if f.enabled { "Active" } else { "Off" };

            html.push_str(&format!(
                r#"<div class="filter-card" data-filter="{idx}">
  <div class="filter-status-bar {status_cls}"></div>
  <div class="filter-toggle">
    <div class="toggle-track {track_cls}" data-filter="{idx}">
      <div class="toggle-knob"></div>
    </div>
  </div>
  <div class="filter-body">
    <div>
      <span class="filter-num">#{num}</span>
      <span class="filter-name {text_cls}">{name}</span>
    </div>
    <div class="filter-row">
      <span class="filter-pill if">IF</span>
      <span class="filter-text {text_cls}">{cond}</span>
    </div>
    <div class="filter-arrow">↓</div>
    <div class="filter-row">
      <span class="filter-pill then {then_cls}">THEN</span>
      <span class="filter-action-text {text_cls}">{action}</span>
    </div>
  </div>
  <span class="filter-status-pill {status_cls}">{pill_text}</span>
</div>"#,
                num = idx + 1,
                name = escape_html(f.name),
                cond = escape_html(f.condition),
                action = escape_html(f.action),
            ));
        }
        set_inner_html_indexed(doc, cards_el, &html);
    }
}

fn render_eudora_compose(doc: &mut Document, state: &AppState) {
    set_text_by_id(doc, "compose-from", &state.compose.from);
    if let Some(el) = doc.get_element_by_id("compose-to") {
        doc.set_attribute(el, "value", &state.compose.to);
    }
    if let Some(el) = doc.get_element_by_id("compose-cc") {
        doc.set_attribute(el, "value", &state.compose.cc);
    }
    if let Some(el) = doc.get_element_by_id("compose-bcc") {
        doc.set_attribute(el, "value", &state.compose.bcc);
    }
    if let Some(el) = doc.get_element_by_id("compose-subj") {
        doc.set_attribute(el, "value", &state.compose.subject);
    }
    set_text_by_id(doc, "compose-attach", &state.compose.attach);
    set_text_by_id(doc, "compose-priority", &state.compose.priority);
    if let Some(body) = doc.get_element_by_id("compose-body") {
        set_inner_html_indexed(doc, body, &state.compose.body_html);
    }
}

fn render_eudora_statusbar(doc: &mut Document, state: &AppState) {
    set_text_by_id(doc, "sb-left", &state.status_left);
    set_text_by_id(doc, "sb-center", &state.status_center);
    set_text_by_id(doc, "sb-right", &state.status_right);
}

// ============================================================================
// DOM Rendering: Email (Flux Mail) Skin
// ============================================================================

fn render_email_all(doc: &mut Document, state: &mut AppState) {
    state.ensure_valid_selection();

    sync_dark_mode(doc, state.dark_mode);
    render_email_sidebar(doc, state);
    render_email_center_list(doc, state);
    render_email_right_pane(doc, state);
}

fn render_email_sidebar(doc: &mut Document, state: &AppState) {
    if let Some(mb_container) = doc.get_element_by_id("sidebar-mailboxes") {
        let mut html = String::new();

        for mb in state.mailboxes.iter().filter(|m| m.show_in_email_skin) {
            let unread = state.unread_count(mb.name);
            let is_active = state.email_mode == EmailViewMode::Mail
                && state.selected_label.is_none()
                && state.selected_mailbox == mb.name;
            let active_cls = if is_active { "active" } else { "" };
            let badge = if unread > 0 {
                format!(r#"<span class="badge">{unread}</span>"#)
            } else {
                String::new()
            };

            html.push_str(&format!(
                r#"<div class="sidebar-item {active_cls}" data-mailbox="{name}">
  <span class="sidebar-icon">{icon}</span>
  <span class="sidebar-label">{label}</span>
  {badge}
</div>"#,
                name = escape_html(mb.name),
                icon = mb.icon,
                label = escape_html(mb.email_label),
            ));

            if mb.name == "In" {
                let starred_active = state.email_mode == EmailViewMode::Mail
                    && state.selected_label.is_none()
                    && state.selected_mailbox == "Starred";
                let s_cls = if starred_active { "active" } else { "" };
                html.push_str(&format!(
                    r#"<div class="sidebar-item {s_cls}" data-mailbox="Starred">
  <span class="sidebar-icon">★</span>
  <span class="sidebar-label">Starred</span>
</div>"#
                ));
            }
        }

        // Contacts item in sidebar
        let contacts_active = if state.email_mode == EmailViewMode::Contacts {
            "active"
        } else {
            ""
        };
        html.push_str(&format!(
            r#"<div class="sidebar-item {contacts_active}" data-view="contacts">
  <span class="sidebar-icon">👥</span>
  <span class="sidebar-label">Contacts</span>
  <span class="badge">{count}</span>
</div>"#,
            count = state.contacts.len()
        ));

        // Settings item in sidebar
        html.push_str(
            r#"<div class="sidebar-item" data-action="open-settings">
  <span class="sidebar-icon">⚙</span>
  <span class="sidebar-label">Settings</span>
</div>"#,
        );

        set_inner_html_indexed(doc, mb_container, &html);
    }

    if let Some(lbl_container) = doc.get_element_by_id("sidebar-labels") {
        let mut html = String::new();
        for lbl in &state.labels {
            let is_active = state.email_mode == EmailViewMode::Mail
                && state.selected_label.as_deref() == Some(lbl.name);
            let active_cls = if is_active { "active" } else { "" };
            html.push_str(&format!(
                r#"<div class="label-item {active_cls}" data-label="{name}">
  <span class="label-dot" style="background:{color};"></span>{name}
</div>"#,
                name = escape_html(lbl.name),
                color = lbl.color,
            ));
        }
        set_inner_html_indexed(doc, lbl_container, &html);
    }
}

fn render_email_center_list(doc: &mut Document, state: &AppState) {
    let Some(list_el) = doc.get_element_by_id("email-list") else {
        return;
    };

    if state.email_mode == EmailViewMode::Contacts {
        set_inner_html_indexed(doc, list_el, &build_contact_list_html(state));
        return;
    }

    let visible = state.visible_messages_for_mailbox(&state.selected_mailbox);
    let active_id = if state.selected_mailbox == "Out" {
        state.selected_out_msg_id
    } else {
        state.selected_in_msg_id
    };

    if visible.is_empty() {
        set_inner_html_indexed(
            doc,
            list_el,
            r#"<div class="email-item selected"><div class="email-item-inner"><div class="email-subject">No messages in this view</div></div></div>"#,
        );
        return;
    }

    let mut html = String::new();
    for (idx, msg) in visible.iter().enumerate() {
        let is_sel = msg.id == active_id;
        let sel_cls = if is_sel { "selected" } else { "" };
        let unread_cls = if msg.unread { "unread" } else { "" };
        let unread_dot = if msg.unread {
            r#"<span class="unread-dot"></span>"#
        } else {
            ""
        };
        let pri_bar = match msg.priority {
            "highest" | "high" => r#"<span class="priority-bar priority-high"></span>"#,
            "low" => r#"<span class="priority-bar priority-low"></span>"#,
            _ => "",
        };
        let star_html = if msg.flagged {
            r#"<span class="star-icon">★</span>"#
        } else {
            ""
        };
        let attach_html = if msg.has_attach {
            r#"<span class="attach-icon" title="Has attachment">📎</span>"#
        } else {
            ""
        };

        let mut pills_html = String::new();
        for &lbl_name in &msg.labels {
            if let Some(def) = state.labels.iter().find(|d| d.name == lbl_name) {
                pills_html.push_str(&format!(
                    r#"<span class="pill" style="background:{bg};color:{fg};">{name}</span>"#,
                    bg = def.bg_rgba,
                    fg = def.color,
                    name = escape_html(def.name),
                ));
            }
        }

        let who = if msg.mailbox == "Out" {
            display_name(&msg.to)
        } else {
            display_name(&msg.from)
        };

        html.push_str(&format!(
            r#"<div class="email-item {sel_cls}" data-msg-id="{id}" data-box="{box_kind}">
  {pri_bar}
  <div class="avatar" style="background:{color};">{initials}</div>
  <div class="email-item-inner">
    {unread_dot}
    <div class="email-row1">
      <span class="email-sender {unread_cls}">{who}</span>
      {star_html}
      {attach_html}
      <span class="email-date">{short_date}</span>
    </div>
    <div class="email-row2">
      <span class="email-subject {unread_cls}">{subj}</span>
      {pills_html}
    </div>
    <div class="email-preview">{preview}</div>
  </div>
</div>"#,
            id = msg.id,
            box_kind = if msg.mailbox == "Out" { "out" } else { "in" },
            color = msg.avatar_color,
            initials = escape_html(&msg.avatar_initials),
            who = escape_html(who),
            short_date = escape_html(&msg.short_date),
            subj = escape_html(&msg.subject),
            preview = escape_html(&msg.preview),
        ));

        if idx + 1 < visible.len() {
            html.push_str(r#"<div class="email-divider"></div>"#);
        }
    }

    set_inner_html_indexed(doc, list_el, &html);
}

fn render_email_right_pane(doc: &mut Document, state: &AppState) {
    let email_view = doc.get_element_by_id("email-view");
    let contact_view = doc.get_element_by_id("contact-view");
    let compose_view = doc.get_element_by_id("compose-view");

    match state.email_mode {
        EmailViewMode::Contacts => {
            if let Some(el) = email_view {
                doc.set_style_property(el, "display", "none");
            }
            if let Some(el) = compose_view {
                doc.set_style_property(el, "display", "none");
            }
            if let Some(el) = contact_view {
                doc.set_style_property(el, "display", "block");
                set_inner_html_indexed(doc, el, &build_contact_detail_html(state));
            }
            return;
        }
        EmailViewMode::Compose => {
            if let Some(el) = email_view {
                doc.set_style_property(el, "display", "none");
            }
            if let Some(el) = contact_view {
                doc.set_style_property(el, "display", "none");
            }
            if let Some(el) = compose_view {
                doc.set_style_property(el, "display", "block");
                let compose_html = format!(
                    r#"<div class="compose-card">
  <div class="compose-card-title">{title}</div>
  <div class="compose-field-row">
    <span class="compose-field-label">From:</span>
    <span>{from}</span>
  </div>
  <div class="compose-field-row">
    <span class="compose-field-label">To:</span>
    <input class="compose-field-input" type="text" value="{to}">
  </div>
  <div class="compose-field-row">
    <span class="compose-field-label">Cc:</span>
    <input class="compose-field-input" type="text" value="{cc}">
  </div>
  <div class="compose-field-row">
    <span class="compose-field-label">Subject:</span>
    <input class="compose-field-input" type="text" value="{subj}">
  </div>
  <div class="compose-body-area">{body}</div>
  <div class="compose-actions">
    <button class="btn btn-primary" data-action="send">✉ Send</button>
    <button class="btn btn-outlined" data-action="discard">Discard</button>
  </div>
</div>"#,
                    title = escape_html(&state.compose.title),
                    from = escape_html(&state.compose.from),
                    to = escape_html(&state.compose.to),
                    cc = escape_html(&state.compose.cc),
                    subj = escape_html(&state.compose.subject),
                    body = state.compose.body_html,
                );
                set_inner_html_indexed(doc, el, &compose_html);
            }
            return;
        }
        EmailViewMode::Mail => {
            if let Some(el) = email_view {
                doc.set_style_property(el, "display", "block");
            }
            if let Some(el) = contact_view {
                doc.set_style_property(el, "display", "none");
            }
            if let Some(el) = compose_view {
                doc.set_style_property(el, "display", "none");
            }
        }
    }

    let visible = state.visible_messages_for_mailbox(&state.selected_mailbox);
    let active_id = if state.selected_mailbox == "Out" {
        state.selected_out_msg_id
    } else {
        state.selected_in_msg_id
    };

    let selected = visible
        .iter()
        .copied()
        .find(|m| m.id == active_id)
        .or_else(|| visible.first().copied());

    let Some(msg) = selected else {
        if let Some(badge) = doc.get_element_by_id("email-priority-badge") {
            doc.set_style_property(badge, "display", "none");
        }
        set_text_by_id(doc, "email-subject-title", "No message selected");
        set_text_by_id(doc, "email-meta", "");
        if let Some(lbls) = doc.get_element_by_id("email-labels") {
            set_inner_html_indexed(doc, lbls, "");
        }
        if let Some(att) = doc.get_element_by_id("email-attachment-note") {
            doc.set_style_property(att, "display", "none");
        }
        if let Some(body) = doc.get_element_by_id("email-body") {
            set_inner_html_indexed(doc, body, "<p>Select a mailbox or message to read it.</p>");
        }
        return;
    };

    if let Some(badge) = doc.get_element_by_id("email-priority-badge") {
        match msg.priority {
            "highest" | "high" => {
                doc.set_style_property(badge, "display", "block");
                doc.set_text_content(badge, "⚡ HIGH PRIORITY");
            }
            "low" => {
                doc.set_style_property(badge, "display", "block");
                doc.set_text_content(badge, "LOW PRIORITY");
            }
            _ => {
                doc.set_style_property(badge, "display", "none");
            }
        }
    }

    set_text_by_id(doc, "email-subject-title", &msg.subject);
    set_text_by_id(
        doc,
        "email-meta",
        &format!("{} · to {} · {}", msg.from, msg.to, msg.date),
    );

    if let Some(lbls) = doc.get_element_by_id("email-labels") {
        let mut pills = String::new();
        for &lbl_name in &msg.labels {
            if let Some(def) = state.labels.iter().find(|d| d.name == lbl_name) {
                pills.push_str(&format!(
                    r#"<span class="pill" style="background:{bg};color:{fg};">{name}</span>"#,
                    bg = def.bg_rgba,
                    fg = def.color,
                    name = escape_html(def.name),
                ));
            }
        }
        set_inner_html_indexed(doc, lbls, &pills);
    }

    if let Some(att) = doc.get_element_by_id("email-attachment-note") {
        if !msg.attachment_note.is_empty() {
            doc.set_style_property(att, "display", "block");
            doc.set_text_content(att, &msg.attachment_note);
        } else {
            doc.set_style_property(att, "display", "none");
        }
    }

    if let Some(body) = doc.get_element_by_id("email-body") {
        set_inner_html_indexed(doc, body, &msg.body_html);
    }
}

// ============================================================================
// DOM Rendering: Classic Eudora Settings Page (21 Categories)
// ============================================================================

const SETTINGS_CATEGORIES: &[(&str, &str, &str, &str)] = &[
    ("getting_started", "🏠", "Getting Started", "Configure basic incoming/outgoing servers and primary identity"),
    ("checking_mail", "📥", "Checking Mail", "POP/IMAP retrieval, polling interval, and server message retention"),
    ("sending_mail", "📤", "Sending Mail", "SMTP server, port 587 submission, and outgoing queue options"),
    ("composing", "✏️", "Composing", "Nickname auto-completion, word wrap column, and encoding"),
    ("mailbox_display", "📋", "Mailbox Display", "Table columns, separator lines, preview pane, and mark-as-read"),
    ("date_display", "📅", "Date Display", "Age-sensitive relative timestamps and timezone formatting"),
    ("styled_text", "🅱", "Styled Text", "HTML/styled composition rules and formatting toolbar preferences"),
    ("fonts", "🔤", "Fonts & Display", "Screen, fixed-width, and print font families, sizes, and graphics"),
    ("labels", "🏷", "Labels", "Customize the 7 classic Eudora message labels and color swatches"),
    ("attachments", "📎", "Attachments", "MIME/BinHex/UUencode encoding and automatic download directory"),
    ("replying", "↩", "Replying", "Reply-to-All behavior, self-inclusion, and priority inheritance"),
    ("junk_mail", "🛡", "Junk Mail", "SpamWatch threshold score, Address Book whitelist, and auto-trim"),
    ("toolbar", "🧰", "Toolbar", "Main toolbar visibility, floating search field, and icon style"),
    ("getting_attention", "🔔", "Getting Attention", "New mail fanfare sound, dock bounce, and background task progress"),
    ("hosts", "🌐", "Hosts & Proxies", "Ph/LDAP/Finger directory servers, SOCKS/HTTP proxy, and offline mode"),
    ("moving_around", "⌨️", "Moving Around", "Next-message selection after delete/transfer and keyboard navigation"),
    ("extra_warnings", "⚠️", "Extra Warnings", "Confirmation prompts for deleting, queueing, and emptying Trash"),
    ("miscellaneous", "🔧", "Miscellaneous", "Window closing, Turbo Redirect, TOC format, and system keychain"),
    ("accounts", "👤", "Accounts & Personalities", "Manage Dominant and secondary Eudora mail personalities"),
    ("ssl", "🔒", "SSL / TLS", "Configure STARTTLS and alternate TLS ports for POP, SMTP, and IMAP"),
    ("spell_checking", "📝", "Spell Checking", "Real-time spell checker, ignore rules, and phonetic suggestions"),
];

fn render_settings_all(doc: &mut Document, state: &mut AppState) {
    sync_dark_mode(doc, state.dark_mode);

    let active_cat = state.settings_category.as_str();

    // Update sidebar items
    for item in doc.query_selector_all("[data-settings-cat]") {
        let is_match = doc.get_attribute(item, "data-settings-cat").as_deref() == Some(active_cat);
        if is_match {
            doc.class_list_add(item, "active");
        } else {
            doc.class_list_remove(item, "active");
        }
    }

    // Update panels and header metadata
    for (idx, &(cat_id, ico, title, sub)) in SETTINGS_CATEGORIES.iter().enumerate() {
        let panel_id = format!("panel-{cat_id}");
        if let Some(panel) = doc.get_element_by_id(&panel_id) {
            if cat_id == active_cat {
                doc.class_list_add(panel, "active");
            } else {
                doc.class_list_remove(panel, "active");
            }
        }
        if cat_id == active_cat {
            set_text_by_id(doc, "settings-cat-ico", ico);
            set_text_by_id(doc, "settings-cat-title", title);
            set_text_by_id(doc, "settings-cat-sub", sub);
            set_text_by_id(
                doc,
                "settings-cat-badge",
                &format!("Category {} of {}", idx + 1, SETTINGS_CATEGORIES.len()),
            );
        }
    }

    set_text_by_id(doc, "settings-status-msg", &state.settings_status);
}

fn sync_dark_mode(doc: &mut Document, dark: bool) {
    let root_id = doc.root.node_id;
    if dark {
        doc.class_list_add(root_id, "dark");
    } else {
        doc.class_list_remove(root_id, "dark");
    }
    if let Some(html_el) = doc.query_selector("html") {
        if dark {
            doc.class_list_add(html_el, "dark");
        } else {
            doc.class_list_remove(html_el, "dark");
        }
    }
    if let Some(body_el) = doc.query_selector("body") {
        if dark {
            doc.class_list_add(body_el, "dark");
        } else {
            doc.class_list_remove(body_el, "dark");
        }
    }
    if let Some(btn) = doc.get_element_by_id("btn-dark") {
        doc.set_text_content(btn, if dark { "☀" } else { "☽" });
    }
}

fn set_text_by_id(doc: &mut Document, id: &str, text: &str) {
    if let Some(el) = doc.get_element_by_id(id) {
        doc.set_text_content(el, text);
    }
}

fn render_active_skin(doc: &mut Document, state: &mut AppState) {
    match state.skin {
        Skin::Eudora => render_eudora_all(doc, state),
        Skin::Email => render_email_all(doc, state),
        Skin::Settings => render_settings_all(doc, state),
    }
    doc.style_dirty = true;
    doc.layout_generation = doc.layout_generation.wrapping_add(1);
    state.dirty = true;
}

// ============================================================================
// Event Handling
// ============================================================================

fn register_listeners(doc: &mut Document, state: Arc<Mutex<AppState>>) {
    let root = doc.root.node_id;
    doc.add_event_listener(
        root,
        "click",
        Box::new(move |evt, doc| {
            let mut st = state.lock().unwrap();
            handle_click(doc, evt.target, &mut st);
        }),
        ListenerOptions::default(),
    );
}

fn handle_click(doc: &mut Document, target: u32, state: &mut AppState) {
    // 0. Check settings category sidebar selection
    if let Some(cat_el) = doc.closest(target, "[data-settings-cat]") {
        if let Some(cat) = doc.get_attribute(cat_el, "data-settings-cat") {
            state.settings_category = cat;
            render_active_skin(doc, state);
            return;
        }
    }

    // 1. Check close button on Compose tab first
    if doc.closest(target, ".tab-close").is_some() {
        state.compose_open = false;
        state.eudora_tab = EudoraTab::In;
        render_active_skin(doc, state);
        return;
    }

    // 2. Check action buttons (toolbar / compose / skin switch / settings / dark mode)
    if let Some(btn) = doc.closest(target, "[data-action]") {
        if let Some(action) = doc.get_attribute(btn, "data-action") {
            match action.as_str() {
                "open-settings" => {
                    if state.skin != Skin::Settings {
                        state.last_mail_skin = state.skin;
                    }
                    state.pending_skin = Some(Skin::Settings);
                    return;
                }
                "close-settings" => {
                    state.pending_skin = Some(state.last_mail_skin);
                    return;
                }
                "save-settings" => {
                    state.settings_status = "✓ Settings saved to Eudora configuration.".to_string();
                    state.status_left = "Settings saved".to_string();
                    state.pending_skin = Some(state.last_mail_skin);
                    return;
                }
                "reset-settings" => {
                    state.settings_status = "↺ Restored default settings for current category.".to_string();
                    render_active_skin(doc, state);
                    return;
                }
                "switch-skin" => {
                    let next = match state.skin {
                        Skin::Eudora => Skin::Email,
                        Skin::Email => Skin::Eudora,
                        Skin::Settings => match state.last_mail_skin {
                            Skin::Eudora => Skin::Email,
                            _ => Skin::Eudora,
                        },
                    };
                    state.last_mail_skin = next;
                    state.pending_skin = Some(next);
                    return;
                }
                "toggle-dark" => {
                    state.dark_mode = !state.dark_mode;
                    render_active_skin(doc, state);
                    return;
                }
                "check-mail" => {
                    let unread = state.unread_count("In");
                    state.status_left = format!("Checked mail — {} unread in In", unread);
                    state.status_center = "Up to date".to_string();
                    render_active_skin(doc, state);
                    return;
                }
                "new-message" => {
                    state.compose = ComposeDraft::default();
                    state.compose_open = true;
                    state.eudora_tab = EudoraTab::Compose;
                    state.email_mode = EmailViewMode::Compose;
                    render_active_skin(doc, state);
                    return;
                }
                "reply" | "reply-all" => {
                    let active_id = if state.skin == Skin::Eudora && state.eudora_tab == EudoraTab::Out {
                        state.selected_out_msg_id
                    } else {
                        state.selected_in_msg_id
                    };
                    if let Some(msg) = state.messages.iter().find(|m| m.id == active_id).cloned() {
                        let subj = if msg.subject.starts_with("Re: ") {
                            msg.subject.clone()
                        } else {
                            format!("Re: {}", msg.subject)
                        };
                        state.compose = ComposeDraft {
                            title: "↩ Reply".to_string(),
                            from: "You <you@example.com>".to_string(),
                            to: msg.from.clone(),
                            cc: if action == "reply-all" { msg.cc.clone() } else { String::new() },
                            bcc: String::new(),
                            subject: subj,
                            attach: "None".to_string(),
                            priority: if msg.priority == "high" || msg.priority == "highest" {
                                "High".to_string()
                            } else {
                                "Normal".to_string()
                            },
                            body_html: format!(
                                r#"<p><br></p><p>At {date}, {from} wrote:</p><blockquote>{body}</blockquote>"#,
                                date = escape_html(&msg.date),
                                from = escape_html(&msg.from),
                                body = msg.body_html,
                            ),
                        };
                        state.compose_open = true;
                        state.eudora_tab = EudoraTab::Compose;
                        state.email_mode = EmailViewMode::Compose;
                        render_active_skin(doc, state);
                    }
                    return;
                }
                "forward" => {
                    let active_id = state.selected_in_msg_id;
                    if let Some(msg) = state.messages.iter().find(|m| m.id == active_id).cloned() {
                        let subj = if msg.subject.starts_with("Fwd: ") {
                            msg.subject.clone()
                        } else {
                            format!("Fwd: {}", msg.subject)
                        };
                        state.compose = ComposeDraft {
                            title: "→ Forward".to_string(),
                            from: "You <you@example.com>".to_string(),
                            to: String::new(),
                            cc: String::new(),
                            bcc: String::new(),
                            subject: subj,
                            attach: if msg.attachment_note.is_empty() {
                                "None".to_string()
                            } else {
                                msg.attachment_note.clone()
                            },
                            priority: "Normal".to_string(),
                            body_html: format!(
                                r#"<p><br></p><p>-------- Forwarded Message --------<br>From: {from}<br>Date: {date}</p><blockquote>{body}</blockquote>"#,
                                from = escape_html(&msg.from),
                                date = escape_html(&msg.date),
                                body = msg.body_html,
                            ),
                        };
                        state.compose_open = true;
                        state.eudora_tab = EudoraTab::Compose;
                        state.email_mode = EmailViewMode::Compose;
                        render_active_skin(doc, state);
                    }
                    return;
                }
                "trash" => {
                    let active_id = state.selected_in_msg_id;
                    if let Some(msg) = state.messages.iter_mut().find(|m| m.id == active_id) {
                        msg.mailbox = "Trash".to_string();
                    }
                    render_active_skin(doc, state);
                    return;
                }
                "archive" => {
                    let active_id = state.selected_in_msg_id;
                    if let Some(msg) = state.messages.iter_mut().find(|m| m.id == active_id) {
                        msg.mailbox = "Archive".to_string();
                    }
                    render_active_skin(doc, state);
                    return;
                }
                "junk" => {
                    let active_id = state.selected_in_msg_id;
                    if let Some(msg) = state.messages.iter_mut().find(|m| m.id == active_id) {
                        msg.mailbox = "Spam".to_string();
                    }
                    render_active_skin(doc, state);
                    return;
                }
                "star" => {
                    let active_id = state.selected_in_msg_id;
                    if let Some(msg) = state.messages.iter_mut().find(|m| m.id == active_id) {
                        msg.flagged = !msg.flagged;
                    }
                    render_active_skin(doc, state);
                    return;
                }
                "send" | "queue" => {
                    state.compose_open = false;
                    state.eudora_tab = EudoraTab::Out;
                    state.email_mode = EmailViewMode::Mail;
                    state.status_left = "Message queued/sent in Out".to_string();
                    render_active_skin(doc, state);
                    return;
                }
                "discard" => {
                    state.compose_open = false;
                    state.eudora_tab = EudoraTab::In;
                    state.email_mode = EmailViewMode::Mail;
                    render_active_skin(doc, state);
                    return;
                }
                _ => {}
            }
        }
    }

    // 3. Check Contact "Email" button in contact details
    if let Some(compose_btn) = doc.closest(target, "[data-compose-to]") {
        if let Some(email) = doc.get_attribute(compose_btn, "data-compose-to") {
            state.compose = ComposeDraft {
                title: format!("✏ To: {}", email),
                to: email,
                ..ComposeDraft::default()
            };
            state.compose_open = true;
            state.eudora_tab = EudoraTab::Compose;
            state.email_mode = EmailViewMode::Compose;
            render_active_skin(doc, state);
            return;
        }
    }

    // 4. Check Eudora top tab bar
    if let Some(tab_el) = doc.closest(target, "[data-tab]") {
        if let Some(tab_name) = doc.get_attribute(tab_el, "data-tab") {
            match tab_name.as_str() {
                "in" => {
                    state.eudora_tab = EudoraTab::In;
                    if state.selected_mailbox == "Out" {
                        state.selected_mailbox = "In".to_string();
                    }
                }
                "out" => {
                    state.eudora_tab = EudoraTab::Out;
                }
                "contacts" => {
                    state.eudora_tab = EudoraTab::Contacts;
                }
                "filters" => {
                    state.eudora_tab = EudoraTab::Filters;
                }
                "compose" => {
                    state.eudora_tab = EudoraTab::Compose;
                }
                _ => {}
            }
            render_active_skin(doc, state);
            return;
        }
    }

    // 5. Check Contacts view item in Email sidebar
    if let Some(view_el) = doc.closest(target, "[data-view]") {
        if doc.get_attribute(view_el, "data-view").as_deref() == Some("contacts") {
            state.email_mode = EmailViewMode::Contacts;
            state.selected_label = None;
            render_active_skin(doc, state);
            return;
        }
    }

    // 6. Check mailbox selection (Wazoo item in Eudora OR sidebar item in Email)
    if let Some(mb_el) = doc.closest(target, "[data-mailbox]") {
        if let Some(mailbox) = doc.get_attribute(mb_el, "data-mailbox") {
            state.selected_label = None;
            state.email_mode = EmailViewMode::Mail;
            if mailbox == "Out" && state.skin == Skin::Eudora {
                state.eudora_tab = EudoraTab::Out;
            } else {
                state.selected_mailbox = mailbox;
                state.eudora_tab = EudoraTab::In;
            }
            render_active_skin(doc, state);
            return;
        }
    }

    // 7. Check label selection in Email skin
    if let Some(lbl_el) = doc.closest(target, "[data-label]") {
        if let Some(label) = doc.get_attribute(lbl_el, "data-label") {
            state.email_mode = EmailViewMode::Mail;
            if state.selected_label.as_deref() == Some(label.as_str()) {
                state.selected_label = None;
            } else {
                state.selected_label = Some(label);
            }
            render_active_skin(doc, state);
            return;
        }
    }

    // 8. Check message row selection
    if let Some(msg_el) = doc.closest(target, "[data-msg-id]") {
        if let Some(id_str) = doc.get_attribute(msg_el, "data-msg-id") {
            if let Ok(msg_id) = id_str.parse::<usize>() {
                let is_out = doc.get_attribute(msg_el, "data-box").as_deref() == Some("out");
                if is_out {
                    state.selected_out_msg_id = msg_id;
                } else {
                    state.selected_in_msg_id = msg_id;
                    if let Some(msg) = state.messages.iter_mut().find(|m| m.id == msg_id) {
                        msg.unread = false;
                    }
                }
                state.email_mode = EmailViewMode::Mail;
                render_active_skin(doc, state);
                return;
            }
        }
    }

    // 9. Check contact card selection
    if let Some(contact_el) = doc.closest(target, "[data-contact]") {
        if let Some(idx_str) = doc.get_attribute(contact_el, "data-contact") {
            if let Ok(idx) = idx_str.parse::<usize>() {
                if idx < state.contacts.len() {
                    state.selected_contact_idx = idx;
                    render_active_skin(doc, state);
                    return;
                }
            }
        }
    }

    // 10. Check filter card / toggle switch
    if let Some(filter_el) = doc.closest(target, "[data-filter]") {
        if let Some(idx_str) = doc.get_attribute(filter_el, "data-filter") {
            if let Ok(idx) = idx_str.parse::<usize>() {
                if let Some(f) = state.filters.get_mut(idx) {
                    f.enabled = !f.enabled;
                    render_active_skin(doc, state);
                    return;
                }
            }
        }
    }
}

// ============================================================================
// Webcore Browser Context Host (WHATWG DOM + BrowserChrome::NONE)
// ============================================================================

fn build_skin_document(skin: Skin, state: &Arc<Mutex<AppState>>, next_generation: u64) -> Document {
    let html = match skin {
        Skin::Eudora => EUDORA_HTML,
        Skin::Email => EMAIL_HTML,
        Skin::Settings => SETTINGS_HTML,
    };
    let mut doc = webcore::parse_html(html);
    doc.rebuild_node_index();
    doc.layout_generation = next_generation;
    {
        let mut st = state.lock().unwrap();
        st.skin = skin;
        render_active_skin(&mut doc, &mut st);
        st.dirty = false;
    }
    doc.rebuild_node_index();
    register_listeners(&mut doc, state.clone());
    doc
}

struct AzenGuest {
    doc: BrowserDocument,
    state: Arc<Mutex<AppState>>,
}

impl BrowserGuest for AzenGuest {
    fn dispatch_pending(&mut self) -> bool {
        let (pending_skin, dirty) = {
            let mut st = self.state.lock().unwrap();
            let pending = st.pending_skin.take();
            let dirty = st.dirty;
            st.dirty = false;
            (pending, dirty)
        };

        if let Some(next_skin) = pending_skin {
            let next_gen = self.doc.read().layout_generation.wrapping_add(1);
            let new_doc = build_skin_document(next_skin, &self.state, next_gen);
            *self.doc.write() = new_doc;
            return true;
        }

        dirty
    }
}

fn main() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let initial_doc = build_skin_document(Skin::Eudora, &state, 1);
    let browser_doc = BrowserDocument::new(initial_doc);

    let options = BrowserOptions {
        title: "Azen — Eudora Mail".into(),
        width: 1180.0,
        height: 800.0,
        chrome: BrowserChrome::NONE,
        ..Default::default()
    };

    let mut browser = Browser::open(options).expect("failed to open webcore browser context");
    browser.attach_document(browser_doc.clone());
    browser.set_guest(AzenGuest {
        doc: browser_doc,
        state,
    });
    browser.run().expect("browser context exited with error");
}

