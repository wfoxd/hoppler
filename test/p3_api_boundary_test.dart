import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

/// P3 guard (G-3): app code consumes only the Core API surface — the generated
/// bindings under `lib/src/rust/api/`. Reaching around it into the frb plumbing
/// or other generated internals is the P3 principle failing quietly, so it
/// fails the build here instead.
///
/// The one allowed exception is `main.dart` importing `frb_generated.dart` to
/// call `RustLib.init()` (the bridge has no other entry point for that).
void main() {
  test('app imports only the core API (src/rust/api/)', () {
    final importRe = RegExp(r'''import\s+['"]([^'"]+)['"]''');
    final offenders = <String>[];

    for (final entity in Directory('lib').listSync(recursive: true)) {
      if (entity is! File || !entity.path.endsWith('.dart')) continue;
      // Skip generated code itself (only hand-written app code is governed).
      if (entity.path.contains('/src/rust/') || entity.path.contains('/src/gen/')) {
        continue;
      }
      final isMain = entity.path.endsWith('lib/main.dart');

      for (final line in entity.readAsLinesSync()) {
        final uri = importRe.firstMatch(line)?.group(1);
        if (uri == null || !uri.contains('src/rust/')) continue;

        final viaApi = uri.contains('src/rust/api/');
        final initException = isMain && uri.endsWith('src/rust/frb_generated.dart');
        if (!viaApi && !initException) {
          offenders.add('${entity.path}: $uri');
        }
      }
    }

    expect(
      offenders,
      isEmpty,
      reason: 'app code must import only the core API (src/rust/api/):\n'
          '${offenders.join('\n')}',
    );
  });
}
