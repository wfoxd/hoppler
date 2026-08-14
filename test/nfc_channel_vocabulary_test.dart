import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/src/nfc/nfc_channel.dart';

/// The NFC channel's half of the same guard `ble_channel_vocabulary_test.dart`
/// applies to BLE, and here for the same reason: the event protocol is a set of
/// strings emitted by Kotlin and matched by Dart, with no shared type between
/// them. Each side tested against its own literals would stay green while a
/// rename stopped taps reporting anything on real phones.
///
/// Reading the Kotlin source is unusual and is the only mechanism available.
/// Adding codegen for two strings would cost more than it saves; letting them
/// drift costs a hardware session.
const relativeAdapterPath =
    'android/app/src/main/kotlin/org/hoppler/hoppler/nfc/NfcAdapter.kt';

void main() {
  /// Walked up from the current directory rather than assumed, so a run from a
  /// subdirectory fails as "not found here" rather than as "the adapter moved".
  File? findAdapter() {
    for (var dir = Directory.current; ; dir = dir.parent) {
      final candidate = File('${dir.path}/$relativeAdapterPath');
      if (candidate.existsSync()) return candidate;
      if (dir.parent.path == dir.path) return null;
    }
  }

  test('Kotlin emits exactly the tap event types Dart decodes', () {
    final source = findAdapter();
    expect(
      source,
      isNotNull,
      reason:
          'cannot find $relativeAdapterPath anywhere at or above '
          '${Directory.current.path} — if the adapter moved, point this test at '
          'it rather than deleting it: it is the only thing holding the two '
          'halves of the channel together',
    );

    final declared = RegExp(
      r'const val EVENT_[A-Z_]+ = "([a-zA-Z]+)"',
    ).allMatches(source!.readAsStringSync()).map((m) => m.group(1)!).toSet();

    expect(
      declared,
      isNotEmpty,
      reason:
          'found no EVENT_ constants in NfcAdapter.kt. They are declared as '
          'plain `const val NAME = "value"` lines so this regex can see them; '
          'if that changed, this test can no longer couple the languages and '
          'is silently worthless — fix the parse, do not relax the assertion',
    );

    expect(
      declared,
      equals(nfcEventTypes),
      reason:
          'the two halves of the NFC channel have drifted. Kotlin emits '
          '${declared.difference(nfcEventTypes)} that Dart does not decode, and '
          'Dart expects ${nfcEventTypes.difference(declared)} that Kotlin does '
          'not emit. Whichever side changed, change the other.',
    );
  });
}
