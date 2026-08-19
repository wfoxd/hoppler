#!/usr/bin/env bash
# R0-F1 acceptance on a phone: is the master secret really in platform hardware,
# and does a device that already had an identity keep it?
#
# ── What this can and cannot decide ────────────────────────────────────────
#
# **The key is hardware-backed.** Decidable, but only by asking the device.
# Requesting StrongBox and receiving it are different things, and the keystore
# is permitted to hand back a software-backed key while every call above it
# succeeds identically. `HardwareKeystore.report` logs which one arrived and
# this fails on anything that is not `strongbox` or `tee`. That log line is the
# entire evidence for the claim — there is no way to infer it from behaviour.
#
# **The seeds on disk are ciphertext.** Decidable, and checked directly: the
# file is pulled and must begin with the `HPKW\x01` marker and must not contain
# the bytes it had before.
#
# **The ciphertext is useless off this device.** *Not* decidable here, and the
# script does not pretend otherwise. Proving it would mean showing no key
# anywhere else can open the blob, which is a negative over every device that
# exists. What is checkable is the premise it rests on — the key is in hardware
# and non-exportable — which is the check above. Stated so nobody reads a pass
# here as an extraction attempt that failed.
#
# **An existing device keeps its identity.** Decidable, and the reason this
# script insists on running before the new build is installed. Both dev phones
# hold bare, unwrapped seeds written before wrapping existed; the upgrade path
# that reads them is exercised exactly once per phone, and after that the phone
# can never test it again without being wiped. So the pre-state is captured
# first, or the run is worthless.
#
# The witness for "kept its identity" is the database. If the store master did
# not survive, the engine finds no master and takes its stale-database path,
# which deletes `hoppler.db` and recreates it — the failure measured on a Pixel
# on 18 Aug 2026, where the inode changed on every launch. A stable inode with
# the app running is the master having been unsealed. (Since #61 a *failed*
# unwrap is a `Backend` error rather than a missing secret, so the third
# outcome is the app refusing to start, which is also visible here.)
#
# ── Usage ──────────────────────────────────────────────────────────────────
#   scripts/keystore-hardware-check.sh <serial>            # phase 1, then stop
#   scripts/keystore-hardware-check.sh <serial> --after    # phase 2, after install
#
# Phase 1 must run against the OLD build, before installing. It prints what to
# do next. Phase 2 runs after `flutter install` and decides the result.
#
# Needs a debuggable build: everything here reads app-private storage through
# `run-as`, which release builds refuse.
set -uo pipefail

ADB="${ADB:-$HOME/Android/Sdk/platform-tools/adb}"
PKG=org.hoppler.hoppler
STATE_DIR="${TMPDIR:-/tmp}/hoppler-keystore-check"

SERIAL="${1:-}"
PHASE="${2:-}"
if [ -z "$SERIAL" ]; then
  echo "usage: $0 <serial> [--after]" >&2
  exit 2
fi
a() { "$ADB" -s "$SERIAL" "$@"; }

# `run-as` puts us inside the app's uid; without it the directory is unreadable
# and that is the point of it.
appfiles() { a shell run-as "$PKG" "$@"; }

if ! a shell true >/dev/null 2>&1; then
  echo "device $SERIAL is not reachable — unlock it and re-check \`adb devices\`" >&2
  exit 1
fi

# The keystore directory is `<files>/keys`, one hashed filename per label. The
# names are not predictable from here, so everything is done by listing.
APP_DIR="/data/data/$PKG/files"
KEYS_DIR="$APP_DIR/keys"

snapshot() {
  # sha256 of each secret, plus whether it carries the wrapped marker. Hashes
  # rather than contents: this file is written to the host, and a bare seed is
  # the store master.
  appfiles sh -c "cd $KEYS_DIR 2>/dev/null || exit 0; for f in *; do
      [ -e \"\$f\" ] || continue
      printf '%s %s %s\n' \"\$f\" \"\$(sha256sum \"\$f\" | cut -d' ' -f1)\" \\
        \"\$(head -c 5 \"\$f\" | od -An -c | tr -d ' \n')\"
    done"
}

mkdir -p "$STATE_DIR"
BEFORE="$STATE_DIR/$SERIAL.before"

if [ "$PHASE" != "--after" ]; then
  echo "── phase 1: the state this phone is in now ──────────────────────────"
  # Exact line, not a substring. `pm list packages` prints `package:<id>` and
  # a plain grep for the id would also match `org.hoppler.hoppler.debug` or any
  # other suffixed build — reporting "installed" about a different app, whose
  # storage this script then cannot read.
  if ! a shell pm list packages | tr -d '\r' | grep -qx "package:$PKG"; then
    echo "not installed. Nothing to migrate, so this phone can only test the"
    echo "fresh-install half. Install and re-run with --after."
    : > "$BEFORE"
    exit 0
  fi
  snapshot > "$BEFORE"
  if [ ! -s "$BEFORE" ]; then
    echo "no keystore files — this phone has no identity yet, so there is"
    echo "nothing to migrate. The fresh-install half still applies."
  else
    echo "secrets found (name, sha256, first 5 bytes):"
    cat "$BEFORE"
    if grep -q 'HPKW' "$BEFORE"; then
      echo
      echo "!! already wrapped. The migration path has run on this phone"
      echo "!! already and cannot be tested again without a wipe."
    else
      echo
      echo "bare, as expected — the migration path has not run here yet."
    fi
  fi
  # The inode is the witness for "the database survived". Captured before, so
  # a change afterwards is visible.
  appfiles sh -c "ls -i $APP_DIR/hoppler.db 2>/dev/null" \
    | tee -a "$BEFORE"
  echo
  echo "── next ─────────────────────────────────────────────────────────────"
  echo "  flutter install -d $SERIAL      # or: adb -s $SERIAL install -r <apk>"
  echo "  open Hoppler, wait for the main screen"
  echo "  $0 $SERIAL --after"
  exit 0
fi

if [ ! -f "$BEFORE" ]; then
  echo "no phase-1 state for $SERIAL. Run without --after first, on the old" >&2
  echo "build — the migration is one-shot and cannot be observed afterwards." >&2
  exit 2
fi

echo "── phase 2: what the new build did ──────────────────────────────────"
fail=0
note() { echo "  $*"; }
bad() { echo "  FAIL: $*"; fail=1; }

# 1. Hardware backing. The only evidence for the R0-F1 claim.
level=$(a logcat -d -s HopplerKeystore | grep -o "wrapping key backing: [a-z]*" | tail -1)
if [ -z "$level" ]; then
  bad "no 'wrapping key backing' line in logcat — the key was never opened."
  note "Is the app actually running, and was logcat cleared since?"
else
  note "$level"
  case "$level" in
    *strongbox|*tee) : ;;
    *) bad "the wrapping key is not in hardware, so R0-F1 is not met" ;;
  esac
fi

# 2. The secrets on disk are ciphertext, and are not what they were.
AFTER="$STATE_DIR/$SERIAL.after"
snapshot > "$AFTER"
[ -s "$AFTER" ] || bad "no secrets on the device at all after the upgrade"
# Over a file, not a pipeline. The first version of this looped over `echo |
# while`, which runs in a subshell — every failure it found set `fail=1` in a
# process that then exited, so a secret left bare printed FAIL and the script
# still passed.
while read -r name _ marker; do
  [ -n "${name:-}" ] || continue
  case "$marker" in
    *H*P*K*W*) note "$name wrapped" ;;
    *) bad "$name is still bare" ;;
  esac
done < "$AFTER"
# And that each one actually changed. A marker on a file whose bytes never
# moved would mean the prefix was written and the wrap was not.
while read -r name hash _; do
  [ -n "${name:-}" ] || continue
  case "$name" in [0-9]*) continue ;; esac   # the `ls -i` line, which starts with the inode
  # Name *and* hash. Matching on the hash alone asks "did any file survive
  # unchanged", which answers about the set rather than about this entry — a
  # removed file whose hash reappears elsewhere would be attributed to the
  # wrong label, and the verdict here is per-secret.
  if awk -v n="$name" -v h="$hash" '$1 == n && $2 == h {found=1} END {exit !found}' "$AFTER"; then
    bad "$name has the same contents as before — it was not wrapped"
  elif ! awk -v n="$name" '$1 == n {found=1} END {exit !found}' "$AFTER"; then
    # Gone entirely, which the wrapped/bare loop above cannot see: it walks
    # what is there now, and a secret that vanished is not there to walk. This
    # is the loss the whole check exists to catch — the upgrade read a seed and
    # left nothing behind — so it is louder than either other verdict.
    bad "$name is gone — a secret that existed before the upgrade no longer does"
  fi
done < "$BEFORE"

# 3. The identity survived: same database, still there.
before_inode=$(grep "hoppler.db" "$BEFORE" | awk '{print $1}')
after_inode=$(appfiles sh -c "ls -i $APP_DIR/hoppler.db 2>/dev/null" | awk '{print $1}')
if [ -z "$after_inode" ]; then
  bad "hoppler.db is gone — the master was not recovered"
elif [ -n "$before_inode" ] && [ "$before_inode" != "$after_inode" ]; then
  bad "hoppler.db inode $before_inode -> $after_inode: the database was"
  note "      deleted and rebuilt, which is the identity being lost"
elif [ -n "$before_inode" ]; then
  note "hoppler.db inode $after_inode unchanged — the master was unsealed"
fi

# 4. Anything the core refused.
if a logcat -d | grep -qi "hardware keystore was never registered"; then
  bad "the JNI bridge did not register — the core refused to open a store"
fi

echo
if [ "$fail" = 0 ]; then
  echo "PASS — hardware-backed, secrets wrapped on disk, identity kept."
  echo
  echo "Not shown, and not shown by design: that the ciphertext cannot be"
  echo "opened on another device. That rests on the key being non-exportable,"
  echo "which is what the backing level above attests."
else
  echo "FAIL — see above."
fi
exit "$fail"
