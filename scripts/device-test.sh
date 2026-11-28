#!/usr/bin/env bash
# Helper script for driving Rustygrade on a connected Android device via adb:
# wake/unlock, build & install debug APK, launch app, dump UI hierarchy, tap nodes/coords,
# capture screenshots, inspect logcat, and run a strictly read-only visual screenshot tour.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Debug builds carry the ".dev" applicationIdSuffix (they install beside the release app);
# RUSTY_PKG=dev.okarin.rustygrade drives an installed release build instead.
PKG="${RUSTY_PKG:-dev.okarin.rustygrade.dev}"
ACTIVITY="$PKG/dev.okarin.rustygrade.MainActivity"
APK_PATH="$ROOT/android/app/build/outputs/apk/debug/rustygrade-debug.apk"
# World-writable-by-shell scratch dir; avoids littering the user-visible /sdcard.
REMOTE_UI_XML="/data/local/tmp/rustygrade_ui.xml"

resolve_adb() {
  for candidate in \
    "${ADB_BIN:-}" \
    "${ANDROID_HOME:-}/platform-tools/adb" \
    "${ANDROID_SDK_ROOT:-}/platform-tools/adb" \
    "$HOME/Android/Sdk/platform-tools/adb"; do
    if [[ -n "$candidate" && -x "$candidate" ]]; then
      echo "$candidate"
      return 0
    fi
  done
  if command -v adb >/dev/null 2>&1; then
    command -v adb
    return 0
  fi
  echo "error: adb not found (set ANDROID_HOME or ADB_BIN)" >&2
  exit 1
}

ADB_BIN="$(resolve_adb)"
SERIAL="${ADB_SERIAL:-}"

adb_cmd() {
  if [[ -n "$SERIAL" ]]; then
    "$ADB_BIN" -s "$SERIAL" "$@"
  else
    "$ADB_BIN" "$@"
  fi
}

select_serial() {
  if [[ -n "$SERIAL" ]]; then
    return 0
  fi
  local connected
  connected="$("$ADB_BIN" devices | awk 'NR>1 && $2=="device" {print $1}')"
  local count
  count="$(wc -w <<<"$connected")"
  if [[ "$count" -eq 0 ]]; then
    echo "error: no connected adb device in 'device' state" >&2
    "$ADB_BIN" devices -l >&2
    exit 1
  elif [[ "$count" -eq 1 ]]; then
    SERIAL="$(xargs <<<"$connected")"
  else
    echo "error: multiple devices connected ($connected); pass --serial <id> or set ADB_SERIAL" >&2
    exit 1
  fi
}

screen_size() {
  local wm_out raw w h
  wm_out="$(adb_cmd shell wm size | tr -d '\r')"
  raw="$(awk '/Override size:/ {print $3; found=1} END {if (!found) print ""}' <<<"$wm_out")"
  if [[ -z "$raw" ]]; then
    raw="$(awk '/Physical size:/ {print $3; found=1} END {if (!found) print ""}' <<<"$wm_out")"
  fi
  if [[ -z "$raw" ]]; then
    raw="$(tail -n1 <<<"$wm_out" | awk '{print $NF}')"
  fi
  w="${raw%%x*}"
  h="${raw#*x}"
  if [[ ! "$w" =~ ^[0-9]+$ || ! "$h" =~ ^[0-9]+$ ]]; then
    echo "error: failed to parse screen size from 'wm size' ($wm_out)" >&2
    exit 1
  fi
  echo "$w $h"
}

cmd_wake() {
  select_serial
  adb_cmd shell input keyevent KEYCODE_WAKEUP
  adb_cmd shell wm dismiss-keyguard >/dev/null 2>&1 || true
  local wakefulness size
  wakefulness="$(adb_cmd shell dumpsys power | tr -d '\r' | grep -m1 'mWakefulness=' | xargs || true)"
  size="$(adb_cmd shell wm size | tr -d '\r' | tail -n1 | xargs || true)"
  echo "Device $SERIAL ready ($wakefulness, $size)"
}

cmd_install() {
  local skip_build=0
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --skip-build) skip_build=1; shift ;;
      *) echo "error: unknown flag for install: $1" >&2; exit 1 ;;
    esac
  done

  select_serial
  if [[ "$skip_build" -eq 0 ]]; then
    echo "==> Building debug APK"
    (cd "$ROOT/android" && ./gradlew :app:assembleDebug)
  fi
  [[ -f "$APK_PATH" ]] || { echo "error: debug APK not found at $APK_PATH" >&2; exit 1; }
  echo "==> Installing $APK_PATH onto $SERIAL (preserving app data)"
  adb_cmd install -r "$APK_PATH"
}

cmd_launch() {
  local restart=0
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --restart) restart=1; shift ;;
      *) echo "error: unknown flag for launch: $1" >&2; exit 1 ;;
    esac
  done

  select_serial
  cmd_wake >/dev/null
  if [[ "$restart" -eq 1 ]]; then
    adb_cmd shell am force-stop "$PKG"
    sleep 0.4
  fi
  adb_cmd shell am start -n "$ACTIVITY" >/dev/null
  sleep 1.2
  ensure_app
  echo "Launched $ACTIVITY on $SERIAL"
}

current_focus() {
  adb_cmd shell dumpsys window | tr -d '\r' | grep -E 'mCurrentFocus|mFocusedApp' || true
}

ensure_app() {
  local focus
  focus="$(current_focus)"
  if ! grep -q "$PKG" <<<"$focus"; then
    adb_cmd shell am start -n "$ACTIVITY" >/dev/null
    sleep 1.2
  fi
}

dump_ui_to_stdout() {
  adb_cmd shell uiautomator dump "$REMOTE_UI_XML" >/dev/null 2>&1 || true
  adb_cmd shell cat "$REMOTE_UI_XML" 2>/dev/null || true
}

cleanup_remote_xml() {
  adb_cmd shell rm -f "$REMOTE_UI_XML" >/dev/null 2>&1 || true
}

find_node_coords() {
  local needle="$1"
  local min_y="${2:-0}"
  local index="${3:-0}"
  dump_ui_to_stdout | python3 -c '
import sys, re, xml.etree.ElementTree as ET
needle = sys.argv[1]
min_y = int(sys.argv[2])
target_idx = int(sys.argv[3])
raw = sys.stdin.read().strip()
if not raw.startswith("<"):
    sys.exit(1)
try:
    root = ET.fromstring(raw)
except ET.ParseError:
    sys.exit(1)
hits = []
needle_cf = needle.casefold()
for n in root.iter("node"):
    t = n.attrib.get("text", "")
    d = n.attrib.get("content-desc", "")
    if t.casefold() == needle_cf or d.casefold() == needle_cf:
        nums = re.findall(r"\d+", n.attrib.get("bounds", ""))
        if len(nums) == 4:
            x1, y1, x2, y2 = map(int, nums)
            cy = (y1 + y2) // 2
            if cy >= min_y:
                hits.append(((x1 + x2) // 2, cy))
if len(hits) > target_idx:
    print(f"{hits[target_idx][0]} {hits[target_idx][1]}")
' "$needle" "$min_y" "$index"
}

# Captures a screenshot atomically: write to "<dest>.tmp", require a non-empty PNG, then rename.
# A dropped adb connection therefore never leaves a 0-byte (or truncated) screenshot behind.
capture_screenshot() {
  local dest="$1" tmp="$1.tmp"
  if ! adb_cmd exec-out screencap -p > "$tmp" || [[ ! -s "$tmp" ]]; then
    rm -f "$tmp"
    echo "error: screencap failed (adb dropped or empty capture) for $dest" >&2
    return 1
  fi
  mv "$tmp" "$dest"
}

# Prints "x y" of the first clickable card/row below the node labelled $1 (e.g. the ALBUMS / ARTISTS tab)
# and above the bottom nav bar. Real nodes come from `uiautomator dump`, never from guessed percentages.
# Exits 1 (no output) when nothing qualifies, e.g. on an empty library.
find_first_card_coords() {
  local anchor="$1" width="$2" height="$3"
  dump_ui_to_stdout | python3 -I -c '
import sys, re, xml.etree.ElementTree as ET
anchor, W, H = sys.argv[1].casefold(), int(sys.argv[2]), int(sys.argv[3])
raw = sys.stdin.read().strip()
if not raw.startswith("<"):
    sys.exit(1)
try:
    root = ET.fromstring(raw)
except ET.ParseError:
    sys.exit(1)

def bounds(n):
    nums = re.findall(r"\d+", n.attrib.get("bounds", ""))
    return tuple(map(int, nums)) if len(nums) == 4 else None

anchor_bottom = None
for n in root.iter("node"):
    if anchor in (n.attrib.get("text", "").casefold(), n.attrib.get("content-desc", "").casefold()):
        b = bounds(n)
        if b:
            anchor_bottom = b[3]
            break
if anchor_bottom is None:
    sys.exit(1)

nav_top = H * 82 // 100
cards = []
for n in root.iter("node"):
    if n.attrib.get("clickable") != "true":
        continue
    b = bounds(n)
    if not b:
        continue
    x1, y1, x2, y2 = b
    cy = (y1 + y2) // 2
    # Big enough to be a card / list row (not a chip or icon button), below the tab, above the nav bar.
    if y1 >= anchor_bottom and cy < nav_top and (x2 - x1) >= W * 25 // 100 and (y2 - y1) >= H * 5 // 100:
        cards.append((y1, x1, (x1 + x2) // 2, cy))
if not cards:
    sys.exit(1)
cards.sort()
print(cards[0][2], cards[0][3])
' "$anchor" "$width" "$height"
}

cmd_dump() {
  select_serial
  trap cleanup_remote_xml EXIT RETURN
  dump_ui_to_stdout | python3 -c '
import sys, re, xml.etree.ElementTree as ET
raw = sys.stdin.read().strip()
if not raw.startswith("<"):
    print("error: empty/invalid uiautomator dump", file=sys.stderr)
    sys.exit(1)
root = ET.fromstring(raw)
for n in root.iter("node"):
    t = n.attrib.get("text", "")
    d = n.attrib.get("content-desc", "")
    c = n.attrib.get("clickable", "false")
    if t or d or c == "true":
        nums = re.findall(r"\d+", n.attrib.get("bounds", ""))
        pos = ""
        if len(nums) == 4:
            x1, y1, x2, y2 = map(int, nums)
            pos = f"[{(x1+x2)//2},{(y1+y2)//2}] ({x1},{y1}-{x2},{y2})"
        print(f"{pos:34s} clickable={c:5s} text={t!r} desc={d!r}")
'
}

cmd_tap() {
  select_serial
  trap cleanup_remote_xml EXIT RETURN
  if [[ $# -eq 2 && "$1" =~ ^[0-9]+$ && "$2" =~ ^[0-9]+$ ]]; then
    adb_cmd shell input tap "$1" "$2"
    echo "Tapped ($1, $2)"
    return 0
  fi
  if [[ $# -lt 1 ]]; then
    echo "error: usage: tap <text-or-desc | x y>" >&2
    exit 1
  fi
  local coords tx ty
  coords="$(find_node_coords "$1" 0 0 || true)"
  if [[ -z "$coords" ]]; then
    echo "error: UI node not found for '$1'" >&2
    exit 1
  fi
  read -r tx ty <<<"$coords"
  adb_cmd shell input tap "$tx" "$ty"
  echo "Tapped '$1' at ($tx $ty)"
}

cmd_shot() {
  select_serial
  local name="screen_$(date +%Y%m%d_%H%M%S)"
  local out_dir=""
  if [[ $# -gt 0 && "${1:-}" != --* ]]; then
    name="$1"
    shift
  fi
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --out)
        [[ $# -ge 2 ]] || { echo "error: --out requires a value" >&2; exit 1; }
        out_dir="$2"; shift 2 ;;
      *) echo "error: unknown flag for shot: $1" >&2; exit 1 ;;
    esac
  done
  if [[ -z "$out_dir" ]]; then
    out_dir="$ROOT/screenshots/$(date +%Y%m%d_%H%M%S)"
  fi
  mkdir -p "$out_dir"
  [[ "$name" == *.png ]] || name="${name}.png"
  local dest="$out_dir/$name"
  capture_screenshot "$dest"
  echo "$dest"
}

cmd_logcat() {
  select_serial
  # Run adb on its own first so a transport error fails the command instead of being swallowed by `| grep || true`.
  local log
  log="$(adb_cmd logcat -d -t 150)"
  grep -iE 'rustygrade|RustyUsb|RustyR128|RustyEq|AndroidRuntime|FATAL|Compose' <<<"$log" || true
}

cmd_tour() {
  local out_dir="$ROOT/screenshots/$(date +%Y%m%d_%H%M%S)"
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --out)
        [[ $# -ge 2 ]] || { echo "error: --out requires a value" >&2; exit 1; }
        out_dir="$2"; shift 2 ;;
      *) echo "error: unknown flag for tour: $1" >&2; exit 1 ;;
    esac
  done

  select_serial
  mkdir -p "$out_dir"
  trap cleanup_remote_xml EXIT RETURN

  read -r W H < <(screen_size)

  snap_step() {
    local file="$1"
    ensure_app
    capture_screenshot "$out_dir/$file"
    echo "Captured $file"
  }

  tap_match_or_warn() {
    local label="$1"
    local min_y="${2:-0}"
    local idx="${3:-0}"
    ensure_app
    local coords tx ty
    coords="$(find_node_coords "$label" "$min_y" "$idx" || true)"
    if [[ -n "$coords" ]]; then
      read -r tx ty <<<"$coords"
      adb_cmd shell input tap "$tx" "$ty"
      sleep 1.0
      return 0
    fi
    echo "warn: '$label' not found" >&2
    return 1
  }

  # Taps a bottom-nav tab located from the live UI dump. No blind coordinate fallback: if the tab is
  # not on screen (dialog open, app crashed) we say so and let the caller skip the shot.
  tap_nav() {
    local label="$1"
    ensure_app
    local coords tx ty
    coords="$(find_node_coords "$label" $(( H * 82 / 100 )) 0 || true)"
    if [[ -z "$coords" ]]; then
      echo "warn: nav tab '$label' not found in UI dump" >&2
      return 1
    fi
    read -r tx ty <<<"$coords"
    adb_cmd shell input tap "$tx" "$ty"
    sleep 0.9
  }

  # Opens the first album card / artist row under the given tab, resolved from the UI dump.
  tap_first_card() {
    local anchor="$1" coords tx ty
    coords="$(find_first_card_coords "$anchor" "$W" "$H" || true)"
    if [[ -z "$coords" ]]; then
      echo "note: no clickable card under '$anchor' (empty library?); skipping its detail shots" >&2
      return 1
    fi
    read -r tx ty <<<"$coords"
    adb_cmd shell input tap "$tx" "$ty"
    sleep 1.2
  }

  echo "==> Starting read-only visual tour on $SERIAL (${W}x${H}) -> $out_dir"
  cmd_launch --restart >/dev/null
  sleep 0.8

  # 1. Library -> Albums
  if tap_nav "LIBRARY"; then
    tap_match_or_warn "ALBUMS" 0 0 || true
    snap_step "01_library_albums.png"

    # 2. Library -> Artists & Artist Detail
    if tap_match_or_warn "ARTISTS" 0 0; then
      snap_step "02_library_artists.png"
      if tap_first_card "ARTISTS"; then
        snap_step "03_artist_detail.png"
        adb_cmd shell input keyevent KEYCODE_BACK
        sleep 0.8
      fi
    fi

    # 3. Album Detail -> Track Action Sheet -> Tag Editor (read-only peek)
    tap_match_or_warn "ALBUMS" 0 0 || true
    if tap_first_card "ALBUMS"; then
      snap_step "04_album_detail.png"

      if tap_match_or_warn "More" $(( H * 55 / 100 )) 0; then
        snap_step "05_track_action_sheet.png"
        if tap_match_or_warn "EDIT TAGS" $(( H * 40 / 100 )) 0; then
          snap_step "06_tag_editor.png"
          adb_cmd shell input keyevent KEYCODE_BACK
          sleep 0.7
        else
          adb_cmd shell input keyevent KEYCODE_BACK
          sleep 0.6
        fi
      fi
      adb_cmd shell input keyevent KEYCODE_BACK
      sleep 0.7
    fi
  else
    echo "warn: skipped Library shots (bottom nav not reachable)" >&2
  fi

  # 4. Playlists -> Favorites Detail -> Smart Playlist Editor (read-only peek)
  if tap_nav "PLAYLISTS"; then
    snap_step "07_playlists.png"
  else
    echo "warn: skipped Playlists shots" >&2
  fi
  if tap_match_or_warn "Favorites" 0 0; then
    snap_step "08_playlist_detail.png"
    adb_cmd shell input keyevent KEYCODE_BACK
    sleep 0.7
  fi
  local smart_hdr smart_y smart_min_y
  smart_hdr="$(find_node_coords "SMART PLAYLISTS" 0 0 || true)"
  if [[ -z "$smart_hdr" ]]; then
    adb_cmd shell input swipe $(( W / 2 )) $(( H * 72 / 100 )) $(( W / 2 )) $(( H * 32 / 100 )) 350
    sleep 0.7
    smart_hdr="$(find_node_coords "SMART PLAYLISTS" 0 0 || true)"
  fi
  if [[ -n "$smart_hdr" ]]; then
    smart_y="${smart_hdr#* }"
    smart_min_y=$(( smart_y > 60 ? smart_y - 60 : 0 ))
    if tap_match_or_warn "NEW" "$smart_min_y" 0; then
      snap_step "09_smart_editor.png"
      adb_cmd shell input keyevent KEYCODE_BACK
      sleep 0.7
    fi
  else
    echo "warn: 'SMART PLAYLISTS' header not found; skipped Smart Editor shot" >&2
  fi

  # 5. Search
  if tap_nav "SEARCH"; then
    snap_step "10_search.png"
  fi

  # 6. Settings (top / mid / bottom)
  if tap_nav "SETTINGS"; then
    snap_step "11_settings_top.png"
    adb_cmd shell input swipe $(( W / 2 )) $(( H * 72 / 100 )) $(( W / 2 )) $(( H * 24 / 100 )) 350
    sleep 0.8
    snap_step "12_settings_mid.png"
    adb_cmd shell input swipe $(( W / 2 )) $(( H * 72 / 100 )) $(( W / 2 )) $(( H * 24 / 100 )) 350
    sleep 0.8
    snap_step "13_settings_bottom.png"
    adb_cmd shell input swipe $(( W / 2 )) $(( H * 25 / 100 )) $(( W / 2 )) $(( H * 78 / 100 )) 300
    sleep 0.5
    adb_cmd shell input swipe $(( W / 2 )) $(( H * 25 / 100 )) $(( W / 2 )) $(( H * 78 / 100 )) 300
    sleep 0.5
  else
    echo "warn: skipped Settings shots" >&2
  fi

  # 7. Now Playing (if MiniBar is present)
  local minibar_btn
  minibar_btn="$(find_node_coords "Play" $(( H * 78 / 100 )) 0 || true)"
  if [[ -z "$minibar_btn" ]]; then
    minibar_btn="$(find_node_coords "Pause" $(( H * 78 / 100 )) 0 || true)"
  fi
  if [[ -n "$minibar_btn" ]]; then
    local btn_y="${minibar_btn#* }"
    adb_cmd shell input tap $(( W * 35 / 100 )) "$btn_y"
    sleep 1.3
    snap_step "14_now_playing_art.png"

    if tap_match_or_warn "Volume and equalizer" 0 0; then
      snap_step "15_volume_eq_sheet.png"
      adb_cmd shell input keyevent KEYCODE_BACK
      sleep 0.8
    fi
    if tap_match_or_warn "LYRICS" 0 0; then
      snap_step "16_now_playing_lyrics.png"
    fi
    if tap_match_or_warn "QUEUE" 0 0; then
      snap_step "17_now_playing_queue.png"
    fi
    tap_match_or_warn "ART" 0 0 || true
    adb_cmd shell input keyevent KEYCODE_BACK
    sleep 0.8
  else
    echo "note: MiniBar not visible (no current track); skipped Now Playing shots"
  fi

  tap_nav "LIBRARY" || true
  cleanup_remote_xml
  echo "==> Tour complete: $out_dir"
  ls -lh "$out_dir"
}

usage() {
  cat <<'EOF'
Usage: scripts/device-test.sh [--serial <id>] <command> [args]

Commands:
  wake                     Wake device screen & dismiss keyguard
  install [--skip-build]   Build debug APK (unless --skip-build) & adb install -r
  launch [--restart]       Wake device & launch Rustygrade MainActivity
  shot [name] [--out dir]  Capture PNG screenshot of current screen
  dump                     Print UI hierarchy nodes with center tap coordinates
  tap <text|desc | x y>    Tap matching node or raw coordinates
  tour [--out dir]         Capture read-only screenshot tour across all screens
  logcat                   Show recent Rustygrade / crash logcat lines
  help                     Show this help
EOF
}

ARGS=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --serial)
      [[ $# -ge 2 ]] || { echo "error: --serial requires a value" >&2; exit 1; }
      SERIAL="$2"
      shift 2
      ;;
    *)
      ARGS+=("$1")
      shift
      ;;
  esac
done

set -- ${ARGS[@]+"${ARGS[@]}"}
SUBCMD="${1:-help}"
shift || true

case "$SUBCMD" in
  wake) cmd_wake "$@" ;;
  install) cmd_install "$@" ;;
  launch) cmd_launch "$@" ;;
  shot) cmd_shot "$@" ;;
  dump) cmd_dump "$@" ;;
  tap) cmd_tap "$@" ;;
  tour) cmd_tour "$@" ;;
  logcat) cmd_logcat "$@" ;;
  help|-h|--help) usage ;;
  *) echo "error: unknown command '$SUBCMD'" >&2; usage >&2; exit 1 ;;
esac
