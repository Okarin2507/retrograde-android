# Rustygrade Android ↔ Rustygrade Linux: parity audit

Audit date: 2026-11-28. Linux reference: `rustygrade` 1.0.2 (iced, `crates/rustygrade-*`).
Android: this repo, 1.0.0 (Compose + Media3 + `rust/rustygrade-core` via UniFFI).

Both ports share the Lonetrail / Orbit Terminal look (void palette, mono tracked HUD chrome,
orange → gold → teal spectrum, hex play button, cover accent). Keep it when changing either.

## 1. Done in this pass

| Area | Change |
|---|---|
| Direct USB | Userspace UAC1/UAC2 driver (`rust/rustygrade-usb`, see §2) replaces the AudioFlinger mixer-attribute path: the DAC is driven directly at the track's native rate and bit depth. |
| Theme tokens | `ui/theme/Tokens.kt` (`Space`, `Dimens`, `Alpha`), `Type.kt` (`OrbitType` roles), `Motion.kt` (durations/easings named after the desktop `motion` module), `Orbit.TextSoft` / `BorderStrong` from the desktop palette. |
| Motion | `ui/motion/`: `pressable` (desktop `HEX_PRESS`/`HEX_HOVER`), `enterOnce` cascade (`EnterSpec` 300 ms cubic-out, 8 dp rise; once per screen, like `ABSENCE`), `PulseDot` (`pulse_dot`), directional push/pop/tab transitions, sliding nav indicator. All gated by `LocalMotionEnabled` (off in Battery Saver or with system animations off). |
| Components | `Common.kt` (704 lines) split into `Locals`, `Format`, `Decor`, `OrbitSlider`, `Controls`, `Cover`, `ScreenHeader`, `Dialogs`, `TextFields`, `TrackRow`. New: `SpectrumBrush` gradient, `KindBadge` (`ALB`/`PL`), `Kicker` (`— SYSTEM_NAV`), `DetailHero`. |
| Screens | `LIBRARY_DATA` / `DATA_BANK` / `SYSTEM_CONFIG` headings with teal status lines, corner-ticked album cards with `ALB` badges and a pulsing now-playing mark, detail hero (blurred cover backdrop), HUD-panel settings, idle/no-match search panels, spectrum transport line on the mini bar. |
| Rust core | `backup`, `scanner`, `playback`, `covers` split into module directories like Linux 1.0.2 (public paths unchanged). |
| Naming | All internal identifiers unified on `rustygrade` (`dev.okarin.rustygrade`, `rustygrade-*` crates, `RUSTYGRADE_KEYSTORE_*`); see §3. |
| Dev loop | Debug builds use `applicationIdSuffix ".dev"` (`dev.okarin.rustygrade.dev`) so they install beside a release build; `scripts/device-test.sh` follows. |

## 2. Linux features ported & remaining backlog

Completed in the latest pass:
1. **Userspace Direct USB DAC driver (`rust/rustygrade-usb`) + Hardware ReplayGain:** UAC1/UAC2
   isochronous `usbdevfs` driver bypassing AudioFlinger completely on API 28+, preserving exact
   sample rate & bit depth, with physical volume buttons via `MediaSession` `PLAYBACK_TYPE_REMOTE`
   and bit-perfect ReplayGain applied through the DAC's hardware Feature Unit (`effective_db =
   user_db + replay_gain_db`).
2. **Online lyrics lookup (`storage/OnlineLyricsLookup.kt`, `ui/player/LyricsPanel.kt`):**
   Multi-provider lookup (LRCLIB, NetEase, KuGou, Lyrics.ovh), interactive Lyric Picker with
   persistent local cache, and batch missing-lyrics fetch in Settings.
3. **Online artwork lookup (`storage/OnlineArtworkLookup.kt`, `ui/components/OnlinePickers.kt`):**
   Multi-provider lookup (iTunes incl. JP store for CJK, Deezer, MusicBrainz / Cover Art Archive),
   Cover Picker in Tag Editor and Album Detail, and batch missing-covers fetch in Settings.
4. **In-process EBU R128 ReplayGain scanner (`rust/rustygrade-core/src/r128.rs`,
   `storage/ReplayGainScanner.kt`):** Decodes local & SAF tracks via `MediaExtractor`+`MediaCodec`
   into pure-Rust ITU-R BS.1770-4 loudness analysis (`-18.0 LUFS` target), saving per-track and
   pooled album gain/peak directly into `library.json`.

Remaining backlog:
1. **Folder watcher** (`core/watcher.rs`). Android relies on MediaStore / manual rescans; a
   `ContentObserver` on MediaStore would be the native equivalent.
2. **Keyboard shortcuts / mini player / tray / MPRIS / Discord RPC** — desktop-only by nature;
   Android covers the same ground with MediaSession, the notification and Android Auto.

Android-only (keep): Android Auto / `MediaLibraryService`, SAF writable folders, the
DynamicsProcessing 10-band EQ, battery-aware animation gating, play-stats UI.
The `PRIVATE_MEMORY_BANK` Easter egg (and its visualizer strip under the full-screen player and in
Settings → About) was removed, matching the Linux build where `egg.rs` is gone.

Checked and **not** gaps (an automated first pass claimed otherwise):
- Atomic JSON writes: `rustygrade-core/src/fsutil.rs` (`write_atomic` + one-generation `.bak`).
- ZIP import safety: `storage/BackupArchive.kt` enforces entry-count, per-file, total and
  decompression-ratio limits; `backup` rejects path-traversal cover names (tested).

## 3. Naming

Both ports are now called **Rustygrade** (app label, About line, release titles
`Rustygrade X.Y.Z`, APK names), and internal identifiers match: `applicationId`
`dev.okarin.rustygrade` (debug `dev.okarin.rustygrade.dev`), Kotlin package `dev.okarin.rustygrade`,
crates `rustygrade-*`, `RUSTYGRADE_KEYSTORE_*` signing variables.

Migration: older builds shipped as `dev.okarin.retrograde`. Android treats the new `applicationId`
as a separate app, so it cannot upgrade those installs in place. Users export a Backup ZIP from the
old install and import it into the new one (playlists, favorites, smart playlists, custom covers,
play stats, settings).
