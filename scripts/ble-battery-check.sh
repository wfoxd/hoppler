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
# For a real check 8, the phone has to be **physically unplugged**, and this
# script cannot do that for you.
#
# The obvious procedure does not work, and it was tried: unplug, wait an hour,
# plug back in, read. **Android resets batterystats when the phone is plugged
# in** — `RESET:TIME` appears in `--history` at the moment of reconnection — so
# the reading destroys the very data it came for. A 68-minute unplugged run
# produced `Time on battery: 240ms`.
#
# What survives a replug is the battery *percentage*, which is a real
# device-level discharge and attributable to nothing in particular. What does
# not survive is the per-component breakdown, which is the only part that
# separates Hoppler from everything else on the phone.
#
# So an unplugged component-level run needs adb over Wi-Fi: pair first, unplug,
# and read while still disconnected from USB. That is unbuilt.
#
# ── Usage ──────────────────────────────────────────────────────────────────
#   scripts/ble-battery-check.sh <adb-serial> [minutes-per-window]
#
# The phone must have Hoppler in the foreground, Discovery ON, and the screen
# staying on — R0-N6 makes Discovery a foreground activity, so screen-on is the
# operating mode and not an artefact to be excluded.
#
# Check what else on the phone uses Bluetooth before trusting a figure. The test
# handset here also carries `com.bitchat.droid`, another BLE mesh app, which was
# running during the first measurements. A steady background user cancels in the
# A/B — that is the design working — but one whose activity varies between the
# two windows is noise this method cannot see.
set -uo pipefail

SERIAL="${1:?usage: ble-battery-check.sh <adb-serial> [minutes-per-window]}"
MINUTES="${2:-25}"
ADB="${ADB:-adb}"
PKG=org.hoppler.hoppler
case "$MINUTES" in
  ''|*[!0-9]*|0) echo "minutes-per-window must be a positive whole number, got '$MINUTES'"; exit 2 ;;
esac
SECONDS_PER_WINDOW=$((MINUTES * 60))

"$ADB" -s "$SERIAL" get-state >/dev/null 2>&1 || { echo "adb cannot reach $SERIAL"; exit 2; }

# `dumpsys battery unplug` is a change to the device, not to this process, and
# it outlives a Ctrl-C. Left set, it makes the phone report as on-battery for
# everything afterwards — so the next person's measurement is quietly wrong
# rather than obviously broken. Restore on every exit path.
restore() { "$ADB" -s "$SERIAL" shell dumpsys battery reset >/dev/null 2>&1; }

# Two traps, not one. A handler that cleans up but does not exit lets bash
# *resume* the script where the signal interrupted it — so a single
# `trap restore EXIT INT TERM` restored the device and then carried straight on
# into the next window, which set the spoof again. Observed rather than
# theorised: a five-minute window ended after nine seconds and the phone came
# back spoofed anyway.
restore_and_quit() { restore; exit 130; }
trap restore EXIT
trap restore_and_quit INT TERM

# Reads the global Bluetooth power estimate, in mAh, for the window just ended.
bluetooth_mah() {
  "$ADB" -s "$SERIAL" shell dumpsys batterystats "$PKG" 2>/dev/null \
    | grep -oE '^ *bluetooth: [0-9.]+' | head -1 | awk '{print $2}'
}

on_battery_seconds() {
  "$ADB" -s "$SERIAL" shell dumpsys batterystats "$PKG" 2>/dev/null \
    | grep -oE 'Time on battery: [0-9hms ]+' | head -1
}

# The battery this phone actually has, as the power model itself sees it.
# Hard-coding one number would print a confident percentage for any device —
# the two phones in this project are 4500 and 4298 mAh, so that is not
# hypothetical. `CAPACITY_MAH` overrides; if neither is available the script
# reports mAh/day and withholds the percentage rather than inventing a divisor.
capacity_mah() {
  if [ -n "${CAPACITY_MAH:-}" ]; then echo "$CAPACITY_MAH"; return; fi
  "$ADB" -s "$SERIAL" shell dumpsys batterystats "$PKG" 2>/dev/null \
    | grep -oE 'Capacity: [0-9]+' | head -1 | awk '{print $2}'
}

# Sets RESULT rather than echoing it, and waits on a backgrounded sleep.
#
# Both matter for the trap above. A command substitution would run this in a
# subshell, and bash defers a trap until the current foreground command returns
# — so with a plain `sleep` in there, Ctrl-C left the phone in the spoofed
# `battery unplug` state for the rest of the window, up to twenty-five minutes.
# That is worse than not trapping at all, because it looks handled. Waiting on a
# background sleep is what lets the signal land while the window is still open.
RESULT=""
window() {
  local label="$1"
  "$ADB" -s "$SERIAL" shell dumpsys batterystats --reset >/dev/null 2>&1
  "$ADB" -s "$SERIAL" shell dumpsys battery unplug >/dev/null 2>&1
  echo "== $label: ${MINUTES} min =="
  sleep "$SECONDS_PER_WINDOW" &
  wait $! || true
  RESULT=$(bluetooth_mah)
  echo "   bluetooth: ${RESULT:-<none reported>} mAh   ($(on_battery_seconds))"
}

# Discovery must already be ON when this starts, and the screen must stay on.
# Neither is checked from here: an app in the wrong state would produce a
# confident and meaningless number, and guessing at the UI is how that happens.
window "A — Discovery ON"
ON="$RESULT"

# Toggled from here rather than by hand, so window B starts at a known moment
# and the two windows are the same length. Coordinates match the Discovery
# switch; override for a different screen size.
"$ADB" -s "$SERIAL" shell input tap "${TAP_X:-912}" "${TAP_Y:-359}"
sleep 3
window "B — Discovery OFF"
OFF="$RESULT"

echo
echo "──────── result ────────"
if [ -z "$ON" ] || [ -z "$OFF" ]; then
  echo "INCONCLUSIVE — one of the windows reported no bluetooth figure at all."
  echo "A missing number is not a small one: without both, the difference that"
  echo "this whole design rests on cannot be taken."
  exit 3
fi

python3 - "$ON" "$OFF" "$MINUTES" "$(capacity_mah)" <<'PY'
import sys
on, off, minutes = float(sys.argv[1]), float(sys.argv[2]), float(sys.argv[3])
capacity = sys.argv[4].strip() if len(sys.argv) > 4 else ""
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
if capacity.isdigit() and int(capacity) > 0:
    cap = int(capacity)
    print(f"against this phone's {cap} mAh battery: {delta/hours*24/cap*100:.2f} %/day   (R0-N4 budget: < 3 %/day)")
else:
    print("battery capacity unknown, so no percentage — set CAPACITY_MAH to get one.")
    print("A guessed divisor would turn an honest mAh figure into a wrong verdict.")
print()
print("Model estimates unless the phone was physically unplugged — see the header.")
PY
