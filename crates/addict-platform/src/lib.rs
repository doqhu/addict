//! Platform adapters for Addict: XDG portals (GlobalShortcuts, Screenshot,
//! Background), text recognition with Tesseract, and the runner of slow work.
//!
//! [`shortcut`] binds the global shortcut; [`screenshot`] takes one
//! interactive screen capture through the portal; [`startup`] asks the
//! Background portal to turn login startup on or off; [`ocr`] recognizes the
//! text of an image in a Tesseract child with a limit; [`work`] runs slow
//! jobs off the caller's thread; [`desktop`] writes the desktop entry and
//! recognises a login entry;
//! [`tray`] puts the icon and its menu in the top bar; [`gpu`] names the
//! Vulkan drivers of the GPU that drives the display.
//!
//! This crate is GTK-free; it must never depend on `gtk4`, `gdk4*`, `gio` or
//! `glib`. Adapters run off the UI thread and report to the application.

pub mod desktop;
pub mod gpu;
pub mod ocr;
mod portal;
pub mod screenshot;
pub mod shortcut;
pub mod startup;
pub mod tray;
pub mod work;
