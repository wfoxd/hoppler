import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/src/ble/ble_channel.dart';

/// The one test that spans the platform channel.
///
/// The BLE event protocol is a set of strings emitted by Kotlin and matched by
/// Dart, and until now **nothing checked that the two halves agreed**. Each side
/// was tested against its own literals: `ble_channel_test.dart` decodes maps it
/// writes itself, and the Kotlin side had no test of the vocabulary at all. So
/// renaming `pipeFailed` in `BleAdapter.kt` would have left every test in both
/// languages green while pings stopped reporting failure on real devices.
///
/// This reads the Kotlin source. That is unusual, and it is the only mechanism
/// available: the two sides share no generated type, and adding codegen for
/// eight strings would cost more than it saves. If the file moves or the
/// declarations change shape, this fails loudly — which is the correct
/// direction to fail in, because the alternative is passing while the protocol
/// is broken.
const relativeAdapterPath =
    'android/app/src/main/kotlin/org/hoppler/hoppler/ble/BleAdapter.kt';

void main() {
  /// Found by walking up from the current directory rather than assuming it.
  ///
  /// A hard-coded relative path silently depends on the working directory being
  /// the package root, which `flutter test` gives it and an IDE or a run from a
  /// subdirectory may not. That failure would look exactly like the adapter
  /// having moved, which is the one thing this test is supposed to report
  /// accurately.
  File? findAdapter() {
    for (var dir = Directory.current; ; dir = dir.parent) {
      final candidate = File('${dir.path}/$relativeAdapterPath');
      if (candidate.existsSync()) return candidate;
      if (dir.parent.path == dir.path) return null;
    }
  }

  test('Kotlin emits exactly the event types Dart decodes', () {
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
          'found no EVENT_ constants in BleAdapter.kt. They are declared as '
          'plain `const val NAME = "value"` lines so this regex can see them; '
          'if that changed, this test can no longer couple the languages and '
          'is silently worthless — fix the parse, do not relax the assertion',
    );

    expect(
      declared,
      equals(bleEventTypes),
      reason:
          'the two halves of the platform channel have drifted. Kotlin emits '
          '${declared.difference(bleEventTypes)} that Dart does not decode, and '
          'Dart expects ${bleEventTypes.difference(declared)} that Kotlin does '
          'not emit. Whichever side changed, change the other.',
    );
  });
}
