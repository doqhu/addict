//! Platform adapters for Addict: XDG portals (GlobalShortcuts, Screenshot),
//! the accessibility bus, text recognition with Tesseract, and the runner of
//! slow work.
//!
//! [`atspi`] reads the word selected in Android Studio and its sentence over
//! AT-SPI; [`shortcut`] binds the global shortcut; [`screenshot`] takes one
//! interactive screen capture through the portal; [`ocr`] recognizes the text
//! of an image in a Tesseract child with a limit; [`work`] runs slow jobs off
//! the caller's thread; [`desktop`] writes the desktop entries.
//!
//! This crate is GTK-free; it must never depend on `gtk4`, `gdk4*`, `gio` or
//! `glib`. Adapters run off the UI thread and report to the application.

pub mod atspi;
pub mod desktop;
pub mod ocr;
pub mod screenshot;
pub mod shortcut;
pub mod work;
