# Changelog

## [0.1.1](https://github.com/doqhu/addict/compare/v0.1.0...v0.1.1) (2026-11-08)


### Bug Fixes

* **platform:** configure shortcuts on portals without ConfigureShortcuts ([#8](https://github.com/doqhu/addict/issues/8)) ([7ce30eb](https://github.com/doqhu/addict/commit/7ce30eb0ef6ad596f8accdf94ec7a401051f24b3))

## 0.1.0 (2026-10-31)

First release of addict, a hotkey translator for Linux readers of English, Spanish and Catalan.


### Features

* look up a selected or copied word with a global shortcut, and capture text that cannot be selected from the screen with Tesseract OCR
* read offline dictionary articles in English, Spanish and Catalan, with senses, synonyms, transcriptions and Russian translations
* find a word by its dictionary forms, and look up a Russian word the other way, into the reading language
* translate a clicked word in the context of its sentence, the sentence or a chosen definition with DeepL, and cache the answers on disk
* import dictionaries with `addict import`
* use the top-bar icon, Settings, About and login startup
* install as a Flatpak that builds with no network and bundles Tesseract with English, Spanish and Catalan data
