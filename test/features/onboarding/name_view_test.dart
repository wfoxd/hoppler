import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/onboarding/name_view.dart';

/// The first screen a new device shows (R0-F1).
///
/// Worth testing more carefully than its size suggests: it is the only thing
/// standing between a fresh install and being discoverable as "Me", which is
/// the state that made the pairing screen read "Pairing with Me".
void main() {
  Future<void> pump(
    WidgetTester tester,
    Future<void> Function(String) onChosen,
  ) => tester.pumpWidget(
    MaterialApp(
      home: NameView(colour: const Color(0xFF4488FF), onChosen: onChosen),
    ),
  );

  testWidgets('a name cannot be submitted until one is typed', (tester) async {
    var called = 0;
    await pump(tester, (_) async => called++);

    final button = find.widgetWithText(FilledButton, 'Continue');
    expect(
      tester.widget<FilledButton>(button).onPressed,
      isNull,
      reason: 'an empty name would store the placeholder as a real choice',
    );

    await tester.enterText(find.byType(TextField), 'Wren');
    await tester.pump();
    expect(tester.widget<FilledButton>(button).onPressed, isNotNull);

    await tester.tap(button);
    await tester.pump();
    expect(called, 1);
  });

  testWidgets('whitespace is not a name', (tester) async {
    var called = 0;
    await pump(tester, (_) async => called++);
    await tester.enterText(find.byType(TextField), '   ');
    await tester.pump();

    expect(
      tester
          .widget<FilledButton>(find.widgetWithText(FilledButton, 'Continue'))
          .onPressed,
      isNull,
      reason: 'a name of spaces reads as no name at all on every other screen',
    );
    expect(called, 0);
  });

  testWidgets('the name is trimmed before it is chosen', (tester) async {
    String? chosen;
    await pump(tester, (name) async => chosen = name);
    await tester.enterText(find.byType(TextField), '  Wren  ');
    await tester.pump();
    await tester.tap(find.widgetWithText(FilledButton, 'Continue'));
    await tester.pump();

    expect(chosen, 'Wren');
  });

  /// A name that could not be stored must not look like one that was.
  /// Continuing would mean discovering under a name the next launch will not
  /// have — the exact failure the persistence work went in to stop.
  testWidgets('a name that cannot be saved keeps the screen and says so', (
    tester,
  ) async {
    await pump(tester, (_) async => throw StateError('disk full'));
    await tester.enterText(find.byType(TextField), 'Wren');
    await tester.pump();
    await tester.tap(find.widgetWithText(FilledButton, 'Continue'));
    await tester.pump();

    expect(find.byType(NameView), findsOneWidget);
    expect(find.textContaining('could not be saved'), findsOneWidget);
    expect(
      tester
          .widget<FilledButton>(find.widgetWithText(FilledButton, 'Continue'))
          .onPressed,
      isNotNull,
      reason:
          'a failed save must leave the button usable, or the only way on '
          'is to restart the app',
    );
  });

  /// Double-tapping a slow save must not store the name twice — the second
  /// call would bump the persona version again for no reason, and every peer
  /// would be told about a change that did not happen.
  testWidgets('a second tap while saving does nothing', (tester) async {
    var called = 0;
    final gate = Completer<void>();
    await pump(tester, (_) async {
      called++;
      await gate.future;
    });

    await tester.enterText(find.byType(TextField), 'Wren');
    await tester.pump();
    final button = find.widgetWithText(FilledButton, 'Saving…');

    await tester.tap(find.widgetWithText(FilledButton, 'Continue'));
    await tester.pump();
    expect(button, findsOneWidget, reason: 'the button did not report saving');
    expect(tester.widget<FilledButton>(button).onPressed, isNull);

    gate.complete();
    await tester.pumpAndSettle();
    expect(called, 1);
  });

  testWidgets('the name is capped at what the core will store', (tester) async {
    await pump(tester, (_) async {});
    expect(
      tester.widget<TextField>(find.byType(TextField)).maxLength,
      NameView.maxNameBytes,
      reason:
          'a longer name is truncated by the core, so a person would be '
          'given back a different name than the one they typed',
    );
  });
}
