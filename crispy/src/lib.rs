//! # Crispy — Standalone Mail Protocol Library
//!
//! Pure Rust implementation of mail protocols and utilities:
//! - SMTP client (send mail)
//! - POP3 client (receive mail)
//! - IMAP client (sync mail)
//! - RFC 5322 message construction and parsing
//! - RFC 2822 address parsing (lex822)
//! - MIME: base64, quoted-printable, uuencode, binhex
//! - HTML tokenizer/parser
//! - Rich text (text/enriched) converter
//! - Authentication (PLAIN, LOGIN, CRAM-MD5, XOAUTH2)

pub mod base64;
pub mod md5;
pub mod encode;
pub mod uudecode;
pub mod binhex;
pub mod lex822;
pub mod rfc822;
pub mod headparse;
pub mod html;
pub mod richtext;
pub mod auth;
pub mod msg;
pub mod transport;
pub mod smtp;
pub mod pop3;
pub mod imap;
