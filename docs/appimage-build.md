# Building the x86_64 AppImage

For the final two-distro checks and current results, use the
[AppImage desktop acceptance record](appimage-acceptance.md).

This recipe packages the production `addict` executable with GTK 4.20 and
libc. It leaves dictionary indexes in the normal shared application data
path, `$XDG_DATA_HOME/addict` (default `~/.local/share/addict`). OCR,
updates, publication and complete dependency notices are later
work. Do not distribute this experimental artifact as a finished release.
The Flatpak recipe and application behavior are unchanged.

## Inputs and prerequisites

Use an x86_64 Linux host with rootless Podman, Bash, Git, Python 3.11 or
later, GNU tar/coreutils and HTTPS access to Fedora, GitHub and crates.io.
Allow several GiB of disk space and memory for the release build; the
notices added by 5.4 need further hosts and space, listed under its
Prerequisites below. Rootless
Podman must already work; no privileged container or desktop bus is needed
to build. No Rust installation or FUSE is needed on the host.

`packaging/appimage/inputs.lock.json` pins the Fedora 43 OCI digest, every
RPM in the resolved package closure (including the Rust compiler and Cargo)
and the quick-sharun, sharun, cross-libc-dlopen and appimagetool downloads.
Each download is SHA-256 checked, including cached inputs. The Containerfile
installs only those local RPMs with repositories disabled. No mutable `latest`
lookup or replacement input is allowed. Cargo uses `Cargo.lock`, a fresh
Cargo home and a fresh target directory; crate checksums are checked by Cargo.
The lock records exact inputs, not a claim of byte-for-byte reproducible output.

Fedora 43 is the build environment, as planned. Its newer bundled libc and
GTK are intentional: sharun starts the executable with the bundled loader.
Fedora Mesa and LLVM are retained (`I_WANT_FAT_APPIMAGE=1`) to include
software rendering instead of mixing in another distribution's driver build.
The GLES library and Vulkan CPU driver (lavapipe) are explicit runtime inputs;
the checker and refusal fixtures require them, including their ICD metadata.
This increases image size; GPU/kernel compatibility on the older target
desktop still requires the acceptance below. Fontconfig links are dereferenced
when staged so they cannot escape the bundle. The bundle opts into
`USE_HOST_XDG_CACHE_HOME=1`, preserving shared application cache paths as
well as configuration and data paths.
The output component and RPM inventories are inputs to the later notices
story; they do not replace that story's license audit.

## Clean checkout build

```sh
git clone https://github.com/doqhu/addict.git
cd addict
scripts/build-appimage.sh
```

The launcher snapshots tracked and nonignored source files into a temporary
build directory, records its revision, source archive checksum and dirty
status, builds the pinned container without layer reuse, and runs exactly:

```sh
cargo build --release --locked -p addict-app --bin addict
```

It uses no test features, existing target directory or existing Cargo cache.
It caches only checked external inputs under `target/appimage-inputs`.
For an entirely uncached run, remove that directory before building.

Outputs appear under `target/appimage/` only after inspection succeeds:

- `Addict-<version>-x86_64.AppImage` and `.sha256`;
- `.build.txt`: revision, source checksum/status, compiler, Cargo and lock hashes;
- `.inputs.lock.json`, `.rpms.txt`, `.components.sha256` and `.layout.txt`:
  locked inputs, installed packages, all bundled file hashes and inspected layout.

```sh
cd target/appimage
sha256sum --check Addict-*.AppImage.sha256
cd ../..
scripts/check-appimage.sh target/appimage/Addict-*.AppImage
packaging/appimage/tests.sh
```

The checker uses the runtime's extraction command, without FUSE. It checks
ELF architecture, loader, GTK/libc, schemas, fonts, icons, modules, symlink
containment and dictionary/OCR data contamination (including renamed indexes
with the `ADDICTIX` header). Refusal tests use disposable layout files, not
published dictionary fixtures. These checks do not prove a visible window or
translation on a desktop.

Failures name their step and input/component. Pinned RPMs are fetched from the
signed copies that Fedora's Koji keeps for each build, so a package update does
not remove them. If one is still missing, restore that exact checked file or
deliberately review a new lock; do not switch to a current package silently.
Bad cached checksums fail; remove only the named cached input and retry. Set
`ADDICT_APPIMAGE_KEEP_WORK=1` to retain the temporary source/build workspace
for diagnosis; each later build still starts with a fresh directory. Existing
outputs are cleared before the first check of a build, so a failed rebuild
cannot report an old image as new.

## Layout and automatic no-FUSE fallback

Quick-sharun stages the real executable at `shared/bin/addict`; `bin/addict`
and `AppRun` are sharun launchers. Libraries live under `lib`, data under
`share`, and configuration under `etc`. `.layout.txt` records the actual
validated tree. Do not use this description as a substitute for process evidence.

The pinned appimagetool contains uruntime 0.8.1 and DwarFS. The launcher
changes its single embedded `URUNTIME_EXTRACT=3` setting to `=2` before
checksumming: automatic extraction after failed mounting is unrestricted by
the default 350 MiB size cap. It preserves ordinary launch and does not force
`APPIMAGE_EXTRACT_AND_RUN`. The checker explicitly extracts only for inspection.
See the pinned [quick-sharun source](https://github.com/pkgforge-dev/Anylinux-AppImages/blob/ce344325ab1db718a7ad4ce9868a92a38d4af12a/useful-tools/quick-sharun.sh)
and [uruntime source](https://github.com/VHSgunzo/uruntime/blob/v0.8.1/src/main.rs).

## Isolated desktop acceptance

Run this on a disposable **Ubuntu 24.04 LTS or Debian 13 full desktop VM**
(GNOME or KDE), with no FUSE device available. A headless display or a host
Fedora window is useful diagnostics but does not satisfy this acceptance.
Record `/etc/os-release`, desktop/version, kernel, `ls -l /dev/fuse`, image
SHA-256 and the build metadata. Transfer the image, its inventories and
`scripts/check-appimage.sh` to the VM, into one directory.

Use a fresh absolute profile and private desktop bus; do not invoke `addict
uninstall` against your normal profile. In a VM terminal:

```sh
image=$(realpath ./Addict-*-x86_64.AppImage)
profile=$(mktemp -d)
mkdir -p "$profile"/{home,data,config,cache,runtime}
chmod 700 "$profile/runtime"
if [ -n "${WAYLAND_DISPLAY:-}" ]; then
  ln -s "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" "$profile/runtime/$WAYLAND_DISPLAY"
fi
env -u APPIMAGE_EXTRACT_AND_RUN -u APPIMAGE_TARGET_DIR -u TARGET_APPIMAGE \
  -u DBUS_SESSION_BUS_ADDRESS -u GTK_PATH -u GTK_MODULES -u LD_LIBRARY_PATH \
  HOME="$profile/home" XDG_DATA_HOME="$profile/data" \
  XDG_CONFIG_HOME="$profile/config" XDG_CACHE_HOME="$profile/cache" \
  XDG_RUNTIME_DIR="$profile/runtime" \
  dbus-run-session -- "$image" > "$profile/launch.log" 2>&1 &
```

The Wayland socket link above uses the VM compositor (keep `WAYLAND_DISPLAY`
set). On X11 keep `DISPLAY` and pass the VM's absolute `XAUTHORITY` path when
launching, since the fresh home has no authority file. Use the
VM desktop's socket, never a host socket. The private bus prevents forwarding
to a resident instance. Keep the profile directory for the evidence.

Observe the window. Use the existing first-run or Settings dictionary download
flow over HTTPS, installing a bilingual source such as FreeDict English to
Russian (`freedict.index`). Record its source/edition, filename, SHA-256 and installed absolute
path under `$profile/data/addict`, outside every image/extraction path. Select
English, type a known word such as `cat`, and record the actual Russian result
(for example `кот`/`кошка`) and a screenshot. Leave the DeepL key unset. A
network download performed outside the application does not establish this check.

Find the resident `addict` PID in this private bus/profile. Before exiting,
run `./check-appimage.sh --trace <pid> "$profile/evidence"` to
save its maps, executable, command line, parent process chain and packaging
environment. Check that mapped `libgtk-4`, `libc.so.6` and loader paths belong to the
bundle's extracted tree, not the VM's older GTK/libc. Record `APPIMAGE`,
`APPDIR`, `ARGV0`/`ARG0` when present, and `/proc/<pid>/exe` literally.
Confirm which variable still names the persistent **outer `.AppImage` file**
after extraction; do not infer it from the staged tree or assume that
`current_exe` identifies the outer file. This evidence is needed by later
startup/update stories. Close the app and record exit/cleanup behavior. The
close button only hides the resident, and tray Exit is unavailable on the
private bus, so end it with `kill -TERM <pid>` and then check that the
`appimage_extracted_*` tree is gone.

## Verification record

### Persistent launcher and login startup checks

`addict install` is explicit: it installs a visible launcher pointing at the
canonical persistent outer `APPIMAGE`, with no background flag. The Settings
startup switch uses the same target with `--background` through the Background
portal. Nothing automatically installs or repairs a launcher. Keep the image
at that location. After moving it, run the image's `install` command from its
new location, then turn startup off and on in Settings to replace the portal's
saved command. Deleting or moving the image cannot automatically repair a
command that already points to its old location. Disable and uninstall do not
need to resolve the old image target.

For a repeatable relaunch check on the disposable target desktop, Python 3
with PyGObject/Gio must be available as verification tooling (they are not
application dependencies):

```sh
packaging/appimage/tests.sh --relaunch IMAGE NEW_EVIDENCE_DIR
```

This copies the image into `images with spaces` under the new evidence
directory, installs against disposable HOME/XDG directories and uses a private
bus with no service activation directories. Its mock implements Registry and
Background only; only that mock writes/removes the disposable autostart entry.
It never contacts the desktop's actual portal. Follow the current instruction
in `stage.json` or the terminal:

1. Observe the normal window, dismiss the optional dictionary dialog, open
   Settings and enable Start at login. The harness captures the real request.
2. After it stops the resident and proves the extraction tree disappeared,
   it launches the installed desktop entry again. Observe the normal window
   and create `NEW_EVIDENCE_DIR/launcher-confirmed`.
3. After another complete cleanup, it launches the portal-written desktop
   command. Confirm that no app window appeared and create
   `NEW_EVIDENCE_DIR/background-confirmed`.
4. Open Settings (the app's `settings` action is available on its private bus),
   turn startup off and on, and let the harness stop the resident and uninstall.

To open Settings for the silent resident without first opening its main
window, use the private bus address recorded by the harness:

```sh
DBUS_SESSION_BUS_ADDRESS="$(cat NEW_EVIDENCE_DIR/bus-address)" \
  gdbus call --session --dest io.github.doqhu.Addict \
  --object-path /io/github/doqhu/Addict \
  --method org.gtk.Actions.Activate settings '[]' '{}'
```

Each observation is bounded to five minutes. `PASS` is written only after
both commands have launched fresh bundled residents after cleanup, the
Settings requests were true/false/true, and uninstall requested false before
removing its launcher. Preserve screenshots for normal/silent observations,
`portal.json`, `saved-command.json`, `cleanup.log`, the process traces and
image checksum. This check uses an isolated mock portal; it does not certify
the target desktop's actual login session or portal implementation.

Persistent relaunch acceptance, recorded 2026-11-11:

- Fresh pinned build from revision
  `9fe8bc24e125a19326b1025d1e4c8f127f260c2a` plus the uncommitted 5.2 Rust
  changes. Artifact SHA-256:
  `c15a5292ca7802f3ddcaa945906265778b8f60cb64653928250e2795d8c39220`.
  Source snapshot SHA-256:
  `5e5013045ff30d43ad8487705a62f356d5c27ab16e849cff20b15aab375aba05`.
  Extraction/layout checks and packaging refusal fixtures passed.
- Disposable snapshot of the earlier Ubuntu 24.04.5 GNOME desktop, kernel
  6.8.0-147-generic, no `/dev/fuse`. The new image was copied to
  `/home/tester/relaunch-5.2/images with spaces/Addict-0.1.1-x86_64.AppImage`.
  All HOME/XDG paths and the Registry/Background mock bus were private.
- Explicit install saved the outer image, without flags. Desktop-entry
  launch produced resident PID 2444 and a visible normal window. Settings
  enable registered and sent the quoted outer image plus `--background`.
  After SIGTERM, the resident and its
  `/tmp/appimage_extracted_Addic1555832400808022495` tree disappeared.
- Launching the installed desktop entry after cleanup produced fresh resident
  PID 2734 and the visible normal main window. After another confirmed cleanup,
  launching the portal-written entry produced PID 2935 with no app window.
  The three process traces show the extracted executable and persistent
  `APPIMAGE`; the reused extraction pathname had been absent between runs.
- Settings off was confirmed as `disabled`; its portal command was empty.
  The subsequent on request again named the persistent image. After final
  resident/extraction cleanup, uninstall registered while the launcher still
  existed, sent an empty disable command, and removed the launcher, icons and
  saved state. Portal request sequence: true, false, true, false. Harness PASS.
- Evidence: `target/appimage-relaunch-5.2/` contains the guest log and process
  traces, portal payloads, three cleanup records, SHA-256/host records, and
  screenshots of the initial window, launcher relaunch, silent background
  relaunch and disabled switch. The final harness additionally retains a
  launcher snapshot and waits for the final on state to be saved; those two
  small verification improvements were added after this recorded run.
- `scripts/check.sh`, focused startup/entries/install/uninstall tests,
  `packaging/appimage/tests.sh`, shell/Python syntax and `git diff --check`
  passed. Private-bus tests require running outside the restricted sandbox.
  Real desktop login startup and real portal behavior were not exercised.

Post-review final artifact, verified again on 2026-11-11:

- Review found that a mode-bit check could accept an image the current user
  could not execute (owner mode `0641`). The resolver now uses
  `faccessat(X_OK, AT_EACCESS)`; the exact refusal case is covered by the
  isolated installation test, preserving all existing profile files.
- Rebuilt after that fix: artifact SHA-256
  `76b159f838597f6ac32695fa6d3f5a7d303d4d9f9d1915ba43f063b0a279cfcb`;
  source snapshot SHA-256
  `80d1825b6baf32f65f951e0b1c8b58ddf093b3d11bd6aa1b78d572ca8bf4ed6e`.
  `scripts/check.sh`, packaging refusals and extraction validation passed again.
- Parent repeated the complete desktop harness with the final code and harness
  on a new disposable Ubuntu snapshot. Explicit install used
  `/home/tester/relaunch-5.2-final/images with spaces/Addict-0.1.1-x86_64.AppImage`.
  Initial/launcher/background residents were PIDs 2427, 3074 and 3354.
  `/tmp/appimage_extracted_Addic2048419049103469335` was confirmed absent after
  each stop, before the next launch. The launcher opened a normal window;
  the portal command launched silently. Settings off/on and uninstall passed,
  with portal sequence true/false/true/false. Final harness result: PASS.
- Evidence: `target/appimage-relaunch-5.2-final/`, including launcher text,
  portal payloads, checksums, process traces, cleanup records and screenshots.
  The VM was powered off. This final run retains the same private mock-portal
  limitation: real desktop login and the system portal were not exercised.

Desktop acceptance **passed** on 2026-11-10 on a disposable Ubuntu 24.04 GNOME
VM (record below). Repeat it on the target desktop for each new artifact; do
not record a headless launch as passing it.

A repeatable launch/trace diagnostic is also available as
`packaging/appimage/tests.sh --runtime IMAGE NEW_EVIDENCE_DIR`; it uses the
caller's display with a fresh profile and private bus, records paths, then
closes the diagnostic instance. It cannot certify visible UI or translation.

Recorded 2026-11-10:

- Cold build: fresh source, Cargo home, target and uncached container build;
  baseline revision `1ec2fd795ac008f3be38edeaadafd0f217e8d3b5`, with these
  packaging changes uncommitted. Rust/Cargo 1.99.1; GTK 4.20.4; 384 pinned RPMs.
  The exact final launcher completed from fresh source/Cargo/target
  directories, followed by extraction and runtime checks.
- Final artifact: `Addict-0.1.1-x86_64.AppImage`, SHA-256
  `42c6b33f0179bc7ee8fe239f5f651b948ebda133662fe676aa8aef18fb85b9b5`.
  SHA-256 verification, artifact extraction/layout validation, shell syntax,
  refusal fixtures, `git diff --check` and `reuse lint` passed.
- Runtime diagnostic: Ubuntu 24.04.4 container, pinned toolbox digest
  `8db97043cc5f028a64d00d604c627b86a839a493a4d3d9ae2e9b2d6755e49fac`,
  with a private bus/profile, no `/dev/fuse` or GPU device, displaying through
  the Fedora 45 host's GNOME 51.1 Wayland socket. This is **not full target
  desktop acceptance**. The ordinary invocation unset extraction controls;
  the log shows failed FUSE followed by successful automatic extraction.
  uruntime itself set `APPIMAGE_EXTRACT_AND_RUN=1` after that fallback.
- Observed executable: `/tmp/appimage_extracted_Addic17395649457895943886/bin/addict`.
  `APPDIR` named that temporary root. Mapped GTK
  `lib/libgtk-4.so.1.2000.4`, libc `lib/libc.so.6`, loader
  `lib/ld-linux-x86-64.so.2` and GLES `lib/libGLESv2.so.2.1.0` all belonged
  to it. The staged real binary remains `shared/bin/addict`; the process
  reports the `bin/addict` alias. Neither is the persistent outer file.
- `APPIMAGE` and `ARG0` both named `/artifact/Addict-0.1.1-x86_64.AppImage`,
  the persistent outer file. HOME and XDG data/config/cache paths were all
  checked against the fresh profile and preserved exactly; index storage
  would be `/evidence/clean-verified-run/data/addict`, outside the image.
- Local raw trace: `target/appimage-runtime-ubuntu24/clean-verified-run/` (log,
  host record and process maps/executable/parents/environment). Bundle resources
  and file hashes are in the emitted `.layout.txt` and `.components.sha256`.
  Expected private-container warnings reported missing accessibility, portals,
  tray host and physical GPUs; the resident survived and the library/path
  assertions passed without forcing a renderer.
- The container diagnostic above installed no dictionary and claims no
  translation; the desktop acceptance below does.

Desktop acceptance, recorded 2026-11-10:

- Target: disposable KVM guest from the signed Ubuntu 24.04 cloud image
  (`noble-server-cloudimg-amd64.img`, SHA-256
  `04e8f2461a16a44e78a370284471f29616cdd9baf1893df25807b71f985b6c94`, SHA256SUMS
  signature from the UEC Image Automatic Signing Key) plus
  `ubuntu-desktop-minimal`: Ubuntu 24.04.5 LTS, kernel 6.8.0-147-generic,
  GNOME Shell 46.0 on Wayland with automatic login. Its own GTK is 4.14.5 and
  glibc 2.39. FUSE was unavailable: no `/dev/fuse`, `fusermount3` mode 0000.
- Image: the SHA-256 file above verified in the guest. Launched with the
  procedure above, verbatim, from the guest's GNOME session. The log shows
  `failed to utilize FUSE during startup!` then automatic extraction; no
  extraction variable was set by the caller.
- Window: the main window and the first-run "Download dictionaries" dialog
  appeared on the GNOME desktop. Its "Download" installed all 13 offered
  indexes from release `indexes-v3` over HTTPS. `freedict` (English to
  Russian; manifest asset SHA-256
  `32f2f6064b96528e483693a5b8d9b9dd2e6af2aef2288a190899a5d505db8bb0`) was
  installed as `$profile/data/addict/freedict.index`, SHA-256
  `511d72bbac3d94b5b217d3f99a916649bcd2106d0484b7bf27e1a09e9524439a`, outside
  the extraction tree, which held no `.index` file.
- Translation: with EN selected and no DeepL key, typing `cat` showed
  `кóшка` with alternatives `кошка` and `кот`, marked "Offline dictionary".
- Process: `/proc/<pid>/exe` was
  `/tmp/appimage_extracted_Addic17395649457895943886/bin/addict` and `APPDIR`
  named that root. Mapped `libgtk-4.so.1.2000.4`, `libc.so.6`,
  `ld-linux-x86-64.so.2`, `libGLESv2.so.2.1.0` and `libglib-2.0.so.0.8600.5`
  all came from it; no system GTK or libc was mapped. `APPIMAGE` and `ARG0`
  both named `/home/tester/Addict-0.1.1-x86_64.AppImage`: the persistent outer
  file signal is `APPIMAGE`. uruntime set `APPIMAGE_EXTRACT_AND_RUN=1` itself.
- Expected limits of the isolated run: the shortcut banner reported that GNOME
  46 has no `org.freedesktop.host.portal.Registry`, and the tray could not
  register on the private bus. Startup and desktop integration belong to later
  stories.
- Exit: the close button hid the window and the resident kept running, as
  designed. Tray Exit was unavailable on the private bus, so the resident was
  ended with SIGTERM. Within 34 seconds the runtime processes exited and the
  extraction tree was removed; an empty
  `/tmp/appimage_extracted_Addic17395649457895943886.lock` file remained. The
  installed indexes stayed in the profile.
- Koji URLs: after the lock moved to Koji, a build from a fresh clone with an
  empty input cache fetched and verified all 388 inputs and validated a new
  artifact (SHA-256
  `1b1e24976307db514dd43609cd832d9521b97796fde50f89c09c8d29c2c18655`). Its
  installed RPM list is identical to the accepted artifact's, but five bundle
  files differ (`shared/bin/addict`, `lib/libp11-kit.so.0.4.11`, two GStreamer
  helpers, a sharun hook), so builds are not bit-for-bit reproducible. The
  desktop acceptance above applies to the `42c6b33f…` artifact.
- Raw evidence: `target/appimage-runtime-ubuntu24-desktop/` (screenshots,
  launch log, process trace, host record, exit record, release manifest and
  the guest's cloud-init configuration).

## Bundled OCR and repeatable capture verification (5.3)

The pinned Fedora 43 input set adds Tesseract 5.5.3-2.fc43, Leptonica
1.87.0-1.fc43 and their resolved RPM dependencies (391 RPMs total). The three
models and Apache license use the same tessdata_fast 4.1.0 URLs and SHA-256
pins as the unchanged Flatpak manifest. The build verifies every RPM, tool,
model and downloaded license before use, and installs with repositories and
weak dependencies disabled. There is no mutable download fallback.

Build and inspect with:

```sh
scripts/build-appimage.sh
packaging/appimage/tests.sh
scripts/check-appimage.sh target/appimage/Addict-0.1.1-x86_64.AppImage
(cd target/appimage && sha256sum --check Addict-0.1.1-x86_64.AppImage.sha256)
```

The refusal fixtures require the verified input cache populated by the build.
The final layout has executable `bin/tesseract` (the sibling launcher) and
`shared/bin/tesseract` (engine), bundled `lib/libtesseract.so*`,
`lib/libleptonica.so*` and their complete ELF dependency closure. Only
`share/tessdata/{eng,spa,cat}.traineddata` are allowed anywhere in the image:

| Model | SHA-256 |
| --- | --- |
| eng | `7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2` |
| spa | `6f2e04d02774a18f01bed44b1111f2cd7f3ba7ac9dc4373cd3f898a40ea6b464` |
| cat | `250db73cd5b380d2798581295dc12f20d0828cdb335a65d833d12dfdbf57117d` |

Quick-sharun's implicitly copied `share/tesseract` and `share/tessdata`
contents are replaced explicitly. `share/ocr-components.sha256` records the
packaged engine and every recursively resolved ELF dependency; validation
checks those contents as well as model pins and executable paths before build
success metadata is emitted. The normal complete file/RPM inventories remain.
Upstream engine, Leptonica, giflib and tessdata notices are retained under
`share/licenses/ocr`. This is not the complete 5.4 notice audit.

On a disposable Ubuntu 24.04 desktop **without host Tesseract, model trees or
`/dev/fuse`**, install verification tooling only:

```sh
sudo apt-get install python3-gi python3-pyatspi at-spi2-core strace dbus-x11
packaging/appimage/tests.sh --ocr /absolute/path/Addict-0.1.1-x86_64.AppImage \
  /absolute/path/new-ocr-evidence
```

Use the desktop session's actual `DISPLAY` and `XAUTHORITY` (omit the latter
assignment when inherited correctly); GNOME Xwayland is supported. Transfer
`scripts/check-appimage.sh`, `packaging/appimage/{tests.sh,ocr.py,inputs.lock.json}`
and `packaging/appimage/fixtures/*.png` with their relative repository paths.
Python, Gio, AT-SPI, strace and D-Bus are verification tools, not application
requirements. Missing tools and host OCR contamination produce named refusals.

The harness first validates the supplied image. For each language it creates
fresh HOME/XDG directories and a private session bus without system portal
activation. Only its private Screenshot/Registry mock and the standard AT-SPI
service are available. There are no OCR configuration overrides; the normal
language preference selects EN, ES or CA. AT-SPI presses the production Capture
button and reads the resulting GTK field. The production app receives the
known PNG URI from Screenshot, performs the existing capture/OCR path, and
removes the snapshot. The known texts, including Spanish and Catalan accents,
are asserted exactly. It records successful model opens with strace and actual
child executable/arguments/loaded-library paths through `/proc`, then bounds
resident/extraction teardown. `PASS.json` appears only after all three outputs
and cleanup succeed. A reused evidence directory is refused.

Fixtures are verification resources and never enter the bundle. Regenerate
with `python3 packaging/appimage/fixtures/generate.py` (Pillow and Liberation
Sans 2.1.5; font SHA-256
`76d04c18ea243f426b7de1f3ad208e927008f961dc5945e5aad352d0dfde8ee8`).
The checked PNGs make repeated verification independent of renderer/font
availability on the target machine.

This verifies packaged capture and OCR with a deterministic Screenshot
response. It does not exercise real area selection, the desktop's Screenshot
portal implementation, or the final two-distribution acceptance of 5.7.

Executed OCR verification, 2026-11-11:

- Fresh production build from baseline revision
  `48fdcd07baf26526bed4ddb46a72a111ff810827` with the 5.3 packaging changes;
  source snapshot `98f039dcb1b6bc3ccb56af922d7cba41f46167fcbd087ba57cd7e5b715fc73e0`.
  AppImage SHA-256:
  `b65a029d4c2dd173a61b18fae9fc995d07548116a6fed47ce62476ea39d8317f`.
  The final fixture/harness refinements are verification resources outside the
  bundle. No Rust or Flatpak changes were needed.
- Disposable overlay of the existing Ubuntu 24.04.5 GNOME desktop base,
  kernel 6.8.0-147-generic. The base was not modified. Only verification
  packages were added to the overlay; it had no host Tesseract, system
  tessdata tree or `/dev/fuse`. Xwayland displayed the normal production UI.
  Each launch log records failed FUSE followed by automatic extraction.
- The final command, `tests.sh --ocr IMAGE /home/tester/ocr-5.3-verified`,
  passed all three exact GTK field assertions:
  `The bank is beside the river.`, `El niño está cerca del río.`, and
  `El camí és a prop del riu.`. Actual OCR child PIDs were 5077, 5166 and
  5320, each executing
  `/tmp/appimage_extracted_Addic11528054946601516958/bin/tesseract` with
  `--tessdata-dir .../bin/../share/tessdata` and its matching `-l eng/spa/cat`.
  Successful model opens identify the canonical bundled model per language.
  Every observed child shared-library mapping belonged to the extracted
  bundle, including Tesseract, Leptonica and libc.
- Each resident terminated and its extraction tree was confirmed absent
  before the next language started. The runtime reused the same extraction
  pathname only after that absence check. `PASS.json` was written after all
  three cleanup checks; the VM was powered off after collecting evidence.
- Earlier diagnostics are not acceptance: an AT-SPI startup-cache race timed
  out before Capture, and the first Spanish fixture's `pingüino` was read as
  `pinguino`. The harness now clears accessible-node caches; the final Spanish
  fixture uses `niño` while retaining exact accented expectations. Those
  failed runs emitted no PASS. Their logs are retained with the final evidence.
- Evidence: `target/appimage-ocr-5.3/ocr-5.3-verified/` contains the image/host
  records, private bus definition, each profile's actual text, production
  application evidence, strace calls, child maps, fixture hashes and cleanup
  records. The archive in its parent directory also retains diagnostic logs.
  The checked PNGs were rendered with Pillow 12.3.0 and the font pin above.
- Packaging refusal fixtures passed for missing/corrupt pinned inputs,
  missing/corrupt engine/Leptonica/models, unexecutable launchers,
  extra/relocated models, dictionary headers/extensions, symlink escapes and
  wrong ELF architecture. Final artifact extraction validation and SHA-256
  verification, shell/Python syntax, `git diff --check` and `reuse lint` passed.

Post-review final artifact, independently verified 2026-11-11:

- Review reproduced a missing-SONAME case: deleting `lib/libtesseract.so.5.5`
  left its payload present and initially escaped validation. The OCR inventory
  now retains required dependency pathnames as well as resolved files. Both
  the refusal fixture and deletion from the real final extracted image fail
  with `missing OCR dependency: lib/libtesseract.so.5.5`.
- A fresh build after that correction passed all package checks. Artifact SHA-256
  `e9873c9977296aeccae7ab131f3f0d097f92dffe47a25d2dee6db1e2ef6f6f56`;
  source snapshot SHA-256
  `cd03c52c34872e771ea13132c1f12937f71b17147aab91d06f1186ad65cad482`.
- A rebooted VM exposed an accessibility-tool initialization race before
  Capture. The unbundled harness now activates its private accessibility bus,
  queries the Registry root and selects that explicit address before importing
  pyatspi or launching the app. This uses no retry and preserves all assertions.
- The parent rebooted the Ubuntu overlay again and reran the complete final
  harness. English, Spanish and Catalan exactly matched the texts above;
  actual OCR child PIDs were 2211, 2305 and 2469. Successful model opens and
  child library maps belonged to
  `/tmp/appimage_extracted_Addic15801695028444076955`; the extraction tree
  was confirmed absent between all launches and after the last one.
- Final evidence: `target/appimage-ocr-5.3/ocr-5.3-parent-reboot/` and its
  `.tgz` archive, including `PASS.json`, image checksum, host record, traces,
  actual UI text and cleanup. The VM was powered off. Shell/Python syntax,
  refusal tests, final image extraction/checksum, whitespace and REUSE checks
  passed. The deterministic Screenshot and two-distro acceptance limitations
  remain as described above.

## Notices for every bundled component (5.4)

The build attributes every regular file and symlink of the AppDir, and the
AppImage runtime header, to a component, installs that component's license
texts and writes a machine-readable inventory,
`share/licenses/components.json`, which is also kept beside the artifact as
`<name>.notices.json`. `packaging/appimage/notices.py` runs inside the pinned
container after quick-sharun's deployment and before AppDir validation.

### Prerequisites

Besides the hosts listed above, the build downloads from
`kojipkgs.fedoraproject.org` (source RPMs), `raw.githubusercontent.com`,
`deb.debian.org` (the dash tarball), `static.crates.io`,
`static.rust-lang.org`, `ziglang.org`, `ftp.gnu.org`, `www.gnu.org` and
`musl.libc.org`. The first build fetches about 2.3 GB of source RPMs and
0.3 GB of other sources into `target/appimage-inputs/`; each build copies
them once more into its temporary workspace (`mktemp -d`, usually `/tmp`) and
once into `target/appimage/<name>.sources/`, so allow about 8 GB beyond the
earlier build. The refusal tests (`packaging/appimage/tests.sh`) need no
`rpm` on the host: they give build-time attribution a fixture owner map.

### Attribution

Attribution is derived from the pinned build environment, not a hand-kept
list:

- Fedora files: the installed RPM headers (`rpm -qa` file names, license
  flags, `SOURCERPM` and `LICENSE`) own an AppDir path through this prefix
  map, the first owned candidate winning, also through resolved system
  symlinks (usrmerge, xkb):

  | AppDir prefix | System prefixes, in order |
  |---|---|
  | `shared/bin/` | `/usr/bin/`, `/usr/libexec/`, `/usr/libexec/gstreamer-1.0/` |
  | `bin/` | `/usr/bin/`, `/usr/libexec/` |
  | `lib/gstreamer-1.0/` | `/usr/libexec/gstreamer-1.0/` |
  | `lib/` | `/usr/lib64/`, `/usr/lib/` |
  | `share/` | `/usr/share/` |
  | `etc/` | `/etc/` |

  Paths, not digests, are compared, since quick-sharun rewrites deployed
  files. An RPM bump therefore re-derives the components.
- Explicit rules cover only the project's own files, relocated notices
  (`share/licenses/ocr/<rpm>/`), files of the pinned tools (`sharun` and its
  copies as `AppRun` and `bin/*` launchers, `dash`, `lib/sharun-preload/*`),
  quick-sharun's generated layout (`AppRun.sh`, `AppRun.lib`, `.env`,
  `bin/*.hook`, `lib/lib.path`, `shared/lib`, `etc/ssl/openssl.cnf`) and two
  caches (`gschemas.compiled` -> glib2, Adwaita `icon-theme.cache` ->
  adwaita-icon-theme).
- A file that matches neither fails the build before any success metadata,
  naming the path (`unattributable file: ...`).

Each component has an origin, an NEVRA or pinned inputs, an SPDX expression,
its license texts under `share/licenses/<component>/` and a source record;
`files` maps every AppDir path to its component. The SPDX expression is the
RPM `LICENSE` tag for Fedora packages; addict's is MIT plus every license
heading of `packaging/licenses/crate-licenses.txt`, read at build time.
Components whose binary RPM has no `%license` file take the texts of an
installed sibling package built from the same source RPM, else the upstream
license files of the pinned source RPM itself (`%license` names and
top-level `COPYING`/`LICENSE`-style files of its source archives). libdrm and
libglvnd ship no license file upstream: their texts are the license header
of `xf86drm.c` and libglvnd's `README.md` licensing section, read from the
pinned source RPMs. No text comes from a generic template when the
component's own text exists.

### Statically linked code

The prebuilt tools carry static code of their own. Each library linked into
them is a component (`origin: static`, `embedded_in` naming the tool), with
its own license texts and pinned source; the tool's component records how
its closure was determined (`static_closure`). The record is
`packaging/appimage/static-closures.json`, generated by
`packaging/appimage/closures.py` and committed; rerun it after bumping
uruntime/appimagetool, Anylinux-sharun or DwarFS and review the diff. Only
regenerating it needs host cargo (it sets `RUSTC_BOOTSTRAP=1` to read the
tools' nightly manifests) and network access; the build itself does not.

- Runtime header: uruntime 0.8.1, variant `appimage-dwarfs-lite` (its only
  embedded helper is `dwarfs-fuse-extract`, whose payload checksum matches
  uruntime's `checksums.txt`). Its Rust crates come from uruntime's pinned
  `Cargo.lock` with those features for `x86_64-unknown-linux-musl` (normal
  dependencies; proc-macro crates and other build-time code excluded); its
  standard library from the `rust-src` of the nightly the header's
  `.comment` names (2026-09-24); musl 1.2.5 and compiler_rt from Zig 0.16.0,
  its linker.
- DwarFS helper: `dwarfs-fuse-extract` 0.15.7 links boost, libarchive,
  LibreSSL, libfuse3, liblzma, zstd, xxHash, jemalloc, fmt, nlohmann/json,
  utfcpp, range-v3, parallel-hashmap, the GCC 16.1.0 runtime (libstdc++,
  libgcc) and musl 1.2.6, at the versions DwarFS's own static build pins
  (`.docker/static-libs-versions.sh`, `toolchain-versions.sh` in the pinned
  DwarFS source), each confirmed by the binary's strings. lz4, brotli, flac,
  ricepp, fuse2, mimalloc, OpenSSL and the stack-trace libraries are not in
  it; the reasons are recorded in `static-closures.json`.
- `sharun` 3.8.3: the pinned source has no `Cargo.lock`, so its crates are
  resolved with `cargo generate-lockfile --publish-time` at the time of the
  3.8.3 release run and checked against its actual x86_64 compilation log;
  std from the pinned nightly-2026-09-11 `rust-src`; musl
  and compiler_rt from Zig 0.16.0, which also builds the preload libraries.
  [The upstream job](https://github.com/pkgforge-dev/Anylinux-sharun/actions/runs/41963279170/job/127034383534)
  records the exact checkout, all 22 selected runtime crate versions, compiler,
  archive checksum and binary checksum. Normalized evidence is committed in
  `packaging/appimage/sharun-provenance.json`; its archive and binary hashes
  match the pinned download. The log confirms only the eight-character
  `userland-execve` git revision prefix; the full source revision remains
  pinned in `inputs.lock.json`. Retrieve the raw log with
  `gh api repos/pkgforge-dev/Anylinux-sharun/actions/jobs/127034383534/logs --allow-escape-sequences`
  and compare its SHA-256 to the evidence record. The generator refuses to
  write an inventory whose sharun crate versions or input identity disagree;
  this includes both Rust toolchain components (`std` and compiler-builtins)
  and their checksum-pinned nightly sources.
  To recheck the committed inventory and cached inputs without network access:
  `packaging/appimage/closures.py --check-sharun-provenance target/appimage-inputs`.
- `dash` 0.5.13.5: musl and compiler_rt from Zig 0.16.0. dash names
  GPL-2.0-or-later (the bash-derived `mksignames.c` output) without its text;
  the GNU GPL-2.0 text is pinned from gnu.org and installed with its notice.

License texts come from the pinned sources themselves (crate packages,
release tarballs, `rust-src`, Zig's `lib/libc/musl/COPYRIGHT`). A package
without a license file of its own (memfd-exec) gets the SPDX license-list
text, pinned at v3.29.0. The standard library's backtrace crates (addr2line,
gimli, object, miniz_oxide, adler2, memchr) are listed although
`panic = "immediate-abort"` may drop them, since the pinned binaries cannot
show whether they are linked.

### Pinned sources and the corresponding-source set

The lock gained three arrays, verified like every other input:

- `srpms`: the Koji source RPM (URL and SHA-256) of every Fedora package that
  contributes a file, mounted read-only at `/srpms` during the build.
- `notices`: license texts and source archives of the non-Fedora components:
  uruntime 0.8.1, DwarFS 0.15.7, Anylinux-sharun 3.8.3 (`sharun`, preload
  libraries, and the patch/build script of its static `dash`), the dash
  0.5.13.5 release tarball (sha512 matches Anylinux-sharun's build script),
  quick-sharun's repository license and cross-libc-dlopen v0.2.10.
- `static`: the sources and texts of the statically linked components above
  (crates with the `Cargo.lock` checksum as SHA-256, git dependency archives,
  `rust-src`, Zig, the DwarFS libraries, the DwarFS source with its build
  scripts, GNU and SPDX texts), mounted read-only at `/static`.

After the artifact passes validation, the launcher assembles every pinned
source the inventory names, with a `SHA256SUMS`, as
`target/appimage/<name>.sources.part` and renames it to
`target/appimage/<name>.sources/` only after the artifact and its metadata
are in place: beside the AppImage, never inside it, for epic 6 to publish
with each release. A source the inventory names that is not pinned, or a
Fedora component without a pinned source RPM, refuses the build. Check with:

```sh
(cd target/appimage/Addict-0.1.1-x86_64.AppImage.sources && sha256sum --check --strict SHA256SUMS)
```

### Audit

`scripts/check-appimage.sh` trusts the inventory only after checking it
against the image, and fails, naming the item, when:

- a file or symlink in the image is not in the inventory, or the inventory
  names a file the image lacks;
- a component's license file is missing, empty or outside the image;
- a GPL, LGPL, MPL (or other copyleft) component has no pinned source record,
  any source record differs from its lock entry, or a Fedora component's
  source is not one of the pinned source RPMs;
- the runtime header lacks uruntime, dwarfs or any library
  `static-closures.json` lists for them (`missing notice: runtime header
  (<component>)`), or, for an artifact, the header is not uruntime;
- a tool in the inventory (sharun, dash) lacks a library of its static
  closure, or a static component's SPDX expression differs from the record.

It prints the component and file counts. The existing addict, crate,
Tesseract, Leptonica, giflib, tessdata and dash notices stay where they were;
the inventory references them. `packaging/appimage/tests.sh` has a refusal
fixture for each case, plus build-time attribution of an unowned file.

Known limits: the SPDX expressions of tool and DwarFS-library components are
taken from their upstream license files and repositories, and the texts
installed are the controlling ones. sharun's reconstructed crate versions
are confirmed against the actual release build (see above). The GCC start files linked into
cross-libc-dlopen (Debian's GCC 10) are not listed separately; they fall under
the GCC runtime library exception.
The 5.6 sweep retains the GStreamer, cdparanoia and bubblewrap payload and
its notices: removing dynamically loaded GTK/media helpers would require
additional runtime acceptance. Their complete source/notices handoff remains
part of this bundle.

### Executed verification, 2026-11-11

- Fresh `scripts/build-appimage.sh` from baseline revision
  `3c169fbc32912217088d9938b5eec6cebb3c99a6` with the 5.4 changes (source
  snapshot `affca6d07a5e77e33f94f2dff04b936c934678d4fcdd7d625a72fce6a10182bb`).
  AppImage SHA-256
  `36aa4dd6c2e0004272b54e5299bfa236c9cfda753d5536a44ab43d5a78755a2a`.
- The build reported `Notices: 233 components (70 statically linked), 2619
  files`; the build's and a separate run of `scripts/check-appimage.sh` on the
  artifact both reported
  `Notices: 233 components (62 in the runtime header) cover 2619 files`.
  License texts came from the binary RPM for 103 components, an installed
  sibling for 22 and the pinned source RPM for 30; 70 are statically linked
  libraries, 8 project, tool and runtime components.
- `target/appimage/Addict-0.1.1-x86_64.AppImage.sources/` holds 195 sources
  plus `SHA256SUMS` (124 source RPMs; 2.5 GB in all);
  `sha256sum --check --strict` passed. The lock pins 125 source RPMs:
  ca-certificates contributed only `etc/ssl/openssl.cnf`, which quick-sharun
  writes itself, so it is not a component.
- Rerunning `packaging/appimage/closures.py` left the lock and
  `static-closures.json` byte-identical.
- `packaging/appimage/tests.sh` passed, including the refusals: unlisted
  file, removed and empty notice, copyleft component without source, changed
  source pin, Fedora component pinned to a binary RPM, missing uruntime,
  dwarfs, libfuse3 and procfs-core runtime-header components, sharun without
  its static closure, stale inventory entry and build-time unattributable file
  (with a fixture owner map, no host `rpm`). Shell syntax, `git diff --check`
  and `reuse lint` passed.

Post-review final artifact, 2026-11-11: after the review fixes (tool binaries
must belong to their tool components, a project revision counts as a copyleft
source record), a fresh build from source snapshot
`f26d267afb371609b1ccc36908d819886fbf4538d0b229d89e8a3dfe0665907b` produced
AppImage SHA-256
`4ab5f3ea487658056e86c491fa506b0da2d650202a97fb20fa8951d7c28f0888`. The build
and a separate `scripts/check-appimage.sh` run on it reported the same 233
components (62 in the runtime header) covering 2619 files; its 195 sources
passed `sha256sum --check --strict`. `tests.sh` also passed the new
misattributed-`sharun` refusal.

## CI artifact handoff (5.5)

The independent `appimage` job in `.github/workflows/ci.yml` runs on an
Ubuntu 24.04 host. It preserves the other jobs and uses the same pinned Fedora
build, verified input lock, fresh Cargo/build directories and production binary
as the maintainer command. It removes unused runner SDKs to allow space for the
temporary build and the roughly 2.5 GiB corresponding-source set; it does not
cache `target`. Allow at least 20 GiB free for this job, including container
storage, inputs, temporary copies, outputs and the final uncompressed tar.

Run the same checks locally on an x86_64 Linux host with working rootless Podman:

```sh
scripts/build-appimage.sh
packaging/appimage/tests.sh
packaging/appimage/smoke-tests.sh
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml","rb"))["workspace"]["package"]["version"])')
scripts/check-appimage-smoke.sh "target/appimage/Addict-$version-x86_64.AppImage" target/appimage-smoke
scripts/archive-appimage.sh target/appimage-smoke target/appimage-handoff
```

Both new output directories must not already exist: use a new name for another
attempt. The smoke builds `packaging/appimage/smoke.Containerfile` (Ubuntu 24.04
at a fixed OCI digest; Ubuntu's verification-tool packages use its repositories).
Only preparing this verification image uses network access. Execution uses
private PID/IPC namespaces, `--network=none`, a read-only root and checkout/image
mounts, a disposable executable `/tmp`, and a writable diagnostics mount. No
host display socket, session/accessibility bus, engine/model directory or FUSE
device is mounted. The existing `ocr.py` creates each language's private
HOME/XDG paths and buses, uses Xvfb with X authentication and exercises the
native Capture button and actual GTK field. It traces the bundled OCR child,
models and libraries, checks exact EN/ES/CA output and waits for extraction
cleanup after each language. Container exit removes any processes or temporary
extraction even when the harness fails or times out.

The container grants only `SYS_PTRACE` for strace and process evidence, retaining
the default seccomp policy; an explicit ptrace probe fails before OCR if the
runner disallows tracing. `container-build.log`, `container.log` and `ocr/`
(including language logs, syscalls, UI results, bus configuration and cleanup)
remain in diagnostics on failure. `ocr/PASS.json` is created only after all
languages and cleanup pass. A failed command prevents handoff assembly/upload;
Actions retains any smoke diagnostics as `appimage-smoke-failure-<attempt>` for
seven days. The focused wrapper tests cover container build/run failure,
missing success evidence, reused output directories and failed handoff.
They also refuse missing/corrupt corresponding sources, including a source
removed together with its checksum line.

The version comes exclusively from `[workspace.package].version` in
`Cargo.toml`. For version `0.1.1`, the Actions artifact is named
`Addict-0.1.1-x86_64-handoff`, scoped to its workflow run/attempt and commit.
It contains `Addict-0.1.1-x86_64-handoff.tar` and its relative `.tar.sha256`.
The tar preserves executable mode, contains no enclosing directory, and holds:

- `Addict-0.1.1-x86_64.AppImage` and `.AppImage.sha256` (relative image name);
- `.AppImage.build.txt`, `.AppImage.inputs.lock.json`, `.AppImage.rpms.txt`,
  `.AppImage.layout.txt`, `.AppImage.notices.json`, `.AppImage.components.sha256`;
- `.AppImage.sources/`, the complete 5.4 corresponding-source set and its
  relative `SHA256SUMS` (currently 195 source files).

The assembler rechecks the image, package/license audit, sidecar notice inventory
against the embedded one, bundled component checksums, input lock and every
source against the pinned inventory. It rejects a missing output, a changed
checksum/source/notice, or smoke evidence for a different image before writing
the tar. Sources cannot be dropped by deleting their checksum line as well.
The upload action uses an immutable revision, fails on missing files, and
applies no additional compression to these already compressed payloads.

Download the handoff artifact from the successful run and extract its Actions
ZIP first. Verify and extract the contained tar in a fresh directory:

```sh
sha256sum --check --strict Addict-0.1.1-x86_64-handoff.tar.sha256
mkdir unpacked
tar -xf Addict-0.1.1-x86_64-handoff.tar -C unpacked
cd unpacked
test -x Addict-0.1.1-x86_64.AppImage
sha256sum --check --strict Addict-0.1.1-x86_64.AppImage.sha256
(cd Addict-0.1.1-x86_64.AppImage.sources && sha256sum --check --strict SHA256SUMS)
```

Epic 6 consumes these relative image, checksum, metadata and corresponding-source
paths together, using `.build.txt` to identify the source revision/snapshot and
the workflow run to identify CI provenance. Actions artifacts expire after seven
days; this job creates no GitHub Release, publishes no release assets and
uploads no dictionaries. Its deterministic Screenshot portal is a smoke test;
real desktop screenshots, portal UX, dictionary downloads, typed translation,
desktop/login and two-distribution human acceptance remain 5.7 work. Local
runner-equivalent results are not evidence that hosted Actions has executed.

### Refactor sweep verification (5.6), 2026-11-11

The exact upstream sharun job confirmed all 22 recorded runtime crate
versions and the pinned archive/binary hashes. Regeneration retained every
static component definition, source pin and byte of `inputs.lock.json`;
only the host's determination text changed. Provenance refusal fixtures,
existing package/wrapper refusals, Python/Bash syntax, REUSE and whitespace
checks passed.

A fresh pinned container/release build from the local source snapshot
`b472f53eac0e93dcb04311ad5962756bbe28ab05b7ab5a5962b109b1504ae578`
produced AppImage SHA-256
`23a550ffdb5f930019b95ff58159ca1a6fe834c0705fcbbead51c563dd971a9a`.
The build metadata records the working-tree changes over its baseline;
this was a local source snapshot build. Package audits passed with 233
components, 2619 files and 195 corresponding sources. Bundle membership,
sharun, the Tesseract executable and all three models match final 5.5;
byte-identical rebuilds are still not promised.

Ubuntu 24.04.5 container smoke passed exact English/Spanish/Catalan output,
bundled child/model/library paths and extraction cleanup with no host OCR,
models or FUSE. Evidence: `target/appimage-smoke-5.6/ocr/`. The checked
handoff is in `target/appimage-handoff-5.6/`; extraction preserved executable
mode, and tar, image and all source checksums passed. Raw upstream evidence
is retained at `target/appimage-provenance-5.6/upstream-job.log`.
This is local verification; hosted Actions and 5.7 desktop/login acceptance
remain unobserved by this sweep.

After Quick review, the provenance check also requires the recorded std and
compiler-builtins components and their exact pinned nightly sources.
Changed/missing toolchain components and a wrong source now have executed
refusal coverage. Regeneration and all checks passed again. A second full
build produced final AppImage SHA-256
`0092a70feb306953a28188d2113b06740de9e930971ba5991dab2c42541c7557`,
source snapshot
`ab2f0c4638d3596ef06f6a56e38ba4cbd476e6ecd95a8dc6a8f871e8a25af270`.
Final package audit, three-language production OCR, bundled paths and
cleanup passed. `target/appimage-handoff-5.6-final/` passed archive transfer,
executable mode and every image/source checksum; smoke and transfer
evidence are under `target/appimage-smoke-5.6-final/` and
`target/appimage-unpacked-5.6-final/`.
