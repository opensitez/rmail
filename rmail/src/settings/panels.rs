//! Settings panel definitions — pure data, no UI code.
//!
//! Each panel matches the corresponding settings_*.c page exactly:
//! same fields, same groups, same order.

use super::{SettingsField, SettingsPanel};

/// All panels in the original Mac Eudora order (from gtk_settings.c:371-378).
pub fn all_panels() -> Vec<SettingsPanel> {
    vec![
        getting_started(),
        checking_mail(),
        sending_mail(),
        composing(),
        mailbox_display(),
        date_display(),
        styled_text(),
        fonts(),
        labels(),
        attachments(),
        replying(),
        junk_mail(),
        toolbar(),
        getting_attention(),
        hosts(),
        moving_around(),
        extra_warnings(),
        miscellaneous(),
        accounts(),
        ssl(),
        spell_checking(),
    ]
}

// ── Getting Started (settings_getting_started.c) ──

fn getting_started() -> SettingsPanel {
    SettingsPanel {
        id: "getting_started",
        title: "Getting Started",
        subtitle: "Configure your basic mail settings to get up and running.",
        icon: "go-home-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Incoming Mail" },
            SettingsField::Text { section: "checking_mail", key: "pop_username", label: "User Name", placeholder: "you@example.com" },
            SettingsField::Text { section: "checking_mail", key: "pop_server", label: "Mail Server", placeholder: "mail.example.com" },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Outgoing Mail" },
            SettingsField::Text { section: "sending_mail", key: "real_name", label: "Real Name", placeholder: "John Doe" },
            SettingsField::Text { section: "sending_mail", key: "smtp_server", label: "SMTP Server", placeholder: "smtp.example.com" },
            SettingsField::Text { section: "sending_mail", key: "email_address", label: "Email Address", placeholder: "you@example.com" },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "" },
            SettingsField::Bool { section: "general", key: "default_mailer", label: "Make rMail the default mail application", default: false },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Checking Mail (settings_checking_mail.c) ──

fn checking_mail() -> SettingsPanel {
    SettingsPanel {
        id: "checking_mail",
        title: "Checking Mail",
        subtitle: "Configure how rMail retrieves your email.",
        icon: "mail-read-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Account / Server Information" },
            SettingsField::Text { section: "checking_mail", key: "pop_username", label: "User Name", placeholder: "" },
            SettingsField::Text { section: "checking_mail", key: "pop_server", label: "Mail Server", placeholder: "" },
            SettingsField::Radio {
                keys: &[("checking_mail", "use_pop"), ("checking_mail", "use_imap")],
                label: "Mail Protocol",
                options: &["POP", "IMAP"],
                default: 0,
            },
            SettingsField::Radio {
                keys: &[("checking_mail", "use_passwords"), ("checking_mail", "use_kerberos"), ("checking_mail", "use_apop")],
                label: "Authentication",
                options: &["Passwords", "Kerberos", "APOP"],
                default: 0,
            },
            SettingsField::Bool { section: "checking_mail", key: "overlap_commands", label: "Overlap commands (pipelining)", default: false },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Connection" },
            SettingsField::Int { section: "checking_mail", key: "check_interval", prefix: "Check for mail every", suffix: "minutes", default: 15, min: 1, max: 999 },
            SettingsField::Bool { section: "checking_mail", key: "check_battery", label: "Don't auto-check when using battery", default: false },
            SettingsField::Bool { section: "checking_mail", key: "send_on_check", label: "Send on check", default: true },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Mail Management" },
            SettingsField::Int { section: "checking_mail", key: "leave_days", prefix: "Leave on server for", suffix: "days", default: 0, min: 0, max: 999 },
            SettingsField::Bool { section: "checking_mail", key: "delete_from_trash", label: "Delete from server when emptied from Trash", default: false },
            SettingsField::Int { section: "checking_mail", key: "skip_size_kb", prefix: "Skip messages over", suffix: "K", default: 0, min: 0, max: 10000 },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Sending Mail (settings_sending_mail.c) ──

fn sending_mail() -> SettingsPanel {
    SettingsPanel {
        id: "sending_mail",
        title: "Sending Mail",
        subtitle: "Configure how rMail sends your email.",
        icon: "mail-send-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Server" },
            SettingsField::Text { section: "sending_mail", key: "email_address", label: "Email Address", placeholder: "you@example.com" },
            SettingsField::Text { section: "sending_mail", key: "smtp_server", label: "SMTP Server", placeholder: "smtp.example.com" },
            SettingsField::Text { section: "sending_mail", key: "default_domain", label: "Default Domain", placeholder: "" },
            SettingsField::Bool { section: "sending_mail", key: "use_submission_port", label: "Use submission port (587)", default: false },
            SettingsField::Bool { section: "sending_mail", key: "allow_smtp_auth", label: "Allow SMTP AUTH", default: true },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Sending" },
            SettingsField::Bool { section: "sending_mail", key: "send_immediately", label: "Send messages immediately on queue", default: true },
            SettingsField::Bool { section: "sending_mail", key: "send_on_check", label: "Send on check", default: true },
            SettingsField::Bool { section: "sending_mail", key: "fix_curly_quotes", label: "Fix curly quotes", default: true },
            SettingsField::Bool { section: "sending_mail", key: "keep_sent_copy", label: "Keep copies of outgoing mail", default: true },
            SettingsField::Bool { section: "sending_mail", key: "auto_fcc_original", label: "FCC to original mailbox", default: false },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Composing (settings_composing.c) ──

fn composing() -> SettingsPanel {
    SettingsPanel {
        id: "composing",
        title: "Composing",
        subtitle: "Preferences for composing new messages.",
        icon: "accessories-text-editor-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Addressing" },
            SettingsField::Bool { section: "composing", key: "auto_complete_nicknames", label: "Automatically complete nicknames", default: true },
            SettingsField::Bool { section: "composing", key: "expand_nicknames_immediately", label: "Expand nicknames immediately", default: false },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Message Format" },
            SettingsField::Int { section: "composing", key: "word_wrap_column", prefix: "Word wrap at column", suffix: "", default: 76, min: 20, max: 200 },
            SettingsField::Bool { section: "composing", key: "may_use_qp", label: "May use Quoted-Printable encoding", default: true },
            SettingsField::Bool { section: "composing", key: "keep_sent_copy", label: "Keep copies of outgoing mail", default: true },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Mailbox Display (settings_mailbox_display.c) ──

fn mailbox_display() -> SettingsPanel {
    SettingsPanel {
        id: "mailbox_display",
        title: "Mailbox Display",
        subtitle: "Choose which columns to show in message lists.",
        icon: "view-list-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Columns" },
            SettingsField::Bool { section: "mailbox_display", key: "show_col_status", label: "Status", default: true },
            SettingsField::Bool { section: "mailbox_display", key: "show_col_priority", label: "Priority", default: true },
            SettingsField::Bool { section: "mailbox_display", key: "show_col_attachments", label: "Attachments", default: true },
            SettingsField::Bool { section: "mailbox_display", key: "show_col_label", label: "Label", default: true },
            SettingsField::Bool { section: "mailbox_display", key: "show_col_who", label: "Who (From/To)", default: true },
            SettingsField::Bool { section: "mailbox_display", key: "show_col_date", label: "Date", default: true },
            SettingsField::Bool { section: "mailbox_display", key: "show_col_size", label: "Size", default: true },
            SettingsField::Bool { section: "mailbox_display", key: "show_col_server", label: "Server Status", default: false },
            SettingsField::Bool { section: "mailbox_display", key: "show_col_mood", label: "Mood", default: false },
            SettingsField::Bool { section: "mailbox_display", key: "show_col_junk", label: "Junk Score", default: false },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Lines" },
            SettingsField::Bool { section: "mailbox_display", key: "draw_horiz_lines", label: "Draw horizontal separator lines", default: true },
            SettingsField::Bool { section: "mailbox_display", key: "draw_vert_lines", label: "Draw vertical separator lines", default: false },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Preview" },
            SettingsField::Bool { section: "mailbox_display", key: "show_selected_count", label: "Show selected message count", default: true },
            SettingsField::Bool { section: "mailbox_display", key: "show_preview", label: "Show preview pane", default: true },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Mark as Read" },
            SettingsField::Int { section: "mailbox_display", key: "mark_read_delay", prefix: "Mark read after", suffix: "seconds preview", default: 0, min: 0, max: 60 },
            SettingsField::Bool { section: "mailbox_display", key: "mark_read_on_scroll", label: "Mark read when scrolling past", default: false },
            SettingsField::Bool { section: "mailbox_display", key: "mark_read_on_delete", label: "Mark read on delete", default: true },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Date Display (settings_date_display.c) ──

fn date_display() -> SettingsPanel {
    SettingsPanel {
        id: "date_display",
        title: "Date Display",
        subtitle: "How dates are shown in message lists.",
        icon: "x-office-calendar-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Date Format" },
            SettingsField::Bool { section: "date_display", key: "date_age_sensitive", label: "Age-sensitive date display (today, yesterday, ...)", default: true },
            SettingsField::Bool { section: "date_display", key: "date_local_timezone", label: "Show dates in local timezone", default: true },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Styled Text (settings_styled_text.c) ──

fn styled_text() -> SettingsPanel {
    SettingsPanel {
        id: "styled_text",
        title: "Styled Text",
        subtitle: "Control how styled (rich) text is handled.",
        icon: "format-text-rich-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "When Sending Styled Text" },
            SettingsField::Radio {
                keys: &[
                    ("styled_text", "send_both"),
                    ("styled_text", "send_styled"),
                    ("styled_text", "send_plain"),
                    ("styled_text", "send_ask"),
                ],
                label: "",
                options: &["Send both plain and styled", "Send styled only", "Send plain only", "Ask each time"],
                default: 0,
            },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Recognize Styles" },
            SettingsField::Bool { section: "styled_text", key: "styled_bold", label: "Bold", default: true },
            SettingsField::Bool { section: "styled_text", key: "styled_italic", label: "Italic", default: true },
            SettingsField::Bool { section: "styled_text", key: "styled_underline", label: "Underline", default: true },
            SettingsField::Bool { section: "styled_text", key: "styled_color", label: "Color", default: true },
            SettingsField::Bool { section: "styled_text", key: "styled_size", label: "Size", default: true },
            SettingsField::Bool { section: "styled_text", key: "styled_font", label: "Font", default: true },
            SettingsField::Bool { section: "styled_text", key: "styled_margins", label: "Margins", default: true },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "" },
            SettingsField::Bool { section: "styled_text", key: "show_format_toolbar", label: "Show formatting toolbar", default: true },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Fonts (settings_fonts.c) — simplified, no FontDialogButton yet ──

fn fonts() -> SettingsPanel {
    SettingsPanel {
        id: "fonts",
        title: "Fonts",
        subtitle: "Font settings for display and printing.",
        icon: "font-x-generic-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Display Fonts" },
            SettingsField::Text { section: "fonts", key: "screen_font", label: "Screen Font", placeholder: "Sans 12" },
            SettingsField::Int { section: "fonts", key: "screen_font_size", prefix: "Size", suffix: "pt", default: 12, min: 6, max: 72 },
            SettingsField::Text { section: "fonts", key: "fixed_font", label: "Fixed Font", placeholder: "Monospace 12" },
            SettingsField::Int { section: "fonts", key: "fixed_font_size", prefix: "Size", suffix: "pt", default: 12, min: 6, max: 72 },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Print Font" },
            SettingsField::Text { section: "fonts", key: "print_font", label: "Print Font", placeholder: "Serif 12" },
            SettingsField::Int { section: "fonts", key: "print_font_size", prefix: "Size", suffix: "pt", default: 12, min: 6, max: 72 },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Options" },
            SettingsField::Bool { section: "fonts", key: "display_graphics", label: "Display graphics in messages", default: true },
            SettingsField::Bool { section: "fonts", key: "display_emoticons", label: "Display emoticons as images", default: true },
            SettingsField::Bool { section: "fonts", key: "zoom_on_open", label: "Zoom message to window width", default: false },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Labels (settings_labels.c) — name entry + color picker per label ──

fn labels() -> SettingsPanel {
    SettingsPanel {
        id: "labels",
        title: "Labels",
        subtitle: "Customize the seven message labels and their colors.",
        icon: "starred-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "" },
            SettingsField::TextWithColor { section: "labels", text_key: "label_0", color_key: "color_0", label: "Label 1", placeholder: "Label 1", default_rgb: (1.0, 0.0, 0.0) },
            SettingsField::TextWithColor { section: "labels", text_key: "label_1", color_key: "color_1", label: "Label 2", placeholder: "Label 2", default_rgb: (0.0, 0.0, 1.0) },
            SettingsField::TextWithColor { section: "labels", text_key: "label_2", color_key: "color_2", label: "Label 3", placeholder: "Label 3", default_rgb: (0.0, 0.6, 0.0) },
            SettingsField::TextWithColor { section: "labels", text_key: "label_3", color_key: "color_3", label: "Label 4", placeholder: "Label 4", default_rgb: (0.6, 0.0, 0.6) },
            SettingsField::TextWithColor { section: "labels", text_key: "label_4", color_key: "color_4", label: "Label 5", placeholder: "Label 5", default_rgb: (1.0, 0.5, 0.0) },
            SettingsField::TextWithColor { section: "labels", text_key: "label_5", color_key: "color_5", label: "Label 6", placeholder: "Label 6", default_rgb: (0.0, 0.6, 0.6) },
            SettingsField::TextWithColor { section: "labels", text_key: "label_6", color_key: "color_6", label: "Label 7", placeholder: "Label 7", default_rgb: (0.4, 0.4, 0.4) },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Attachments (settings_attachments.c) ──

fn attachments() -> SettingsPanel {
    SettingsPanel {
        id: "attachments",
        title: "Attachments",
        subtitle: "How attachments are encoded and stored.",
        icon: "mail-attachment-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Encoding" },
            SettingsField::Radio {
                keys: &[
                    ("attachments", "enc_mime"),
                    ("attachments", "enc_binhex"),
                    ("attachments", "enc_uuencode"),
                ],
                label: "Encoding",
                options: &["MIME", "BinHex", "UUencode"],
                default: 0,
            },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Storage" },
            SettingsField::Text { section: "attachments", key: "attach_folder", label: "Attachment Folder", placeholder: "" },
            SettingsField::Bool { section: "attachments", key: "auto_save_attachments", label: "Save attachments to disk on receive", default: false },
            SettingsField::Bool { section: "attachments", key: "trash_attachments_with_msg", label: "Trash attachments with message", default: false },
            SettingsField::Bool { section: "attachments", key: "receive_digests_as_attach", label: "Receive digests as attachments", default: false },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Replying (settings_replying.c) ──

fn replying() -> SettingsPanel {
    SettingsPanel {
        id: "replying",
        title: "Replying",
        subtitle: "Defaults for replying to messages.",
        icon: "mail-reply-all-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Reply Options" },
            SettingsField::Bool { section: "replying", key: "reply_to_all_default", label: "Reply to all by default", default: false },
            SettingsField::Bool { section: "replying", key: "reply_include_self", label: "Include yourself in Reply All", default: false },
            SettingsField::Bool { section: "replying", key: "reply_to_in_cc", label: "Put original To: recipients in Cc:", default: false },
            SettingsField::Bool { section: "replying", key: "copy_original_priority", label: "Copy original message priority", default: false },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Junk Mail (settings_junk_mail.c) ──

fn junk_mail() -> SettingsPanel {
    SettingsPanel {
        id: "junk_mail",
        title: "Junk Mail",
        subtitle: "Junk mail scoring and handling.",
        icon: "edit-delete-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Junk Scoring" },
            SettingsField::Int { section: "junk_mail", key: "junk_threshold", prefix: "Junk threshold", suffix: "(0-100)", default: 50, min: 0, max: 100 },
            SettingsField::Bool { section: "junk_mail", key: "junk_check_addressbook", label: "Whitelist senders in address book", default: true },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Junk Handling" },
            SettingsField::Bool { section: "junk_mail", key: "junk_hold_mailbox", label: "Hold junk in Junk mailbox", default: true },
            SettingsField::Bool { section: "junk_mail", key: "junk_never_unread", label: "Never mark junk as unread", default: false },
            SettingsField::Int { section: "junk_mail", key: "junk_remove_days", prefix: "Auto-delete junk after", suffix: "days (0 = never)", default: 0, min: 0, max: 365 },
            SettingsField::Bool { section: "junk_mail", key: "junk_warn_remove", label: "Warn before deleting old junk", default: true },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Toolbar (settings_toolbar.c) ──

fn toolbar() -> SettingsPanel {
    SettingsPanel {
        id: "toolbar",
        title: "Toolbar",
        subtitle: "Toolbar display settings.",
        icon: "applications-system-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Toolbar" },
            SettingsField::Bool { section: "toolbar", key: "show_toolbar", label: "Show toolbar", default: true },
            SettingsField::Bool { section: "toolbar", key: "show_search_field", label: "Show search field in toolbar", default: true },
            SettingsField::Choice {
                section: "toolbar", key: "toolbar_button_style",
                label: "Button Style",
                options: &["Large icons with names", "Large icons only", "Small icons with names", "Small icons only", "Names only"],
                default: 0,
            },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Getting Attention (settings_getting_attention.c) ──

fn getting_attention() -> SettingsPanel {
    SettingsPanel {
        id: "getting_attention",
        title: "Getting Attention",
        subtitle: "How rMail notifies you of new mail.",
        icon: "dialog-information-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "New Mail Alerts" },
            SettingsField::Bool { section: "getting_attention", key: "alert_on_new", label: "Show alert dialog", default: true },
            SettingsField::Bool { section: "getting_attention", key: "bounce_dock", label: "Bounce dock icon", default: true },
            SettingsField::Bool { section: "getting_attention", key: "open_mailbox_on_new", label: "Open In mailbox", default: false },
            SettingsField::Bool { section: "getting_attention", key: "play_sound_on_new", label: "Play sound", default: true },
            SettingsField::Text { section: "getting_attention", key: "new_mail_sound", label: "Sound File", placeholder: "" },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Progress" },
            SettingsField::Bool { section: "getting_attention", key: "show_task_progress", label: "Show task progress in sidebar", default: true },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Hosts (settings_hosts.c) ──

fn hosts() -> SettingsPanel {
    SettingsPanel {
        id: "hosts",
        title: "Hosts",
        subtitle: "Directory and network host settings.",
        icon: "network-server-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Directory Services" },
            SettingsField::Text { section: "hosts", key: "ph_server", label: "Ph Server", placeholder: "" },
            SettingsField::Text { section: "hosts", key: "finger_server", label: "Finger Server", placeholder: "" },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Proxy" },
            SettingsField::Choice { section: "proxy", key: "type", label: "Proxy Type",
                options: &["None", "HTTP CONNECT", "SOCKS5"], default: 0 },
            SettingsField::Text { section: "proxy", key: "host", label: "Proxy Server", placeholder: "proxy.example.com" },
            SettingsField::Int { section: "proxy", key: "port", prefix: "Port", suffix: "", default: 8080, min: 1, max: 65535 },
            SettingsField::Text { section: "proxy", key: "username", label: "Username", placeholder: "(optional)" },
            SettingsField::Text { section: "proxy", key: "password", label: "Password", placeholder: "(optional)" },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Network" },
            SettingsField::Bool { section: "hosts", key: "dns_load_balance", label: "DNS load balancing", default: false },
            SettingsField::Bool { section: "hosts", key: "offline_mode", label: "Start in offline mode", default: false },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Moving Around (settings_moving_around.c) ──

fn moving_around() -> SettingsPanel {
    SettingsPanel {
        id: "moving_around",
        title: "Moving Around",
        subtitle: "Navigation behavior after actions.",
        icon: "go-next-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "After Transfer/Delete" },
            SettingsField::Choice {
                section: "moving_around", key: "after_message",
                label: "After deleting/transferring",
                options: &["Do nothing", "Open next message", "Open next unread", "Open next if unread exists", "Open same subject"],
                default: 1,
            },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Key Behavior" },
            SettingsField::Bool { section: "moving_around", key: "tab_switches_fields", label: "Tab switches between header fields", default: true },
            SettingsField::Bool { section: "moving_around", key: "return_switches_fields", label: "Return switches between fields", default: false },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Extra Warnings (settings_extra_warnings.c) ──

fn extra_warnings() -> SettingsPanel {
    SettingsPanel {
        id: "extra_warnings",
        title: "Extra Warnings",
        subtitle: "Additional confirmation dialogs.",
        icon: "dialog-warning-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Delete Warnings" },
            SettingsField::Bool { section: "extra_warnings", key: "warn_delete_unread", label: "Warn when deleting unread messages", default: true },
            SettingsField::Bool { section: "extra_warnings", key: "warn_delete_queued", label: "Warn when deleting queued messages", default: true },
            SettingsField::Bool { section: "extra_warnings", key: "warn_delete_unsent", label: "Warn when deleting unsent messages", default: true },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Send Warnings" },
            SettingsField::Bool { section: "extra_warnings", key: "warn_queue_no_subject", label: "Warn when queueing without subject", default: true },
            SettingsField::Bool { section: "extra_warnings", key: "warn_queue_styled", label: "Warn when queueing styled text", default: false },
            SettingsField::Int { section: "extra_warnings", key: "warn_send_size_kb", prefix: "Warn when sending over", suffix: "KB", default: 0, min: 0, max: 99999 },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Quit Warnings" },
            SettingsField::Bool { section: "extra_warnings", key: "warn_quit_queued", label: "Warn on quit with queued messages", default: true },
            SettingsField::Bool { section: "extra_warnings", key: "warn_empty_trash", label: "Warn before emptying Trash", default: true },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Miscellaneous (settings_miscellaneous.c) ──

fn miscellaneous() -> SettingsPanel {
    SettingsPanel {
        id: "miscellaneous",
        title: "Miscellaneous",
        subtitle: "Various application settings.",
        icon: "preferences-system-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Behavior" },
            SettingsField::Bool { section: "miscellaneous", key: "close_msg_with_mailbox", label: "Close message windows with mailbox", default: true },
            SettingsField::Bool { section: "miscellaneous", key: "empty_trash_on_quit", label: "Empty Trash on quit", default: false },
            SettingsField::Bool { section: "miscellaneous", key: "turbo_redirect", label: "Turbo redirect (no confirmation)", default: false },
            SettingsField::Bool { section: "miscellaneous", key: "resort_less_often", label: "Resort mailboxes less often", default: false },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Advanced" },
            SettingsField::Bool { section: "miscellaneous", key: "use_old_toc", label: "Use old-style TOC format", default: false },
            SettingsField::Bool { section: "miscellaneous", key: "generate_filter_reports", label: "Generate filter reports", default: false },
            SettingsField::Bool { section: "miscellaneous", key: "use_keychain", label: "Use system keychain for passwords", default: true },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Accounts (settings_accounts.c) — placeholder, needs custom widget ──

fn accounts() -> SettingsPanel {
    SettingsPanel {
        id: "accounts",
        title: "Accounts",
        subtitle: "Manage email accounts and personalities.",
        icon: "system-users-symbolic",
        fields: &[
            // Accounts page is custom — uses Conf::get_all_accounts() + list widget.
            // For now, show the dominant account fields.
            SettingsField::GroupStart { label: "Primary Account" },
            SettingsField::Text { section: "sending_mail", key: "real_name", label: "Real Name", placeholder: "" },
            SettingsField::Text { section: "sending_mail", key: "email_address", label: "Email", placeholder: "" },
            SettingsField::Text { section: "checking_mail", key: "pop_server", label: "Incoming Server", placeholder: "" },
            SettingsField::Text { section: "sending_mail", key: "smtp_server", label: "SMTP Server", placeholder: "" },
            SettingsField::GroupEnd,
        ],
    }
}

// ── SSL (settings_ssl.c) ──

fn ssl() -> SettingsPanel {
    SettingsPanel {
        id: "ssl",
        title: "SSL / TLS",
        subtitle: "Secure connection settings.",
        icon: "channel-secure-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "SSL Mode" },
            SettingsField::Choice {
                section: "ssl", key: "ssl_pop_mode",
                label: "POP SSL",
                options: &["Off", "Required", "Alternate Port (995)"],
                default: 0,
            },
            SettingsField::Choice {
                section: "ssl", key: "ssl_smtp_mode",
                label: "SMTP SSL",
                options: &["Off", "Required", "Alternate Port (465)"],
                default: 0,
            },
            SettingsField::Choice {
                section: "ssl", key: "ssl_imap_mode",
                label: "IMAP SSL",
                options: &["Off", "Required", "Alternate Port (993)"],
                default: 0,
            },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "" },
            SettingsField::Bool { section: "ssl", key: "save_password", label: "Save password in keychain", default: true },
            SettingsField::GroupEnd,
        ],
    }
}

// ── Spell Checking (settings_spell_checking.c) ──

fn spell_checking() -> SettingsPanel {
    SettingsPanel {
        id: "spell_checking",
        title: "Spell Checking",
        subtitle: "Spell check preferences.",
        icon: "tools-check-spelling-symbolic",
        fields: &[
            SettingsField::GroupStart { label: "Automatic" },
            SettingsField::Bool { section: "spell_checking", key: "spell_auto_check", label: "Check spelling as you type", default: true },
            SettingsField::Bool { section: "spell_checking", key: "spell_warn_on_send", label: "Warn about misspellings before sending", default: true },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Ignore" },
            SettingsField::Bool { section: "spell_checking", key: "spell_ignore_caps", label: "Ignore words starting with capitals", default: false },
            SettingsField::Bool { section: "spell_checking", key: "spell_ignore_mixed_case", label: "Ignore mixed case words", default: false },
            SettingsField::Bool { section: "spell_checking", key: "spell_ignore_numbers", label: "Ignore words with numbers", default: true },
            SettingsField::Bool { section: "spell_checking", key: "spell_ignore_all_caps", label: "Ignore ALL CAPS words", default: true },
            SettingsField::GroupEnd,

            SettingsField::GroupStart { label: "Suggestions" },
            SettingsField::Choice {
                section: "spell_checking", key: "spell_suggest_mode",
                label: "Suggest by",
                options: &["Looks like", "Sounds like", "Both", "Never suggest"],
                default: 0,
            },
            SettingsField::GroupEnd,
        ],
    }
}
