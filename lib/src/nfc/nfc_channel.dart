/// The Dart half of the NFC platform channel (T11, R0-F4).
///
/// Same arrangement as the BLE channel and for the same reason: this layer
/// decides nothing. It turns calls into method invocations and platform facts
/// into typed events, so every rule lives either in Rust — where it is tested —
/// or in the adapter, where it must be, and never here in between.
///
/// In particular a code read off a tap is handed on **as text**, exactly as the
/// camera hands one on. Whether it is an invite, whose it is, and what happens
/// next are questions the core already answers, and answering them twice is how
/// the two paths would drift apart.
library;

import 'dart:async';

import 'package:flutter/services.dart';

const MethodChannel _commands = MethodChannel('org.hoppler/nfc');
const EventChannel _events = EventChannel('org.hoppler/nfc/events');

/// The event vocabulary, declared here and asserted against the adapter's own
/// constants by `test/nfc_channel_vocabulary_test.dart` — the two halves of a
/// channel drifting apart is a class of bug this project has already had once.
const Set<String> nfcEventTypes = {'codeRead', 'unreadable'};

/// Something the adapter reported.
sealed class NfcEvent {
  const NfcEvent();
}

/// A code was read off another phone. Raw text, unparsed and unvalidated.
class NfcCodeRead extends NfcEvent {
  const NfcCodeRead(this.code);
  final String code;
}

/// A tap happened and produced nothing usable.
///
/// Reported rather than swallowed: two people holding phones together need to
/// know it did not take, and silence is indistinguishable from not having
/// tapped at all. The reason is a sentence to show, not a code to branch on —
/// every cause has the same remedy, which is to tap again.
class NfcUnreadable extends NfcEvent {
  const NfcUnreadable(this.reason);
  final String reason;
}

/// Whether this device can take part in a tap at all.
///
/// Answered honestly rather than optimistically. Plenty of phones have no NFC,
/// or have it switched off, and offering "tap to pair" on one of them is an
/// instruction nobody can follow — the same rule R0-N6 applies to the radio.
Future<bool> nfcAvailable() async {
  try {
    return await _commands.invokeMethod<bool>('isAvailable') ?? false;
  } on PlatformException {
    return false;
  } on MissingPluginException {
    // Desktop, where the channel does not exist. Not an error: it is the
    // answer.
    return false;
  }
}

/// Make this device's code readable by a tap, until [stopSharingCode].
///
/// The code is the same string the QR carries. One payload, two ways of
/// getting it across — which is why the invite format knows nothing about
/// either.
Future<void> shareCode(String code) =>
    _commands.invokeMethod<void>('startSharing', {'code': code});

/// Stop answering taps.
///
/// Called whenever the code leaves the screen, for the reason the QR path has:
/// a code that is still readable after its owner put the phone down keeps a
/// Layer-2 key paired with the rung id this device advertises under, and R0-F2
/// spends twelve minutes at a time making that link hard to observe.
Future<void> stopSharingCode() => _commands.invokeMethod<void>('stopSharing');

/// Start looking for another phone's code.
Future<void> startReadingTaps() => _commands.invokeMethod<void>('startReading');

/// Stop looking.
Future<void> stopReadingTaps() => _commands.invokeMethod<void>('stopReading');

/// What the adapter reports.
///
/// Nothing here throws. A `map` that casts and fails takes the *stream* down
/// with it, not just the event — and a dead stream means every later tap goes
/// silent, which on this screen is indistinguishable from not having tapped at
/// all. So a malformed event becomes a failed tap and the stream lives; the
/// BLE channel treats its own the same way.
Stream<NfcEvent> nfcEvents() => _events.receiveBroadcastStream().map((event) {
  if (event is! Map) return const NfcUnreadable('that tap did not take');
  final map = event.cast<Object?, Object?>();
  final value = map['value'];
  final text = value is String ? value : '';
  return switch (map['type']) {
    // Only a code with actual text is a code. An empty one would be handed to
    // the core as an invite, refused there, and reported as nothing at all.
    'codeRead' when text.isNotEmpty => NfcCodeRead(text),
    // Anything else is a failed tap rather than a dropped event. An adapter on
    // a newer build must not be able to make an older app go quiet.
    _ => NfcUnreadable(text.isEmpty ? 'that tap did not take' : text),
  };
});
