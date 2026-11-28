# Rustygrade for Android

<div align="center">

![Platform](https://img.shields.io/badge/PLATFORM-ANDROID%209%2B%20%7C%20COMPOSE%20%2B%20RUST-4FD6BE?style=for-the-badge&logo=android&logoColor=white)
![Audio](https://img.shields.io/badge/AUDIO-MEDIA3%20%7C%20EXOPLAYER-E8C060?style=for-the-badge)
![Version](https://img.shields.io/badge/VERSION-1.0.0-FF6B35?style=for-the-badge)
![License](https://img.shields.io/badge/LICENSE-MIT-white?style=for-the-badge)

<p align="center">
  <b>Native Android Hi-Res local music player powered by Jetpack Compose, Media3 ExoPlayer, and a Rust core via UniFFI.</b>
</p>

</div>

---

## Key Features

- **Orbit Terminal UI/UX (`Kotlin + Jetpack Compose`):**
  - High-contrast OLED dark palette (`#08080A`) with WCAG-checked text tiers (`Text`, `TextDim`, `TextMute`), clean sans-serif metadata typography, and crisp monospace HUD controls.
  - Dynamic cover-extracted accent colors & ambient glow (`AnimatedVisibility` liquid-reveal player, responsive portrait & landscape Now Playing layouts, corner-bracketed artwork frames, custom geometric `OrbitSlider` & segmented `SeekBar`).
- **Audiophile Audio Engine (`AndroidX Media3` / `ExoPlayer`):**
  - Gapless playback for sequential album tracks plus configurable crossfade (`0–12 s`).
  - `ReplayGainAudioProcessor` (`Off`, `Track`, `Album`) with built-in EBU R128 / ITU-R BS.1770-4 loudness scanner and Android 9+ `DynamicsProcessing` 10-band equalizer with presets.
  - Userspace UAC1/UAC2 Direct USB DAC driver (`rust/rustygrade-usb` over `usbdevfs` isochronous transfers, Android 9+): bypasses Android AudioFlinger and the OS resampler completely to preserve native sample rate and bit depth; applies ReplayGain through the DAC's hardware Feature Unit when available while keeping PCM bit-perfect.
- **Online Metadata & Artwork Fetchers:**
  - Multi-provider lyrics lookup (**LRCLIB**, **NetEase**, **KuGou**, **Lyrics.ovh**) with per-track picker, persistent cache, and batch missing-lyrics fetch.
  - Multi-provider cover art lookup (**iTunes**, **Deezer**, **MusicBrainz / Cover Art Archive**) in the tag editor, album menu, and batch missing-covers fetch.
- **High-Speed Rust Library Core (`rust/rustygrade-core` via `UniFFI`):**
  - MediaStore and writable Storage Access Framework (`SAF`) folder scanning with thumbnail/full cover caching (`arm64-v8a` & `x86_64`).
  - Album grid & artist views with sortable collections, multi-disc album tracklists, instant search across albums/artists/tracks, play count/history tracking, and multi-select album removal.
- **Collections & Smart Playlists:**
  - Favorites, custom playlists with custom cover picking, drag-and-drop queue reordering (`QueuePanel`), and rule-based Smart Playlists with live preview matching.
- **Synced Lyrics & In-App Tag Editor:**
  - Embedded and sidecar `.lrc` lyrics viewer (`LyricsPanel`) with smooth edge fading, tap-to-seek lines, and automatic playback follow.
  - Track-level and batch album tag/cover editing (`lofty`) across MediaStore (including API 30+ write request recovery across rotation) and SAF files.
- **Android Integration & Battery Care:**
  - `MediaLibraryService` & `MediaSession` supporting lockscreen controls, media notifications, Bluetooth headsets, and Android Auto.
  - Strict battery-aware animation gating: marquee scrolling, moving shader glow, and lyric polling halt completely when paused, backgrounded, obscured, or in Battery Saver mode.
- **Backup & Sync (`.zip`):**
  - Portable ZIP export/import supporting `MERGE` (add new items & keep higher play counts) and `REPLACE` modes for playlists, favorites, smart playlists, custom covers, play stats, and settings with automatic library path remapping.

---

> **Naming & Migration.** The app is called **Rustygrade** (like the Linux build), and all internal identifiers
> now use `rustygrade` too: the `applicationId` `dev.okarin.rustygrade` (debug builds install beside the release as
> `dev.okarin.rustygrade.dev`, "Rustygrade Dev"), the Kotlin package, the `rustygrade-*` Rust crates and the
> `RUSTYGRADE_KEYSTORE_*` signing variables.
>
> **Upgrading from an older build:** because the `applicationId` changed from `dev.okarin.retrograde` to
> `dev.okarin.rustygrade`, Android treats Rustygrade as a new app and will **not** upgrade over an old install in
> place. To keep your data, export a **Backup ZIP** from the old install (Settings → `BACKUP_&_SYNC`), install the new
> app, and import that ZIP there: it transfers playlists, favorites, smart playlists, custom covers, play stats
> and settings. Uninstall the old app afterwards.

## Project Structure

```text
rustygrade-mobile/
├── android/                  # Jetpack Compose UI, Media3 playback service & SAF storage bridge
├── rust/
│   ├── rustygrade-core/      # Library scanner, metadata/cover cache, queue, smart rules, lyrics, tags, backup
│   ├── rustygrade-ffi/       # UniFFI bindings layer exposing core & player APIs to Kotlin
│   ├── rustygrade-usb/       # Userspace UAC1/UAC2 USB audio driver (usbdevfs isochronous transfers)
│   └── uniffi-bindgen/       # Local UniFFI Kotlin binding generator
├── docs/LINUX_PARITY.md     # Feature parity audit vs the Linux build
├── scripts/
│   ├── build-rust.sh         # Cross-compile .so libraries (arm64-v8a, x86_64) & generate UniFFI bindings
│   ├── verify-all.sh         # Full Rust + Android + R8 split APK/AAB + signing verification pipeline
│   └── device-test.sh        # ADB device helper (wake, install, launch, UI dump, tap, screenshot tour)
├── .github/workflows/
│   ├── android.yml           # CI: clippy + Rust tests, debug/release (R8, ABI split) builds, lint
│   └── release.yml           # Tag-driven release: signed split APKs + AAB, checksums, R8 mapping archive
└── LICENSE.md                # MIT License
```

---

## Build, Verification & Device Testing

### Prerequisites
- Rust toolchain (`1.89+` MSRV; CI pins `1.99.1`) with Android targets (`aarch64-linux-android`, `x86_64-linux-android`) and `cargo-ndk`
- Android SDK (`compileSdk 37.2`, `minSdk 28`, `targetSdk 35`) and Android NDK (`27.3.13750724`)
- JDK 17

### Common Commands

```bash
# Build native Rust .so libraries and regenerate UniFFI Kotlin bindings
./scripts/build-rust.sh

# Run full verification (Rust clippy/tests, JNI symbol check, lint, debug/release splits & AAB)
./scripts/verify-all.sh

# Build & install Debug APK onto connected device (preserves app data)
./scripts/device-test.sh install

# Capture a read-only visual tour across all app screens via ADB
./scripts/device-test.sh tour
```

### Release Builds

```bash
# ABI-split APKs (arm64-v8a, x86_64, universal). versionCode = base * 10 + {universal 0, x86_64 1, arm64-v8a 2}
./gradlew assembleRelease -PsplitApks=true -PversionName=1.0.0 -PversionCode=1

# Play Store bundle (versionCode stays exactly the base value)
./gradlew bundleRelease -PversionName=1.0.0 -PversionCode=1
```

Release signing reads `RUSTYGRADE_KEYSTORE_PATH`, `RUSTYGRADE_KEYSTORE_PASSWORD`, `RUSTYGRADE_KEY_ALIAS` and `RUSTYGRADE_KEY_PASSWORD` from the environment (all four, or the build stays unsigned). Pushing a `v*` tag runs `release.yml` (release title `Rustygrade X.Y.Z`, files `rustygrade-<abi>-release.apk`), which requires the four matching `ANDROID_KEYSTORE_*` secrets, verifies every signature, uploads the R8 `mapping.txt` as a private workflow artifact, and publishes the GitHub Release (hyphenated versions such as `1.1.0-rc.1` are marked pre-release).
