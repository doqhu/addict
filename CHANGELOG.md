# Changelog

## [0.4.1](https://github.com/doqhu/addict/compare/v0.4.0...v0.4.1) (2026-11-19)


### Bug Fixes

* **appimage:** pin libaom 3.15.3, as Koji dropped the signed 3.15.2 copy ([#30](https://github.com/doqhu/addict/issues/30)) ([4ad29f9](https://github.com/doqhu/addict/commit/4ad29f9c73a21fa6bdc66c0c9b49e2c333563988))

## [0.4.0](https://github.com/doqhu/addict/compare/v0.3.1...v0.4.0) (2026-11-19)


### Features

* keep the shortcut names fixed on Hyprland and say how to bind them ([#28](https://github.com/doqhu/addict/issues/28)) ([fd1dc4f](https://github.com/doqhu/addict/commit/fd1dc4f608eb1c6ff5353d6e62303066194d79cb))
* offline pairs — offline translations for all 12 pairs (epic 3) ([#27](https://github.com/doqhu/addict/issues/27)) ([5b04e19](https://github.com/doqhu/addict/commit/5b04e194fbec33e9eb87adb3d8178cf206c30fc6))

## [0.3.1](https://github.com/doqhu/addict/compare/v0.3.0...v0.3.1) (2026-11-17)


### Bug Fixes

* upload release assets before immutable publication ([#25](https://github.com/doqhu/addict/issues/25)) ([e45682c](https://github.com/doqhu/addict/commit/e45682ccb281415cebb8cc8790eed7408d8a0f01))

## [0.3.0](https://github.com/doqhu/addict/compare/v0.2.0...v0.3.0) (2026-11-17)


### Features

* my language — choose the language of translation (epic 1) ([#21](https://github.com/doqhu/addict/issues/21)) ([5b78563](https://github.com/doqhu/addict/commit/5b78563a9b0a331dc45a397fd9a529c2b68d11d6))
* Russian Capture — recognize Russian with the rus data (epic 2) ([#23](https://github.com/doqhu/addict/issues/23)) ([ffb0df5](https://github.com/doqhu/addict/commit/ffb0df548ac9de7f3f44275cbc7b31cf7293e60a))

## [0.2.0](https://github.com/doqhu/addict/compare/v0.1.1...v0.2.0) (2026-11-12)


### Features

* **appimage:** ship an x86_64 AppImage and redesign the download dialog ([#13](https://github.com/doqhu/addict/issues/13)) ([4fb85b9](https://github.com/doqhu/addict/commit/4fb85b90df85270982cb200018fe2213efbc46a1))

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
