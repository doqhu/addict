//! The online translator of Addict: an asynchronous DeepL client.
//!
//! [`DeepL`] implements [`addict_core::translation::Translator`]. It reads the
//! key from a file at each request ([`key`]), chooses the host by the key and
//! knows three requests: translate, the languages and the usage ([`client`]).
//! It starts nothing by itself and sets no timer: the caller runs its futures
//! and gives them a deadline. With the `stub` feature, `stub` is a server on a
//! loopback port that answers as a test scripts it.
//!
//! This crate is GTK-free; it must never depend on `gtk4`, `gdk4*`, `gio` or
//! `glib`. The key never leaves it except in the `Authorization` header: no
//! error, no `Debug` output and no log of this crate holds it.

pub mod client;
pub mod key;
#[cfg(feature = "stub")]
pub mod stub;

pub use client::{DeepL, Language, Usage};
