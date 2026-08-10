#!/usr/bin/env bash
# Acceptance check 3 (T08b §5.2): Discovery off must silence the radio, not
# just the screen.
#
# This is the only Ring 0 acceptance that cannot be judged from inside the app,
# because the app claiming it stopped is precisely what is under test. So the
# observer is this host's own Bluetooth controller, reading LE advertising
# reports at HCI level through btmon — below BlueZ, below the app, and on a
# different machine from the one being tested.
#
# Why not BlueZ over D-Bus: it deduplicates. With `duplicate-data on` and a
# device sitting 40 cm away, it still emitted one PropertiesChanged in twelve
# seconds, so absence of D-Bus traffic says nothing about absence of
# advertisements. Measured, not assumed.
#
# ── One-time setup ─────────────────────────────────────────────────────────
#   sudo setcap 'cap_net_raw,cap_net_admin+eip' "$(command -v btmon)"
# or run this whole script under sudo. btmon needs a monitor socket; nothing
# else here does.
#
# ── Usage ──────────────────────────────────────────────────────────────────
#   scripts/ble-silence-check.sh <adb-serial> [seconds-before] [seconds-after]
#
# The phone must have Hoppler in the foreground with Discovery ON, and be the
# only Hoppler device advertising. The script turns Discovery off itself, so
# the moment it happens is known to the millisecond rather than eyeballed.
#
# ── Privacy note ───────────────────────────────────────────────────────────
# This capture contains Bluetooth addresses of every device in range, including
# ones belonging to passers-by. It is written to a temporary file, and the
# summary deliberately reports counts and timings rather than addresses. Do not
# paste raw captures into the findings: a line pairing a Hoppler peer id with a
# Bluetooth address outlives the id rotation and undoes R0-F2, which is the very
# property this check exists to prove.
set -uo pipefail

UUID_FRAGMENT="6f8c1d2e"   # BleAdapter.SERVICE_UUID, first group
SERIAL="${1:?usage: ble-silence-check.sh <adb-serial> [before-secs] [after-secs]}"
BEFORE="${2:-20}"
AFTER="${3:-20}"
ADB="${ADB:-adb}"

command -v btmon >/dev/null || { echo "btmon not found (bluez package)"; exit 2; }
command -v bluetoothctl >/dev/null || { echo "bluetoothctl not found"; exit 2; }
"$ADB" -s "$SERIAL" get-state >/dev/null 2>&1 || { echo "adb cannot reach $SERIAL"; exit 2; }

WORK="$(mktemp -d)"
BTMON=""
SCAN=""
# Ctrl-C is the normal way out of a run you have decided against, and an
# orphaned btmon or scan keeps the controller busy for the next one. Kill what
# this script started, whatever the exit path.
cleanup() {
  [ -n "$BTMON" ] && kill "$BTMON" 2>/dev/null
  [ -n "$SCAN" ] && kill "$SCAN" 2>/dev/null
  wait "$BTMON" "$SCAN" 2>/dev/null
  rm -rf "$WORK"
}
trap cleanup EXIT INT TERM
CAP="$WORK/btmon.txt"

echo "== starting the observer =="
btmon --time-format=time > "$CAP" 2>"$WORK/btmon.err" &
BTMON=$!
sleep 1
if ! kill -0 "$BTMON" 2>/dev/null || grep -qi "not permitted" "$WORK/btmon.err"; then
  echo "btmon could not open a monitor socket. Grant it once with:"
  echo "  sudo setcap 'cap_net_raw,cap_net_admin+eip' \"\$(command -v btmon)\""
  exit 2
fi

# A scan has to be running or the controller reports nothing at all.
bluetoothctl --timeout $((BEFORE + AFTER + 10)) scan le >/dev/null 2>&1 &
SCAN=$!

echo "== watching for ${BEFORE}s with Discovery ON =="
sleep "$BEFORE"

BEFORE_HITS=$(grep -ci "$UUID_FRAGMENT" "$CAP")
BEFORE_TOTAL=$(grep -c "LE Advertising Report" "$CAP")
if [ "$BEFORE_HITS" -eq 0 ]; then
  kill "$BTMON" "$SCAN" 2>/dev/null
  echo
  echo "INCONCLUSIVE — never saw the phone advertising in the first place."
  echo "  advertising reports from all devices: $BEFORE_TOTAL"
  echo "  carrying $UUID_FRAGMENT: 0"
  echo
  echo "So this run cannot judge silence: a check that only knows how to see"
  echo "nothing would pass whether or not the radio stopped. Confirm Discovery"
  echo "is on, the app is foregrounded, and the phone is in range."
  exit 3
fi

echo "   seen advertising: $BEFORE_HITS reports carrying $UUID_FRAGMENT"
MARK_LINE=$(grep -n -i "$UUID_FRAGMENT" "$CAP" | tail -1 | cut -d: -f1)

echo "== turning Discovery OFF via adb =="
"$ADB" -s "$SERIAL" shell input tap "${TAP_X:-918}" "${TAP_Y:-364}"
date '+   toggled at %H:%M:%S'

echo "== watching for ${AFTER}s with Discovery OFF =="
sleep "$AFTER"
kill "$BTMON" "$SCAN" 2>/dev/null
wait "$BTMON" 2>/dev/null

AFTER_HITS=$(tail -n +"$((MARK_LINE + 1))" "$CAP" | grep -ci "$UUID_FRAGMENT")
AFTER_TOTAL=$(tail -n +"$((MARK_LINE + 1))" "$CAP" | grep -c "LE Advertising Report")
# The control has to exclude the device under test, or on a FAIL it counts the
# very advertisements it is supposed to be independent of.
AFTER_OTHER=$((AFTER_TOTAL - AFTER_HITS))

echo
echo "──────── result ────────"
echo "before: $BEFORE_HITS advertisements carrying $UUID_FRAGMENT"
echo "after : $AFTER_HITS advertisements carrying $UUID_FRAGMENT"
echo "control: $AFTER_OTHER advertising reports from *other* devices after the toggle"

# The control matters as much as the result. A scanner that died the moment
# Discovery went off would report perfect silence, and be measuring nothing.
if [ "$AFTER_OTHER" -eq 0 ]; then
  echo
  echo "INCONCLUSIVE — the controller heard nothing from any *other* device"
  echo "afterwards. The scan probably stopped, so the silence is the observer's"
  echo "and not the phone's."
  exit 3
fi

if [ "$AFTER_HITS" -eq 0 ]; then
  echo
  echo "PASS — silent for the whole ${AFTER}s window, while the observer kept"
  echo "hearing $AFTER_OTHER advertisements from other devices."
  echo
  echo "Note this verifies silence across the window; it does not by itself"
  echo "assert the acceptance bound. Run with an after-window of 5 to test that"
  echo "directly — at HCI rates a 5s window holds many advertisements, so zero"
  echo "in it is the acceptance bound itself, not an artefact of a slow"
  echo "observer."
  echo
  echo "Record the phone's make, model and Android version"
  echo "in T08b §5.4; OEM variation in BLE is the norm."
  exit 0
fi

echo
echo "FAIL — $AFTER_HITS advertisement(s) carrying the Hoppler UUID after"
echo "Discovery was switched off. R0-F2 says the radio stops, not that the list"
echo "hides. Timestamps of the offending reports:"
tail -n +"$((MARK_LINE + 1))" "$CAP" | grep -i -B2 "$UUID_FRAGMENT" | grep -oE "^[0-9:.]+" | head -5
exit 1
