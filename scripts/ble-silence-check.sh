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
# ── What this can and cannot claim ─────────────────────────────────────────
# BlueZ starts its scan with the controller's **duplicate filter enabled**
# (`Filter duplicates: Enabled` in the HCI trace), so a device that is
# advertising steadily is reported roughly once every ten seconds rather than
# many times a second. Measured, not assumed.
#
# That is enough to answer *did the radio stop* over a window of twenty seconds
# or more: a still-advertising phone will produce a report inside it. It is
# **not** enough to assert the acceptance bound of five seconds — at that rate a
# five-second window normally contains no report at all, so zero would be the
# expected result from a working radio and a stopped one alike. Running this
# with a five-second window would manufacture a pass.
#
# Closing the 5 s bound needs the scan driven with duplicate filtering off,
# which BlueZ does not expose here. That is unbuilt.
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
# summary reports counts rather than addresses, except on a FAIL where naming
# the device that kept advertising is the whole diagnostic value. Do not
# paste raw captures into the findings: a line pairing a Hoppler peer id with a
# Bluetooth address outlives the id rotation and undoes R0-F2, which is the very
# property this check exists to prove.
set -uo pipefail

# BleAdapter.SERVICE_UUID as it appears on the wire: 128-bit UUIDs are
# little-endian in advertising data, so the text form never appears in btmon
# output. The first version of this script grepped for "6f8c1d2e" and could not
# have matched anything — see the note on rates below for how that was found.
UUID_WIRE="6f5e4d3c2b1a0f9e5d4c3b7a2e1d8c6f"
SERIAL="${1:?usage: ble-silence-check.sh <adb-serial> [before-secs] [after-secs]}"
BEFORE="${2:-20}"
AFTER="${3:-20}"
ADB="${ADB:-adb}"

command -v btmon >/dev/null || { echo "btmon not found (bluez package)"; exit 2; }
command -v bluetoothctl >/dev/null || { echo "bluetoothctl not found"; exit 2; }
# The report parser is python. Without it every count comes back empty and the
# arithmetic below fails with something that looks nothing like the real cause.
command -v python3 >/dev/null || { echo "python3 not found (needed to parse btmon output)"; exit 2; }
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

# Counts advertising reports carrying the Hoppler UUID.
#
# A plain grep cannot do this. btmon prints advertising data as a wrapped
# hexdump, so the UUID's sixteen bytes are regularly split across two lines,
# and the bytes also appear twice per advertisement (service-UUID list and
# service data). Both would miscount. This walks the report blocks instead and
# joins each one's data before looking.
# hoppler_reports <capture> [addresses]
#   default     — prints how many reports carried the Hoppler UUID
#   "addresses" — prints the addresses that carried it, with per-address counts
hoppler_reports() {
  python3 - "$1" "$UUID_WIRE" "${2:-count}" <<'PYEOF'
import re, sys
path, needle, mode = sys.argv[1], sys.argv[2], sys.argv[3]
addr_re = re.compile(r'^\s*Address: ([0-9A-F:]{17})')
data_re = re.compile(r'^\s*Advertising Data\[\d+\]:')
hex_re  = re.compile(r'^\s{6,}((?:[0-9a-f]{2} )+)\s*')
hits, addr, buf, in_data = {}, None, [], False
def record(a, b):
    if a and needle in "".join(b):
        hits[a] = hits.get(a, 0) + 1
for ln in open(path, errors="ignore"):
    if addr_re.match(ln):
        record(addr, buf)
        addr, buf, in_data = addr_re.match(ln).group(1), [], False
        continue
    if data_re.match(ln):
        in_data = True
        continue
    if in_data:
        h = hex_re.match(ln)
        if h:
            buf.append(h.group(1).replace(" ", ""))
        else:
            in_data = False
record(addr, buf)
if mode == "addresses":
    for a, c in sorted(hits.items(), key=lambda kv: -kv[1]):
        print(f"  {a}  {c} report(s)")
else:
    print(sum(hits.values()))
PYEOF
}

echo "== starting the observer =="
btmon > "$CAP" 2>"$WORK/btmon.err" &
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

BEFORE_HITS=$(hoppler_reports "$CAP")
BEFORE_TOTAL=$(grep -c "Advertising Data\[" "$CAP")
if [ "$BEFORE_HITS" -eq 0 ]; then
  kill "$BTMON" "$SCAN" 2>/dev/null
  echo
  echo "INCONCLUSIVE — never saw the phone advertising in the first place."
  echo "  advertising reports from all devices: $BEFORE_TOTAL"
  echo "  carrying the Hoppler UUID: 0"
  echo
  echo "So this run cannot judge silence: a check that only knows how to see"
  echo "nothing would pass whether or not the radio stopped. Confirm Discovery"
  echo "is on, the app is foregrounded, and the phone is in range."
  exit 3
fi

echo "   seen advertising: $BEFORE_HITS report(s) carrying the Hoppler UUID"
MARK_LINE=$(wc -l < "$CAP")

echo "== turning Discovery OFF via adb =="
"$ADB" -s "$SERIAL" shell input tap "${TAP_X:-918}" "${TAP_Y:-364}"
date '+   toggled at %H:%M:%S'

echo "== watching for ${AFTER}s with Discovery OFF =="
sleep "$AFTER"
kill "$BTMON" "$SCAN" 2>/dev/null
wait "$BTMON" 2>/dev/null

tail -n +"$((MARK_LINE + 1))" "$CAP" > "$WORK/after.txt"
AFTER_HITS=$(hoppler_reports "$WORK/after.txt")
AFTER_TOTAL=$(grep -c "Advertising Data\[" "$WORK/after.txt")
# The control has to exclude the device under test, or on a FAIL it counts the
# very advertisements it is supposed to be independent of.
AFTER_OTHER=$((AFTER_TOTAL - AFTER_HITS))

echo
echo "──────── result ────────"
echo "before: $BEFORE_HITS report(s) carrying the Hoppler UUID"
echo "after : $AFTER_HITS report(s) carrying the Hoppler UUID"
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
  echo "This verifies silence across the window. It does NOT establish the"
  echo "five-second acceptance bound: BlueZ scans with the controller's"
  echo "duplicate filter on, so a live advertiser is only reported about once"
  echo "every ten seconds, and a five-second window would read as silent either"
  echo "way. Keep the window at twenty seconds or more."
  echo
  echo "Record the phone's make, model and Android version"
  echo "in T08b §5.4; OEM variation in BLE is the norm."
  exit 0
fi

echo
echo "FAIL — $AFTER_HITS advertisement(s) carrying the Hoppler UUID after"
echo "Discovery was switched off. R0-F2 says the radio stops, not that the list"
echo "hides. The addresses still advertising it, for local diagnosis only:"
hoppler_reports "$WORK/after.txt" addresses
echo
echo "Those are Bluetooth addresses. They tell you *which* device did not stop —"
echo "worth knowing when more than one Hoppler phone is in the room — and they"
echo "must not be pasted into the findings alongside a peer id, for the same"
echo "R0-F2 reason this check exists."
exit 1
