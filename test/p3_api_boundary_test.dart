import 'dart:io';

import 'package:analyzer/dart/analysis/features.dart';
import 'package:analyzer/dart/analysis/utilities.dart';
import 'package:analyzer/dart/ast/ast.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:path/path.dart' as p;

/// P3 guard (G-3): app code consumes only the Core API surface — the generated
/// bindings under `lib/src/rust/api/`. Reaching around it into the frb plumbing
/// or other generated internals is the P3 principle failing quietly, so it
/// fails the build here instead.
///
/// The one allowed exception is `lib/main.dart` importing `frb_generated.dart`
/// to call `RustLib.init()` (the bridge has no other entry point for that).
///
/// This uses the Dart AST — not a regex — so it catches `export`, conditional
/// imports (`if (dart.library.io) '...'`, whose branch ships on native builds),
/// relative imports, and `..`-normalized paths. Regex versions miss all of
/// these.
void main() {
  final rustRoot = p.join('lib', 'src', 'rust');
  final apiRoot = p.join('lib', 'src', 'rust', 'api');
  final genRoot = p.join('lib', 'src', 'gen');
  final frbInit = p.join('lib', 'src', 'rust', 'frb_generated.dart');
  final mainDart = p.join('lib', 'main.dart');

  bool underRust(String path) => p.isWithin(rustRoot, path);
  bool underApi(String path) => p.isWithin(apiRoot, path);

  // Resolve an import/export URI to a normalized repo-relative path (or null if
  // it doesn't point into our own lib/).
  String? resolve(String fromFile, String uri) {
    const pkg = 'package:hoppler/';
    if (uri.startsWith(pkg)) {
      return p.normalize(p.join('lib', uri.substring(pkg.length)));
    }
    if (uri.contains(':')) return null; // other package: / dart: URIs
    return p.normalize(p.join(p.dirname(fromFile), uri)); // relative
  }

  test('app imports only the core API (src/rust/api/) — P3/G-3', () {
    final offenders = <String>[];

    for (final entity in Directory('lib').listSync(recursive: true)) {
      if (entity is! File || !entity.path.endsWith('.dart')) continue;
      final relPath = p.relative(entity.path);
      // Generated code is not hand-written app code — skip it.
      if (p.isWithin(rustRoot, relPath) || p.isWithin(genRoot, relPath)) continue;

      final isMain = p.equals(relPath, mainDart);
      final unit = parseFile(
        path: entity.absolute.path,
        featureSet: FeatureSet.latestLanguageVersion(),
      ).unit;

      for (final directive in unit.directives) {
        if (directive is! NamespaceDirective) continue; // import or export
        // The default URI plus every conditional-configuration URI.
        final uris = <String>[
          ?directive.uri.stringValue,
          for (final c in directive.configurations) ?c.uri.stringValue,
        ];
        for (final uri in uris) {
          final target = resolve(relPath, uri);
          if (target == null || !underRust(target)) continue;
          final ok = underApi(target) ||
              (isMain && directive is ImportDirective && p.equals(target, frbInit));
          if (!ok) offenders.add('$relPath -> $uri');
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
