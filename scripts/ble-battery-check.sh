#!/usr/bin/env bash
# Acceptance check 8 (T08b §5.2): idle drain with Discovery on, against R0-N4's
# < 3 %/day.
#
# ── Why this is an A/B and not a single reading ─────────────────────────────
# `batterystats` reports Bluetooth power as one global figure, and attributes it
# to **no app** (`apps: 0`). The first attempt at this check read that figure and
# called all of it Hoppler's, which it is not: the system and any other app using
# the radio are in the same number. §5.0.24 records ~4.6 %/day on that basis and
# should be read with this in mind.
#
# So this runs two windows of equal length — Discovery on, then Discovery off —
# and reports the *difference*. Whatever the baseline is, it appears in both and
# cancels. The A/B is the point of this script; the total is not the answer.
#
# ── The confound this cannot remove ─────────────────────────────────────────
# A phone on USB is charging, so `Computed drain` is 0 and the figures are
# power-*model* estimates rather than observed discharge. `dumpsys battery
# unplug` makes the framework account as if on battery, which is what lets the
# counters accumulate at all, but it cannot stop electrons.
#
# For a real check 8 the phone has to be **physically unplugged**, and this
# script cannot do that for you. There is no flag for it. By hand:
#
#   adb -s <serial> shell dumpsys batterystats --reset
#   (unplug; leave it an hour with Discovery on and the screen on; replug)
#   adb -s <serial> shell dumpsys batterystats org.hoppler.hoppler
#
# `batterystats` accumulates on-device, so nothing is lost while adb is away,
# and `Computed drain` comes back as a real discharge instead of zero.
#
# ── Usage ──────────────────────────────────────────────────────────────────
#   scripts/ble-battery-check.sh <adb-serial> [minutes-per-window]
#
# The phone must have Hoppler in the foreground, Discovery ON, and the screen
# staying on — R0-N6 makes Discovery a foreground activity, so screen-on is the
# operating mode and not an artefact to be excluded.
set -uo pipefail

SERIAL="${1:?usage: ble-battery-check.sh <adb-serial> [minutes-per-window]}"
MINUTES="${2:-25}"
ADB="${ADB:-adb}"
PKG=org.hoppler.hoppler
SECONDS_PER_WINDOW=$((MINUTES * 60))

"$ADB" -s "$SERIAL" get-state >/dev/null 2>&1 || { echo "adb cannot reach $SERIAL"; exit 2; }

# Reads the global Bluetooth power estimate, in mAh, for the window just ended.
bluetooth_mah() {
  "$ADB" -s "$SERIAL" shell dumpsys batterystats "$PKG" 2>/dev/null \
    | grep -oE '^ +bluetooth: [0-9.]+' | head -1 | awk '{print $2}'
}

on_battery_seconds() {
  "$ADB" -s "$SERIAL" shell dumpsys batterystats "$PKG" 2>/dev/null \
    | grep -oE 'Time on battery: [0-9hms ]+' | head -1
}

window() {
  local label="$1"
  "$ADB" -s "$SERIAL" shell dumpsys batterystats --reset >/dev/null 2>&1
  "$ADB" -s "$SERIAL" shell dumpsys battery unplug >/dev/null 2>&1
  echo "== $label: ${MINUTES} min =="
  sleep "$SECONDS_PER_WINDOW"
  local mah
  mah=$(bluetooth_mah)
  echo "   bluetooth: ${mah:-<none reported>} mAh   ($(on_battery_seconds))"
  echo "${mah:-}"
}

# Discovery must already be ON when this starts, and the screen must stay on.
# Neither is checked from here: an app in the wrong state would produce a
# confident and meaningless number, and guessing at the UI is how that happens.
ON=$(window "A — Discovery ON" | tail -1)

# Toggled from here rather than by hand, so window B starts at a known moment
# and the two windows are the same length. Coordinates match the Discovery
# switch; override for a different screen size.
"$ADB" -s "$SERIAL" shell input tap "${TAP_X:-912}" "${TAP_Y:-359}"
sleep 3
OFF=$(window "B — Discovery OFF" | tail -1)

"$ADB" -s "$SERIAL" shell dumpsys battery reset >/dev/null 2>&1

echo
echo "──────── result ────────"
if [ -z "$ON" ] || [ -z "$OFF" ]; then
  echo "INCONCLUSIVE — one of the windows reported no bluetooth figure at all."
  echo "A missing number is not a small one: without both, the difference that"
  echo "this whole design rests on cannot be taken."
  exit 3
fi

python3 - "$ON" "$OFF" "$MINUTES" <<'PY'
import sys
on, off, minutes = float(sys.argv[1]), float(sys.argv[2]), float(sys.argv[3])
delta = on - off
hours = minutes / 60
print(f"Discovery on : {on:.3f} mAh over {minutes:.0f} min")
print(f"Discovery off: {off:.3f} mAh over {minutes:.0f} min   (baseline: system and other apps)")
print(f"attributable : {delta:.3f} mAh -> {delta/hours:.2f} mAh/h -> {delta/hours*24:.0f} mAh/day")
if delta <= 0:
    print()
    print("INCONCLUSIVE — Discovery off cost as much as Discovery on. Either the")
    print("radio was still running in window B, or the difference is below what")
    print("this method can resolve. Do not read a negative number as a saving.")
    sys.exit(3)
for cap in (4500,):
    print(f"against a {cap} mAh battery: {delta/hours*24/cap*100:.2f} %/day   (R0-N4 budget: < 3 %/day)")
print()
print("Model estimates unless the phone was physically unplugged — see the header.")
PY
