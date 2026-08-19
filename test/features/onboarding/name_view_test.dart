import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/onboarding/name_view.dart';
import 'package:hoppler/src/rust/api/types.dart';

/// The first screen a new device shows (R0-F1).
///
/// Worth testing more carefully than its size suggests: it is the only thing
/// standing between a fresh install and being discoverable as "Me", which is
/// the state that made the pairing screen read "Pairing with Me".
void main() {
  /// The first two of the core's eight. Two is enough for every question here
  /// — that a pick registers, and that it is the pick that gets sent — and the
  /// real list is the core's to hold.
  const palette = [
    PersonaColourDto(name: 'blue', value: 0x4488ff),
    PersonaColourDto(name: 'coral', value: 0xe05c4a),
  ];

  Future<void> pump(
    WidgetTester tester,
    Future<void> Function(String, int) onChosen, {
    int colour = 0x4488ff,
  }) => tester.pumpWidget(
    MaterialApp(
      home: NameView(
        colour: Color(0xFF000000 | colour),
        palette: palette,
        onChosen: onChosen,
      ),
    ),
  );

  /// R0-F1 asks for a colour as well as a name. The core draws one, and until
  /// this screen offered a choice, that draw was final — a device wearing a
  /// colour nobody picked, in a list where the colour is how you find yourself.
  testWidgets('the drawn colour starts selected', (tester) async {
    await pump(tester, (_, _) async {}, colour: 0xe05c4a);

    // `isSemantics`, not `matchesSemantics`: the latter asserts the complete
    // set, so it fails on the tap and focus actions an InkWell adds — which is
    // the widget working, not the property under test.
    expect(
      tester.getSemantics(find.bySemanticsLabel('coral')),
      isSemantics(label: 'coral', isButton: true, isSelected: true),
      reason: 'the colour the core drew is not the one shown as chosen',
    );
    // And the other one is not, which is the half that catches "everything is
    // selected" — a state that would satisfy the assertion above.
    expect(
      tester.getSemantics(find.bySemanticsLabel('blue')),
      isSemantics(label: 'blue', isSelected: false),
    );
  });

  /// The point of the row. Not just that the avatar repaints — that the value
  /// which reaches the core is the one that was tapped, which is the only part
  /// that outlives the screen.
  testWidgets('the colour that is tapped is the colour that is stored', (
    tester,
  ) async {
    int? stored;
    await pump(tester, (_, colour) async => stored = colour, colour: 0x4488ff);

    await tester.tap(find.bySemanticsLabel('coral'));
    await tester.pump();
    await tester.enterText(find.byType(TextField), 'Wren');
    await tester.pump();
    await tester.tap(find.widgetWithText(FilledButton, 'Continue'));
    await tester.pump();

    expect(stored, 0xe05c4a);
  });

  /// A ring in the accent colour is the obvious indicator and the wrong one on
  /// its own: this is a row of colours, and the people a clear indicator helps
  /// most are the ones who cannot separate two of them. The tick is a shape, so
  /// it works without colour perception at all.
  testWidgets('the selected swatch is marked by a shape, not only a colour', (
    tester,
  ) async {
    await pump(tester, (_, _) async {}, colour: 0x4488ff);
    expect(find.byIcon(Icons.check), findsOneWidget);

    await tester.tap(find.bySemanticsLabel('coral'));
    await tester.pump();
    expect(
      find.byIcon(Icons.check),
      findsOneWidget,
      reason: 'exactly one swatch is chosen, so exactly one tick',
    );
  });

  /// The avatar is the only thing that says a tap did anything before the
  /// screen is left. Nothing else on this screen changes size or position, so
  /// a swatch that registers internally and does not repaint reads as a dead
  /// control.
  testWidgets('the avatar follows the selection', (tester) async {
    await pump(tester, (_, _) async {}, colour: 0x4488ff);
    Color? avatar() => tester
        .widget<CircleAvatar>(find.byType(CircleAvatar))
        .backgroundColor;

    expect(avatar(), const Color(0xFF4488FF));
    await tester.tap(find.bySemanticsLabel('coral'));
    await tester.pump();
    expect(avatar(), const Color(0xFFE05C4A));
  });

  /// Every swatch is named, or the row is eight identical circles to a screen
  /// reader and unusable to the people it exists to serve.
  testWidgets('every swatch carries its name', (tester) async {
    await pump(tester, (_, _) async {});
    for (final name in ['blue', 'coral']) {
      expect(find.bySemanticsLabel(name), findsOneWidget);
    }
  });

  testWidgets('a name cannot be submitted until one is typed', (tester) async {
    var called = 0;
    await pump(tester, (_, _) async => called++);

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
    await pump(tester, (_, _) async => called++);
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
    await pump(tester, (name, _) async => chosen = name);
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
    await pump(tester, (_, _) async => throw StateError('disk full'));
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
  ///
  /// The second tap is the test. Written without it first, and review caught
  /// that: it was named for double-tap protection and would have passed just as
  /// happily against a build that saved twice.
  testWidgets('a second tap while saving does nothing', (tester) async {
    var called = 0;
    final gate = Completer<void>();
    await pump(tester, (_, _) async {
      called++;
      await gate.future;
    });

    await tester.enterText(find.byType(TextField), 'Wren');
    await tester.pump();

    await tester.tap(find.widgetWithText(FilledButton, 'Continue'));
    await tester.pump();
    final saving = find.widgetWithText(FilledButton, 'Saving…');
    expect(saving, findsOneWidget, reason: 'the button did not report saving');
    expect(tester.widget<FilledButton>(saving).onPressed, isNull);

    // Tapped again mid-save, which is what a person does when nothing seems to
    // be happening.
    //
    // What this proves is the behaviour, not the mechanism: the second tap does
    // nothing because the button is disabled, and removing the `_saving` guard
    // inside `_submit` still passes. That guard is a second lock behind this
    // one, which is why its mutant survives — said here as well as in the
    // widget so neither reads as an untested hole.
    await tester.tap(saving, warnIfMissed: false);
    await tester.pump();
    expect(called, 1, reason: 'a second tap started a second save');

    gate.complete();
    await tester.pumpAndSettle();
    expect(called, 1);
  });

  /// The limit is a byte count in the core, and `maxLength` counts code units.
  /// Sixty-four emoji are 64 to Flutter and 256 to Rust, which would truncate
  /// them after submission — the exact failure the widget's doc says it stops.
  testWidgets('the limit is counted in bytes, not characters', (tester) async {
    String? chosen;
    await pump(tester, (name, _) async => chosen = name);

    // Sixteen four-byte characters is 64 bytes: right at the limit.
    final full = '🐇' * 16;
    await tester.enterText(find.byType(TextField), full);
    await tester.pump();
    expect(find.text('64/64'), findsOneWidget);

    // One more must not be accepted, or the core would cut it back.
    await tester.enterText(find.byType(TextField), '$full🐇');
    await tester.pump();
    expect(
      tester.widget<TextField>(find.byType(TextField)).controller?.text,
      full,
      reason: 'a 68-byte name was accepted and would come back shortened',
    );

    await tester.tap(find.widgetWithText(FilledButton, 'Continue'));
    await tester.pump();
    expect(utf8.encode(chosen!).length, 64);
  });
}
