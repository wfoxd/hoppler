import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/pairing/pairing_surface.dart';

/// The composition, which is where the two-phone defect actually lived.
///
/// Both pairing views passed their own tests. What failed was which one was on
/// screen — the code was a modal route and the colours were drawn in the body
/// underneath it, so the person showing a code reached the SAS and never saw
/// it. A race between a route and a piece of state has nowhere to put a test;
/// an ordered expression does, and this is it.
void main() {
  Widget host({Widget? sas, Widget? code, Widget? scanner}) => MaterialApp(
    home: Scaffold(
      body: PairingSurface(
        sas: sas,
        code: code,
        scanner: scanner,
        nearby: const Text('nearby'),
      ),
    ),
  );

  testWidgets('with nothing happening, it shows who is nearby', (t) async {
    await t.pumpWidget(host());
    expect(find.text('nearby'), findsOneWidget);
  });

  testWidgets('the colours beat this device showing its own code', (t) async {
    // The exact defect, as a test. The shower reaches the colours while its own
    // QR is still up; if the code wins, that person can never confirm, and the
    // ceremony cannot complete for *either* of them.
    await t.pumpWidget(host(sas: const Text('sas'), code: const Text('code')));
    expect(find.text('sas'), findsOneWidget);
    expect(find.text('code'), findsNothing);
  });

  testWidgets('the colours beat the camera', (t) async {
    // The scanner's side of the same moment: a camera still running over the
    // colours would leave the person comparing swatches through a viewfinder.
    await t.pumpWidget(
      host(sas: const Text('sas'), scanner: const Text('scanner')),
    );
    expect(find.text('sas'), findsOneWidget);
    expect(find.text('scanner'), findsNothing);
  });

  testWidgets('the colours beat everything at once', (t) async {
    await t.pumpWidget(
      host(
        sas: const Text('sas'),
        code: const Text('code'),
        scanner: const Text('scanner'),
      ),
    );
    expect(find.text('sas'), findsOneWidget);
    expect(find.text('code'), findsNothing);
    expect(find.text('scanner'), findsNothing);
    expect(find.text('nearby'), findsNothing);
  });

  testWidgets('a code shows when nothing is happening', (t) async {
    await t.pumpWidget(host(code: const Text('code')));
    expect(find.text('code'), findsOneWidget);
    expect(find.text('nearby'), findsNothing);
  });

  testWidgets('the camera shows when nothing is happening', (t) async {
    await t.pumpWidget(host(scanner: const Text('scanner')));
    expect(find.text('scanner'), findsOneWidget);
  });

  testWidgets('exactly one surface is ever on screen', (t) async {
    // Not a tidiness rule. Two pairing surfaces at once is how the original bug
    // presented — one of them visible, the other one live underneath.
    for (final surfaces in [
      (sas: null, code: null, scanner: null),
      (sas: const Text('sas'), code: null, scanner: null),
      (sas: null, code: const Text('code'), scanner: null),
      (sas: null, code: null, scanner: const Text('scanner')),
      (sas: const Text('sas'), code: const Text('code'), scanner: null),
      (
        sas: const Text('sas'),
        code: const Text('code'),
        scanner: const Text('scanner'),
      ),
    ]) {
      await t.pumpWidget(
        host(sas: surfaces.sas, code: surfaces.code, scanner: surfaces.scanner),
      );
      final showing = ['sas', 'code', 'scanner', 'nearby']
          .where((label) => find.text(label).evaluate().isNotEmpty)
          .toList();
      expect(showing, hasLength(1), reason: 'showing $showing at once');
    }
  });
}
