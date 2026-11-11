# AppImage desktop acceptance (5.7)

Accept one post-sweep CI artifact on both **Debian 13 x86_64** and
**Ubuntu 24.04 LTS x86_64**, in clean GNOME graphical accounts with no
FUSE or system Tesseract. Keep one result record per distro. A container,
mock portal, another distro, or an earlier artifact cannot fill these rows.
Use `pass`, `fail`, `blocked`, or `pending`; attach an observation or evidence
path to every completed row. A missing required portal is a blocker.

## Artifact and environment

Download `Addict-<version>-x86_64-handoff` from a successful CI run containing
the completed 5.6 changes. Record the run URL, attempt, full source commit,
artifact name and image SHA-256. Follow the
[handoff verification](appimage-build.md#ci-artifact-handoff-55), including tar,
image and corresponding-source checksums and executable mode. Keep the full
handoff, not just the executable. Run the package checker from that revision:

```sh
scripts/check-appimage.sh /absolute/path/to/Addict-0.1.1-x86_64.AppImage
```

Create disposable VM snapshots and a dedicated graphical test account. Use
the real desktop session bus and normal HOME/XDG paths for capture and login
tests. Stop other Addict installations in that account. Do not use
`dbus-run-session`, private runtime directories, a mock portal or forced
extraction variables. Leave DeepL and configured OCR executable overrides
unset. Do not replace required portal backends or install system Tesseract to
make a failed row pass.

In each graphical session, retain the output of:

```sh
cat /etc/os-release
uname -a
gnome-shell --version
printf 'session=%s desktop=%s\n' "$XDG_SESSION_TYPE" "$XDG_CURRENT_DESKTOP"
ls -l /dev/fuse
dpkg-query -W -f='${binary:Package} ${Status} ${Version}\n' 'fuse*' 'libfuse*' 'tesseract*' 'xdg-desktop-portal*'
command -v tesseract
find /usr/share /usr/local/share -type d -name tessdata -print 2>/dev/null
env | sort | rg '^(APPIMAGE[^=]*|APPDIR|ARG0|ARGV0|URUNTIME[^=]*|XDG_[^=]*|TARGET_APPIMAGE|WAYLAND_DISPLAY|DISPLAY|TESSDATA_PREFIX|GTK_PATH|GTK_MODULES|LD_LIBRARY_PATH)='
gdbus introspect --session --dest org.freedesktop.portal.Desktop --object-path /org/freedesktop/portal/desktop
```

Some probes intentionally return nonzero when a device, package, executable
or interface is absent. Retain that output; do not chain probes with `&&`.
If `rg` is unavailable, retain `env | sort` and inspect it directly. Verify
actual installed packages, filesystem paths and launch logs together; absence
of `/dev/fuse` alone does not prove absence of installed FUSE packages.
In that desktop object's introspection, check both
`org.freedesktop.portal.Background` and `org.freedesktop.host.portal.Registry`.
The latter is an interface on the same object, not a separate object path.
Record portal
frontend/backend package versions, interface versions and session type.

Place the executable in a persistent directory containing spaces, for example
`~/AppImage acceptance/Addict-0.1.1-x86_64.AppImage`, and leave it there for
the entire run. Record its absolute path and checksum on both VMs.
After observing the first ordinary launch and before testing capture, invoke
that outer file with `install` explicitly. The real host Registry requires
the matching desktop entry even for screenshot requests. Retain the installed
entry; installation must not enable startup by itself. This is the explicit
installation under test in step 6, not automatic desktop integration.

## Ordered checks on each target

1. **Ordinary launch and dictionary.** Launch the image directly, without
   flags or environment workarounds. Observe a visible main window and direct
   access to Settings. Record automatic no-FUSE extraction in its log. On the
   clean account, accept the in-app HTTPS dictionary download; do not copy an
   existing index in as a substitute. Record installed source/edition, filename,
   absolute path and SHA-256. With English selected and DeepL unset, translate
   `cat` and retain the actual Russian result and screenshot. The index must
   live outside the bundle and extraction tree.
2. **Runtime and licenses.** Identify the resident PID in this account and run
   `scripts/check-appimage.sh --trace PID NEW_EVIDENCE_DIR`. Retain its maps,
   executable, environment and parent chain. Verify GTK, libc and loader map
   to the extracted bundle; record literal APPIMAGE/APPDIR/ARG0/ARGV0 where
   present and `/proc/PID/exe`. Verify the package audit, embedded
   `share/licenses/components.json`, addict/Rust/GTK/libc/runtime licenses,
   Tesseract/tessdata licenses and Leptonica notice. Retain the corresponding
   sources beside the handoff and verify every `SHA256SUMS` entry.
3. **Real capture and OCR.** Display the three fixture lines below in a text
   editor at a readable size. Select the matching language, press the visible
   window's Capture button and select only one line using the real region
   picker. Record the actual recognized text for all three languages. Repeat
   each capture twice more, confirming the picker appears each time and only
   the chosen region becomes text. Collect successful engine/model opens and
   child mappings, for example with `strace -f -e trace=execve,openat` attached
   to the resident during capture if available. Record the actual bundled
   Tesseract child, selected eng/spa/cat model, engine, Leptonica and loader
   paths. The production executable rule resolves sibling `bin/tesseract`
   with `../share/tessdata`; filesystem layout alone is insufficient evidence.
4. **Cancellation and recovery.** Note the current field/result. Start another
   Capture and press Esc in the picker. Verify no OCR/translation starts and
   the previous result remains; then capture successfully without restarting
   Addict. If permission is refused or the backend fails, record the error and
   verify the visible focused-window recovery route. A prompt that never
   appears does not establish a denial pass. No path may silently substitute
   a full-screen image for interactive selection.
5. **Optional shortcut capability.** Only on GNOME 48+ with the actual
   GlobalShortcuts interface available, assign the screenshot shortcut in
   Settings, close the window, and repeat capture/cancel/retry from the
   shortcut. On older GNOME, mark this row outside the applicable target;
   steps 3–4 still apply. Do not upgrade the target desktop just to enable it.
6. **Persistent launcher relaunch.** Use the explicit installation above. Save the generated
   `~/.local/share/applications/io.github.doqhu.Addict.desktop` and check that
   its Exec names the persistent image with correct quoting for spaces. Exit
   the resident; if no tray Exit is available, send SIGTERM to its recorded
   PID. Confirm its process and exact extraction tree are gone. Launch the
   installed entry from the application grid and verify a fresh resident and
   visible window. Trace it again and record the outer image and new runtime.
   A direct shell launch is not evidence of launcher execution.
7. **Real login startup.** With the explicit launcher installed, enable Start
   at login in Settings through the real portal. Retain its confirmed state,
   portal diagnostics and portal-managed autostart entry/command, which must
   name the persistent image plus `--background`. Log out completely, then
   log in. Verify a new bundled resident starts silently without an app
   window. Launch from the grid and confirm the window becomes visible. Turn
   Start at login off, retain the confirmed disabled state and entry removal,
   then log out/in again and verify no resident starts. Do not write an
   autostart entry manually or replace the portal with the relaunch harness.
   Missing Background/Registry support blocks this row and final acceptance.
8. **Finish.** Preserve all evidence and remaining failures/blockers. On the
   disposable account, uninstall explicitly if desired and confirm it disables
   startup and removes the launcher without deleting downloaded dictionaries.
   Record any cleanup performed. Do not remove a daily-use installation.

Fixtures from the existing bundled OCR verification:

```text
The bank is beside the river.
El niño está cerca del río.
El camí és a prop del riu.
```

## Results

Run 2026-11-11, observer: Claude Code driving disposable QEMU/KVM GNOME
sessions through QMP input, SSH and AT-SPI. **Acceptance is not complete:
Debian 13 passed every applicable row except the optional shortcut; Ubuntu
24.04 is blocked by missing portal support.**

Artifact: CI run 43085196346, attempt 1, source commit
`182def524355413b05578991ca5e127b966657de`, artifact
`Addict-0.1.1-x86_64-handoff`; tar SHA-256
`fd65ebd077dc01a162da61875c9cf5cf4e9c9a32d459d93ea6458d067aa8c43e`; image
SHA-256 `7929354f94e83ca84d7000fd9efa265d2567694774eabb024c9f52e52e11d4b7`.
Tar, image and every corresponding-source checksum verified; the same image
checksum was confirmed on both targets. Image path on both:
`/home/tester/AppImage acceptance/Addict-0.1.1-x86_64.AppImage`.

| Evidence | Debian 13 | Ubuntu 24.04 LTS |
|---|---|---|
| CI run/attempt/full commit/image checksum | pass | pass |
| Clean graphical account, OS/GNOME/session/kernel | pass: Debian 13.7, GNOME Shell 48.7, Wayland, 6.12.115 | pass: Ubuntu 24.04.5, GNOME Shell 46.0, Wayland, 6.8.0-147 |
| No FUSE/system engine/models/OCR overrides | pass: no `/dev/fuse`, no tesseract; `fuse3`/`libfuse3` packages present | pass: no `/dev/fuse`, no tesseract; `fuse`/`libfuse2t64` packages present; both log automatic extraction |
| Real Background/Registry capabilities | pass: Background and `host.portal.Registry` present | **blocked**: Background present, `host.portal.Registry` absent (xdg-desktop-portal 1.18.4) |
| Ordinary visible launch and bundled GTK/libc/loader | pass | pass |
| In-app download, index identity, typed Russian translation | pass: 13 indexes, `cat` -> кошка | pass: identical index checksums, `cat` -> кошка |
| Complete embedded licenses and source checksums | pass (host audit, same checksum) | pass (same image) |
| English real picker/OCR and child/model paths | pass, x3, strace: bundled tesseract/libs/`eng.traineddata` | **blocked**: Capture reports "Screen capture is not available here" |
| Spanish real picker/OCR and child/model paths | pass, x3, `spa.traineddata` | **blocked** |
| Catalan real picker/OCR and child/model paths | pass, x3, `cat.traineddata` | **blocked** |
| Repeated selection, cancellation and successful retry | pass: Esc shows "No snapshot was taken", result kept, retry works in the same resident | **blocked** |
| Shortcut capture, only where applicable | pending: GNOME 48 dialog opened; assignment attempts did not complete (bind reported "Portal request didn't succeed"); not a pass | not applicable (GNOME 46) |
| Explicit launcher after process/extraction teardown | pass: quoted `Exec`, SIGTERM, tree gone, grid launch shows window | pass |
| Enable, new login, silent resident, visible grid launch | pass: portal entry names image + `--background`; new login started `addict --background` with no window; grid showed it | **blocked**: "Startup was not changed: the Background portal is not available"; no entry written |
| Disable, new login, no resident | pass: entry removed, new login started no resident | **blocked** (nothing to disable) |

Evidence: `target/appimage-acceptance-5.7/{handoff-verified.txt,package-audit.txt}`
and per-distro directories (screenshots, `trace-*`, `strace-*`,
`autostart-*`, `login-*`, `dictionary-identity.txt`).

### Ubuntu re-run after the Registry degradation fix (2026-11-11)

Artifact: CI run 43100480735, attempt 1, commit
`46405b04fb479f9650ffffa1097628cf3467bd45`, image SHA-256
`6def4e59fc8799a2c89e0c03b4b929beb884179fb4a4b93a3480d8ce3b2b03f8`; tar,
image and all source checksums verified, installed explicitly on the same
Ubuntu 24.04.5 session. The Capture button now reaches the real GNOME region
picker (the earlier "Screen capture is not available here" is gone), and the
shell writes a correct PNG of the selected region to `~/Pictures/Screenshots`.
The capture still ends without text: the portal answers `Response (2, {})` and
its log says `InteractiveScreenshot didn't return a file`. A plain Python
client that knows nothing of Addict (`target/appimage-acceptance-5.7/portal-shot.py`)
gets the identical answer after the same pick (`ubuntu/portal-probe-response.txt`),
so this is the portal/shell stack of this VM, not the application. Addict
reports it as "No snapshot was taken. Press Capture to try again." and
logs `Capture(Ended)`. A fresh account also lacked
`~/.local/share/recently-used.xbel`, which GNOME 46 logs as an error on each
screenshot; an empty valid file was created and did not change the result.
The VM has software rendering and no GPU acceleration, which was not ruled out as a cause.

Ubuntu capture, OCR, cancellation/retry and login rows therefore stay
**blocked** (now by the portal returning no file, plus the Background portal
refusal for login startup), not passed. Debian's results are unchanged and
were obtained with the earlier image (`7929354f...`); they have not been
repeated on the newer image, which differs only in the capture-connection fix.

Observations needing follow-up:

- After exit, `/tmp` keeps an empty `appimage_extracted_*.lock` and two
  dangling symlinks into the removed tree (created during `install`).
- A first Catalan-model pass over the English line read "The bant is beside the
  river." (an invalid capture caused by a helper error, not a Catalan result).
- Ubuntu 24.04 ships xdg-desktop-portal 1.18.x without the host Registry,
  so capture and login startup cannot pass there unless a newer portal is
  accepted as a prerequisite or the app degrades without the Registry.

Preparation observations, not target passes:

- Source HEAD is `ab42b0d9c896d05c537c8e62c199a9cf5a1b4e87` (5.6).
  Live hosted CI lookup found latest run 42812838854 at
  `f37c831348b0b816e9e53c040e02b9a8fcb57de6`, predating AppImage changes.
  A post-sweep CI handoff is still required.
- Final local 5.6 image checksum is
  `0092a70feb306953a28188d2113b06740de9e930971ba5991dab2c42541c7557`.
  Its metadata identifies a dirty snapshot over the prior baseline. This is
  local package/mock-smoke evidence, not hosted CI or desktop/login acceptance.
- A stopped Ubuntu 24.04.5 GNOME 46 base and an OCR overlay remain locally.
  Their earlier results apply to earlier artifacts/private mock portals.
  A new snapshot and real-session capability probe are required. No verified
  Debian 13 desktop was found in the inspected local inventory.

Store new raw observations under `target/appimage-acceptance-5.7/`, grouped
by artifact and distro. Add dates, observer and evidence paths to this record
as each real check is completed; keep failed and blocked observations.
