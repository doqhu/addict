# Controller contract

A shell calls `start(dirs, platform, start_facts, renderer)`. The controller processes events on its own thread. Each render carries a full `ViewState` and the new effects, in order. Queue the render for the UI thread without blocking. The shell may skip an older snapshot but must still run its effects in order, without waiting for one effect to finish before starting the next.

Each result-bearing request carries a `RequestId`. Return the same ID once, even when the shell can only report cancellation. `Cancel` is advisory. The controller owns deadlines and ignores stale results. Lookup, DeepL, downloads, cache and recording fetches run as Rust jobs inside the controller.

## Events

| Event | Payload and meaning |
| --- | --- |
| `PermissionChanged` | `Permissions` as observed by the shell. Renders the new values. Linux supplies grants at startup and never sends this event. |
| `Woke` | The shell resumed from sleep. Advances the snapshot revision. Linux never sends it. |
| `CopyPressed` | Copies the current source line's install hint if there is one. |
| `OpenPressed` | `url` from a link the user activated. Opens HTTP or HTTPS URLs. |
| `DismissPressed` | The user dismissed the panel, for example with Escape. Emits `HidePanel`. |
| `PlayPressed` | `article`, `url` for a recording in the shown article. |
| `Played` | `id`, `result` of playing the recording. |
| `Reopened` | A foreground launch or the tray asks to present the existing panel. |
| `PanelClosed` | The shell hid the panel in response to dismissal or a window close. |
| `QuitRequested` | The user asked to quit. The controller finishes pending writes before emitting `Quit`. |
| `FieldEdited` | `text`, `replaced` for a full replacement, and optional `source` of the replacement. |
| `CapturePressed` | Optional `activation_token` for the capture request. |
| `PickerStarted` | `id` of the area picker that has begun after the panel is hidden. |
| `AreaCaptured` | `id`, `result` containing an opaque image or failure, and optional `capture_ms`. |
| `TextRecognized` | `id`, `result` containing recognized text or failure, and `timing`. |
| `ReadPressed` | `sequence` for the Read interaction. Clears the field and requests a read. |
| `SelectionRead` | `id`, `result` containing raw `SelectionText` or failure. |
| `FieldCleared` | Clears the field in preparation for a read. Linux sends `ReadPressed` for the combined action and never sends `FieldCleared` separately. |
| `FieldSubmitted` | Submits the field, for example on Enter or paste. |
| `WordChosen` | `text` identifies the current sentence or word, and `index` selects a word. |
| `SenseChosen`, `GlossChosen` | `article`, `index` of the sense or gloss the user chose. |
| `SynonymChosen` | `article`, `word` to put into the field and submit. |
| `LookUpEnglish` | `article`, `index`, `word` of the selected English gloss link. |
| `Back`, `Forward` | Move through the field history. |
| `ReadingChosen` | Reading `Language` the user chose. |
| `MyLanguageChosen` | The user's own `Language`, chosen in Settings or in the first-run offer. |
| `SetPressed` | Reading `Language` pressed in the switch for rendering only. Linux never sends it. |
| `SetReadingSource` | Optional `Source` being read now. |
| `EnglishChosen` | `English` spelling chosen in Settings. |
| `AtOnceChosen` | Whether eligible clipboard sentences translate immediately. |
| `SettingsOpened` | Refreshes the login item and cache status. |
| `ClearCachePressed` | Clears the translation cache. |
| `SaveKey` | The entered `Secret`, or a validation error reported by the shell. |
| `KeyChanged` | The current `Secret` or a key-read error after a load or committed store. |
| `KeyStored` | `id`, `result` of storing the key. |
| `LoginItemToggled` | Whether the user wants the login item enabled. |
| `LoginItemSet` | `id`, `result` of changing the login item, and `runner_missing` when the effect could not run. |
| `LoginItemRead` | Optional `shown` login state and whether `runner_missing` prevented the read. |
| `RetryPressed` | Retry binding the shortcuts. |
| `ConfigurePressed` | Open shortcut configuration. |
| `ShortcutChanged` | `ShortcutState` reported by the shell. |
| `DownloadPressed` | Requests a dictionary download from Settings. |
| `OfferPresented` | `OfferId` of the dialog the shell has shown. |
| `OfferDownloadPressed`, `OfferNotNowPressed` | `OfferId` of the offer accepted or deferred. |
| `OfferCancelPressed` | `OfferId` and optional source name to cancel. |
| `ImportPressed` | Requests a local dictionary import. |
| `FileChosen` | `id`, `result` of the file chooser. |
| `AskAgain` | Repeats the current word's offline lookup and online requests. Linux never sends it directly. |
| `DefinitionAction` | `article`, `choice` whose selected definition the user requested. |

The architecture's `ShortcutPressed(action)` maps to `ReadPressed` for Read and `CapturePressed` for Capture. Buttons and global shortcuts use the same events. Linux may attach a Wayland activation token to a capture shortcut. `SelectionText.selection` and `SelectionText.clipboard` each hold the raw text when available. The controller applies the selection-first rule, limits, and filtering of copied install commands.

## Effects

| Effect | Payload | Completion or follow-up |
| --- | --- | --- |
| `HidePanel` | — | Fire-and-forget. Closing the window sends the lifecycle event `PanelClosed`. |
| `OpenUrl` | `url` | Fire-and-forget. Open the URL in the default browser. |
| `WriteClipboard` | `text` | Fire-and-forget. Write exactly this text. |
| `CaptureArea` | `id`, `read_sequence`, optional `activation_token` | Result-bearing. Send `PickerStarted` when the picker starts and one `AreaCaptured` when the request ends. |
| `Recognize` | `id`, `image`, `language`, `read_sequence` | Result-bearing. Send one `TextRecognized`. |
| `Cancel` | `RequestId` | Fire-and-forget. The original effect still sends its normal result. |
| `ReadSelection` | `id`, `read_sequence` | Result-bearing. Send one `SelectionRead`. |
| `ShowPanel` | Optional `read_sequence` of the Read that requested presentation | Fire-and-forget. |
| `Play` | `id`, local `path` | Result-bearing. Send one `Played`. |
| `Quit` | — | Fire-and-forget. Exit the process. |
| `PresentCapture` | `read_sequence`, `retry` | Fire-and-forget. Restore the capture window and focus the retry control if requested. |
| `RecordCapture` | `read` outcome, `timing` | Fire-and-forget. Linux evidence. |
| `ChooseFile` | `id` | Result-bearing. Send one `FileChosen`. |
| `RecordLookup` | `Answered` | Fire-and-forget. Linux evidence. |
| `RecordOnline` | Evidence `Record` | Fire-and-forget. Linux evidence. |
| `StoreKey` | `id`, `key` | Result-bearing. Send one `KeyStored`. On commit, first send `KeyChanged` with the persisted key. |
| `SetLoginItem` | `id`, `on` | Result-bearing. Send one `LoginItemSet`. |
| `ReadLoginItem` | — | Result-bearing. Report `LoginItemRead` for the current read. Linux drops a read superseded by a newer Settings opening. |
| `BindShortcuts` | — | Fire-and-forget. The shortcut worker reports binding success or failure as `ShortcutChanged`. |
| `ConfigureShortcuts` | — | Fire-and-forget. Opens the shell's configuration UI. The shell reports configuration and binding changes as `ShortcutChanged`. |

## ViewState

| Fields | What the shell renders or keeps |
| --- | --- |
| `revision` | Monotonically increasing snapshot revision. The first is 1. |
| `field` | `Field.text` and `Field.revision`. Replace the local field text only when the field revision rises. |
| `read_sequence` | Latest Read interaction, used by the shell to order presentation. |
| `ocr_field_revision` | Field revision set by recognition, if present. |
| `capture_sensitive` | Whether Capture is available and the picker is idle. |
| `source_failure`, `source_copy` | Optional failure line and the exact install text offered by Copy. |
| `permissions` | Latest `Permissions`. Linux does not expose permission controls. |
| `key_location` | `KeyLocation::File(PathBuf)` if a key file path is available, or `KeyLocation::Hint(String)` for the platform's guidance. Render this in no-key messages. |
| `shown`, `choice_revision` | Shown result and its choice. The revision also advances on a repeat click. `article_id()` identifies the current article for callbacks. |
| `online` | Word and sentence rows, with `label`, `line`, `state`, `word`, `kind` and `no_key`. |
| `window_state`, `source` | Main window status and the source of the current text. |
| `sentence`, `words`, `chosen` | Text, word byte ranges and chosen word index. |
| `can_back`, `can_forward`, `back_tooltip`, `forward_tooltip` | History button availability and tooltip text. |
| `text_number` | Identity of the current text for callbacks. |
| `reading_source` | Optional source currently being read. |
| `input`, `context` | Chosen word with its sentence if present, and optional sentence number and byte range. |
| `looking_up`, `answered` | Whether an offline lookup is running, and the optional language of the last result. |
| `settings` | Current `SettingsView` as described below. |
| `downloads` | `DownloadsView` with `indexes`, `offer`, `settings_busy`, `settings_line`, `import` and `present`. |
| `playback` | Optional `PlaybackView` with `id`, `url` and `state` (`Fetching`, `Playing`, `Ended { credit }`, `Failed` with display text). |
| `fatal` | Optional `FatalView` with `heading`, `text` and `restart` for the fatal screen. |

`SettingsView` includes `my_language`, `reading`, `english`, `english_shown`, `at_once`, `languages_read` and `my_chosen` for language choices. Key and cache controls receive `key`, `key_line`, `key_entry_clears`, `cache` and `cache_line`. Login controls receive `login_on`, `login_confirmed`, `login_sensitive`, `login_line` and `runner_missing`. Shortcut controls receive `binding`, `bindings`, `rebinding` and `status_line`.

An `OfferView` supplies `id`, `recovery`, `my_language`, `listed` and `phase` (`Listing`, `Installing` with the queue, or `Ended { heading, text, complete }`). Its `queue()` method provides the per-source outcomes when available.

## Platform and startup

| `Platform` field | Meaning and Linux value |
| --- | --- |
| `name` | Platform name. Linux supplies `Linux`. |
| `read_before_show` | Read the selection before presenting the panel. Linux supplies `false` so the Wayland window can get clipboard access. |
| `has_capture` | Whether capture is available. Linux supplies `true`. |
| `ocr_languages` | Languages offered by OCR. Linux includes English, Spanish, Catalan and Russian. |
| `install_hints` | `InstallHints` has `en`, `es`, `ca` and `ru`, each an `InstallHint` with optional `engine` and `data` strings. Linux supplies both Fedora commands for each language. An absent hint omits the install instruction and Copy. |
| `shortcut_names` | `ShortcutNames.back` and `ShortcutNames.forward`. Linux supplies `Alt+Left` and `Alt+Right`. |
| `key_location_hint` | Wording for locating the DeepL key when there is no key file path. Linux names the XDG and home directory settings. |

| `StartFacts` field | Meaning and Linux value |
| --- | --- |
| `key` | Optional key read result. Linux starts with `None` and sends `KeyChanged` after its asynchronous read. |
| `recording` | Whether to emit evidence effects. Linux sets it for `--evidence`. |
| `release` | Base URL for the dictionary release. |
| `permissions` | `Permissions.accessibility` and `Permissions.screen_recording`, each `Granted`, `Denied` or `NotDetermined`. Linux always starts with both `Granted`. |
| `login_item` | Optional `LoginItemShown` with `on` and `legacy`. Linux always starts with `None` and reads it when Settings opens. |
| `bindings` | Optional `Bindings` with optional `clipboard` and `screenshot` names. Linux always starts with `None` and reports its asynchronous binding through `ShortcutChanged`. |

`ShortcutState` can be `Bound`, `Changed` or `Rebinding`, each with `Bindings`, `Failed` with a detail, or `RebindEnded` and `ConfigureAnswered` with an optional message. The controller applies known startup login and binding values before the first snapshot, using the same transitions as later reports. The shell owns permission prompts and reports the permission state it observes.

## Linux shell boundary

`addict-app` keeps the GPU probe, About, the tray icon and worker, GTK widgets, the evidence writer, activation tokens, and the Hyprland snippet copy in Settings. The shell owns transient presentation such as focus and the Copy button's “Copied” label. It opens the Commons link, writes clipboard text and closes the panel when running controller effects. The close-request handler reports `PanelClosed`. With no controller, the shell handles Copy, link activation and Escape locally.
