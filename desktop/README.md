# Veirune desktop

This is the active React 19 and Tauri 2 application. Playback, SoundCloud API
access, cache, and settings run through the Rust backend in `src-tauri/` and
the shared modules in `../src/`.

```sh
npm ci
npm run tauri dev
```

`npm run build` checks TypeScript and bundles the interface. Browser preview
(`npm run dev`) uses simulated data; test real playback in Tauri.

The tagged Windows release downloads and bundles the pinned CLAP model and
worker in GitHub Actions. For a local signed build, prepare the resources with
`../tools/prepare_private_clap.ps1`, set `TAURI_SIGNING_PRIVATE_KEY` to the
private updater key path, then run `npm run tauri:private -- --bundles nsis`.
The regular `npm run tauri build` excludes CLAP.

## Linux (experimental)

The Linux workflow builds x86-64 `.deb` and AppImage artifacts. The `.deb`
targets Ubuntu 24.04 and compatible systems; AppImage is intended for other
distributions, including Arch and CachyOS, provided FUSE and the required
desktop services are available.

On a `v*` tag the release workflow calls the Linux workflow, so the `.deb` and
`.AppImage` are attached to the same GitHub release as the Windows and macOS
installers. Publication waits for that job; `tools/validate_release_assets.py`
rejects a release without both Linux packages, and `tools/stage_linux_release.py`
names them `Veirune_<version>_linux-x86_64.deb` / `.AppImage`. These packages
are unsigned downloads without automatic updates; install the next release
manually. Ordinary pushes build the same packages only as CI artifacts.
 Build locally on Ubuntu 24.04 with the
dependencies listed in `.github/workflows/linux.yml`, then run:

```sh
cd desktop
npm ci
npm run tauri -- build --config src-tauri/tauri.linux.conf.json --config src-tauri/tauri.private.conf.json --bundles deb,appimage --ci -- --locked
```

The CLAP worker and model that power "My Wave" are prepared and smoke-tested
before packaging, the same way as on Windows and macOS:

```sh
python tools/prepare_linux_clap.py
```

It needs `uv` and Python 3.12 and reuses the pinned model files if they are
already present in `desktop/src-tauri/resources/clap/model`. The worker is a
frozen PyInstaller build for the current CPU architecture, so a Linux worker
cannot be copied from a Windows or macOS checkout.

Sign-in requires an unlocked Secret Service keyring (such as GNOME Keyring or
KWallet) and a user D-Bus session. Audio requires an ALSA-compatible device;
PipeWire or PulseAudio normally provides one. System tray and global shortcuts
depend on the desktop environment, especially under Wayland. The bundles now
include the CLAP worker and weights, so they are much larger than the
lightweight updates. Protected playback depends on the system WebKitGTK and its
DRM capabilities and is not yet verified on Linux. Test sign-in, playback,
"My Wave", media controls, device selection, tray, Wayland/X11, and
installation on the intended distribution before treating Linux as supported.
