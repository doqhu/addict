# Building the x86_64 AppImage

This recipe packages the production `addict` executable with GTK 4.20 and
libc. It leaves dictionary indexes in the normal shared application data
path, `$XDG_DATA_HOME/addict` (default `~/.local/share/addict`). OCR, startup
integration, updates, publication and complete dependency notices are later
work. Do not distribute this experimental artifact as a finished release.
The Flatpak recipe and application behavior are unchanged.

## Inputs and prerequisites

Use an x86_64 Linux host with rootless Podman, Bash, Git, Python 3.11 or
later, GNU tar/coreutils and HTTPS access to Fedora, GitHub and crates.io.
Allow several GiB of disk space and memory for the release build. Rootless
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
