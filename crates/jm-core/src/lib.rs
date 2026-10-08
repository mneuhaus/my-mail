//! Just Mail core: Microsoft 365 mailboxes over Microsoft Graph.
//!
//! Shared by the desktop app and the `jm` CLI so both see the same accounts, tokens,
//! signatures and short message ids. Everything here is blocking; the app calls it on
//! background threads.
//!
//! Sending lives behind the `send` cargo feature, which only the app enables.

pub mod auth;
pub mod config;
pub mod error;
pub mod events;
pub mod graph;
pub mod html;
pub mod ids;
pub mod model;
pub mod paths;

pub use config::{AccountConfig, Config};
pub use error::{Error, ErrorCode, Result};
pub use graph::Mailbox;
pub use model::*;
