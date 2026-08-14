import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/pairing/pairing_code_view.dart';
import 'package:qr_flutter/qr_flutter.dart';

void main() {
  Widget host({
    bool canScan = true,
    bool canTap = false,
    VoidCallback? onDone,
  }) => MaterialApp(
    home: Scaffold(
      body: PairingCodeView(
        code: 'HOPPLER://PAIR/MZXW6YTBOI',
        canScan: canScan,
        canTap: canTap,
        onDone: onDone ?? () {},
      ),
    ),
  );

  testWidgets('puts a code on screen', (t) async {
    await t.pumpWidget(host());
    expect(find.byType(QrImageView), findsOneWidget);
  });

  // Not tested: that the payload reaches the QR unaltered. `QrImageView` keeps
  // its data private, so nothing here can observe it, and the obvious
  // workaround — reading back the field this widget was handed — would assert
  // the test's own setup and nothing else.
  //
  // What the property actually rests on is one line: `code` is a `final String`
  // passed straight to `QrImageView(data: code)`, with nowhere to be trimmed or
  // re-cased. The bytes themselves are pinned where they can be seen, by
  // `pairing::invite::tests::golden_uri` on the Rust side.

  testWidgets('keeps a white field behind the code in any theme', (t) async {
    await t.pumpWidget(
      MaterialApp(
        theme: ThemeData.dark(),
        home: Scaffold(
          body: PairingCodeView(code: 'HOPPLER://PAIR/AA', onDone: () {}),
        ),
      ),
    );
    // A QR reader needs the contrast the spec assumes. Dark modules on a dark
    // surface is a code that will not scan, which looks to both people like
    // the ceremony failing rather than the theme doing it.
    final qr = t.widget<QrImageView>(find.byType(QrImageView));
    expect(qr.backgroundColor, Colors.white);
  });

  testWidgets('a device that cannot scan says so instead of instructing', (
    t,
  ) async {
    await t.pumpWidget(host(canScan: false));
    // R0-N6's rule — state the reach honestly — applied to a platform gap
    // rather than a radio one. Telling a desktop user to scan something is an
    // instruction they cannot follow, and they will look for the button.
    expect(find.text(PairingCodeView.noCameraHere), findsOneWidget);
    expect(find.text(PairingCodeView.instruction), findsNothing);
  });

  testWidgets('a device that can scan gets the ordinary instruction', (
    t,
  ) async {
    await t.pumpWidget(host());
    expect(find.text(PairingCodeView.instruction), findsOneWidget);
    expect(find.text(PairingCodeView.noCameraHere), findsNothing);
  });

  testWidgets('dismissing reaches the caller, which is what stops the code', (
    t,
  ) async {
    var done = false;
    await t.pumpWidget(host(onDone: () => done = true));
    await t.tap(find.text('Done'));
    // The caller takes the code off the air on the way out. A code left showing
    // keeps a Layer-2 key paired with the rung id this device advertises under,
    // which is the link R0-F2's rotation exists to break.
    expect(done, isTrue);
  });
  testWidgets('a phone that can tap is told so, beside the code', (t) async {
    await t.pumpWidget(host(canTap: true));
    // Beside, not instead of. The QR is the path that always works, and plenty
    // of phones have no NFC at all — a screen offering only the tap would
    // strand them with no way to finish.
    expect(find.text(PairingCodeView.tapInstruction), findsOneWidget);
    expect(find.text(PairingCodeView.instruction), findsOneWidget);
  });

  testWidgets('a phone that cannot tap is not told to', (t) async {
    await t.pumpWidget(host());
    expect(find.text(PairingCodeView.tapInstruction), findsNothing);
  });
}
