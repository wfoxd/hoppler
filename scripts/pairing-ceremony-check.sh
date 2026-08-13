#!/usr/bin/env bash
# R0-F4 acceptance on two phones (T11): does the ceremony complete, in time,
# with the same colours on both screens?
#
# ── What this can and cannot decide ────────────────────────────────────────
# Three things are being claimed, and they are not equally measurable.
#
# **Completes at all.** Fully measurable from here, and the whole point of a
# hardware run: everything up to now has been verified over a loopback rung
# with no radio and no people in it.
#
# **Under fifteen seconds.** Measurable, but it has to be split or the number
# means nothing. The budget covers a human comparing colours, so a slow run
# might be a slow protocol or might be somebody thinking. This reports two
# intervals per phone:
#
#   started -> colours   what the code costs: dial, session, three handshake
#                        messages. This is the part a change can regress.
#   colours -> paired    what the two people cost, plus one round trip. Long
#                        here is not a defect.
#
# Not in either: the seconds spent aiming a camera before a code decodes.
# Nothing observes that but the person holding the phone. So a pass here is
# "the ceremony fits the budget with time to spare for aiming", not "the
# fifteen seconds are accounted for".
#
# **The same colours on both screens.** *Not* decidable from logs, on purpose.
# The SAS is deliberately never logged — it is the one value a relaying
# attacker is trying to guess, and a log outlives the ceremony and leaves the
# device in any diagnostics export. So this grabs a screenshot of each phone
# at the moment both are showing colours, and a person compares them. That is
# also the honest test: the claim is about what two people see.
#
# ── Usage ──────────────────────────────────────────────────────────────────
#   scripts/pairing-ceremony-check.sh <shower-serial> <scanner-serial>
#
# The *shower* displays a code; the *scanner* reads it. Both phones need the
# app in the foreground with Discovery on. The script clears the logs, waits
# for you to run the ceremony by hand, and reads the result out afterwards.
#
# ── Which rung ─────────────────────────────────────────────────────────────
# The ceremony rides an established session and does not care which rung
# carried it, but the *number* does: BLE dials slower than LAN, and the
# `started -> colours` interval is mostly that dial.
#
# Both phones must be on the same rung, which is a build-time choice:
#
#   flutter build apk --debug                                  # LAN (default)
#   flutter build apk --debug --dart-define=HOPPLER_RADIO=ble   # BLE
#
# LAN first is the cheaper order — it proves the ceremony works at all with one
# fewer thing able to go wrong — but tech spec §6 says the ceremony happens over
# BLE, so the BLE run is the one that answers R0-F4.
set -uo pipefail

SHOWER="${1:?usage: pairing-ceremony-check.sh <shower-serial> <scanner-serial>}"
SCANNER="${2:?usage: pairing-ceremony-check.sh <shower-serial> <scanner-serial>}"
ADB="${ADB:-adb}"
PKG=org.hoppler.hoppler
BUDGET_MS=15000

for serial in "$SHOWER" "$SCANNER"; do
  "$ADB" -s "$serial" get-state >/dev/null 2>&1 || { echo "adb cannot reach $serial"; exit 2; }
done
command -v python3 >/dev/null || { echo "python3 not found (needed to read the timings)"; exit 2; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Granted up front rather than left to the prompt. A permission dialog in the
# middle of a timed ceremony measures how fast someone taps "Allow", and the
# refusal path has its own screen and is not what this run is about.
echo "== granting the camera on the scanner =="
"$ADB" -s "$SCANNER" shell pm grant "$PKG" android.permission.CAMERA 2>/dev/null \
  || echo "   (could not pre-grant; the app will ask, which is fine but adds time)"

# The LE slot pool is phone-wide and fixed at about seven. A previous run that
# left channels open makes the next dial fail with a generic error that says
# nothing about slots — a full afternoon went into learning that once.
echo "== cycling Bluetooth on both phones =="
for serial in "$SHOWER" "$SCANNER"; do
  "$ADB" -s "$serial" shell cmd bluetooth_manager disable >/dev/null 2>&1
done
sleep 2
for serial in "$SHOWER" "$SCANNER"; do
  "$ADB" -s "$serial" shell cmd bluetooth_manager enable >/dev/null 2>&1
done
sleep 3

for serial in "$SHOWER" "$SCANNER"; do
  "$ADB" -s "$serial" logcat -c 2>/dev/null
done

cat <<'STEPS'

== run the ceremony now ==
  1. Both phones: Hoppler in the foreground, Discovery ON, and each seeing the
     other in the nearby list. Do not start until they do — a ceremony started
     before the peer is visible fails on resolution, which is a different test.
  2. Shower phone:  "Show my code".
  3. Scanner phone: "Scan a code", and point it at the other screen.
  4. Both phones will show two colours and a word. LOOK AT BOTH before tapping
     anything — that comparison is the acceptance, not the tap.
  5. Both phones: "They match".

Press Enter once both say they are paired (or once it has clearly failed).
STEPS
read -r _

echo "== reading the phones back =="
for serial in "$SHOWER" "$SCANNER"; do
  # -v epoch so the two devices can be compared without parsing local time, and
  # because these phones have a known ~1.6 s clock skew between them: each
  # phone's intervals are computed from its own clock only, never across.
  "$ADB" -s "$serial" logcat -d -v epoch 2>/dev/null > "$WORK/$serial.log"
done

python3 - "$WORK/$SHOWER.log" "$WORK/$SCANNER.log" "$SHOWER" "$SCANNER" "$BUDGET_MS" <<'PY'
import re, sys

paths = {sys.argv[3]: sys.argv[1], sys.argv[4]: sys.argv[2]}
budget = int(sys.argv[5])

MARKS = [
    ("started", re.compile(r"starting a ceremony with (\S+)")),
    ("colours", re.compile(r"ceremony with (\S+) reached the colours")),
    ("paired",  re.compile(r"paired with (\S+) on thread")),
]
FAILED = re.compile(r"ceremony with (\S+) (failed|abandoned)")

def stamps(path):
    found, failures = {}, []
    for line in open(path, errors="ignore"):
        head = line.split(None, 1)
        if not head:
            continue
        try:
            when = float(head[0])
        except ValueError:
            continue
        for name, pattern in MARKS:
            if pattern.search(line) and name not in found:
                found[name] = when
        if FAILED.search(line):
            failures.append(line.strip())
    return found, failures

verdict = 0
for serial, path in paths.items():
    found, failures = stamps(path)
    print(f"\n──── {serial} ────")
    for line in failures:
        print(f"  ! {line}")
    if "paired" not in found:
        print("  never paired.")
        # The shower has no "started" line — it never scanned anything — so a
        # missing one there is expected and not a finding on its own.
        if "colours" in found:
            print("  reached the colours, so the handshake worked and the")
            print("  ceremony died at or after the confirmations.")
        elif "started" in found:
            print("  started but never reached the colours: the handshake did")
            print("  not complete. A stale code or the wrong device.")
        else:
            print("  no ceremony markers at all. Was this phone in it?")
        verdict = 1
        continue

    if "started" in found:
        protocol = (found["colours"] - found["started"]) * 1000
        print(f"  started -> colours : {protocol:7.0f} ms   (dial, session, handshake)")
    else:
        protocol = None
        print("  started -> colours :       —   (this phone showed the code)")
    people = (found["paired"] - found["colours"]) * 1000
    total = None
    if "started" in found:
        total = (found["paired"] - found["started"]) * 1000
    print(f"  colours -> paired  : {people:7.0f} ms   (two people, plus a round trip)")
    if total is not None:
        print(f"  whole ceremony     : {total:7.0f} ms   against R0-F4's {budget} ms")
        if total > budget:
            print("  OVER BUDGET — but read the split above before calling it a")
            print("  regression. Slow humans and a slow protocol look identical")
            print("  in the total and completely different in the two halves.")
            verdict = 1

print()
print("Screenshots of both screens are what decides the colours. Take them at")
print("the moment both are showing colours — this script cannot, because it")
print("cannot know when that is, and the SAS is deliberately never logged:")
print()
print(f"  adb -s {sys.argv[3]} exec-out screencap -p > shower-sas.png")
print(f"  adb -s {sys.argv[4]} exec-out screencap -p > scanner-sas.png")
print()
print("Record the make, model and Android version of both phones in T08b §5.4.")
sys.exit(verdict)
PY
