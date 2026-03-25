//! # Macmbx — Standalone Eudora Mbox Storage Library
//!
//! Pure Rust implementation of Eudora mailbox management:
//! - TOC (table of contents) read/write/validate
//! - Message storage (mbox format)
//! - Mailbox store (directory tree management)
//! - Message operations (transfer, delete, copy, compact)
//! - Filters (match rules, actions, load/save)
//! - Junk mail scoring
//! - Nicknames / address books
//! - Configuration (INI-style preferences)
//! - Signatures and stationery
//! - vCard import/export
//! - Emoji mapping
//! - Unicode utilities
//! - XML parser
//! - Spell checking
//! - Message threading
//! - Mail send/receive engine (uses crispy)

pub mod toc;
pub mod build;
pub mod ops;
pub mod store;
pub mod compact;
pub mod filter;
pub mod junk;
pub mod nicknames;
pub mod conf;
pub mod sigstat;
pub mod vcard;
pub mod emoji;
pub mod unicode;
pub mod xml;
pub mod calendar;
pub mod ics;
pub mod caldav_sync;
pub mod spell;
pub mod thread;
pub mod concentrator;
pub mod mailer;
pub mod stats;
