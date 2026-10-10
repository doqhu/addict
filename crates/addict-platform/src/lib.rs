//! Platform adapters for Addict: XDG portals (GlobalShortcuts, Screenshot),
//! AT-SPI context reads and screen capture.
//!
//! This crate is GTK-free; it must never depend on `gtk4`, `gdk4*`, `gio` or
//! `glib`. Adapters run off the UI thread and report to the application.

pub mod desktop;
pub mod shortcut;
pub mod work;
