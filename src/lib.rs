//! Flooring Partners Tools: the QR code generator and the VendorCafe CSV
//! converter in one program.
//!
//! * [`generator`] - turn a web address or contact into a PNG, save it, show it
//! * [`converter`] - consolidate a Rollmaster invoice export for VendorCafe
//! * [`config`]    - the user's saved settings (one file for both tools)
//! * [`old_apps`]  - find and remove the separate apps this one replaces
//! * [`common::updater`] - check for, download and start updates
//! * [`gui`]       - the window (Windows only)
//! * [`cli`]       - the same features from the command line, for scripts
//!
//! No third-party code is used except rustls/ring/webpki-roots for HTTPS.

pub mod app;
pub mod cli;
pub mod common;
pub mod config;
pub mod converter;
pub mod csv;
pub mod generator;
#[cfg(windows)]
pub mod gui;
pub mod image;
pub mod old_apps;
pub mod qr;
pub mod render;
pub mod vcard;

pub use app::APP;
