import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/nearby/nearby_view.dart';

Widget _wrap(Widget child) => MaterialApp(home: Scaffold(body: child));

Widget _tile(String d) => ListTile(key: ValueKey(d), title: Text(d));

void main() {
  _radioReasonGroup();

  testWidgets('an unusable radio says why instead of claiming the room is empty', (
    tester,
  ) async {
    await tester.pumpWidget(
      _wrap(
        const NearbyView<String>(
          radioReason: 'Bluetooth is off',
          devices: [],
          tile: _tile,
        ),
      ),
    );

    expect(find.text('Bluetooth is off'), findsOneWidget);
    // The R0-F2 line. An empty list is equally what being alone in a field
    // looks like, so saying it while the radio is off is a false claim about
    // the world — which is the whole reason the reason exists.
    expect(find.text(NearbyView.emptyText), findsNothing);
  });

  testWidgets('the reason wins even while stale devices are still listed', (
    tester,
  ) async {
    await tester.pumpWidget(
      _wrap(
        const NearbyView<String>(
          radioReason: 'Bluetooth is off',
          devices: ['alice', 'bob'],
          tile: _tile,
        ),
      ),
    );

    // The radio can go down before the list is cleared. Showing peers we
    // cannot possibly reach is the same lie wearing a fuller list.
    expect(find.text('Bluetooth is off'), findsOneWidget);
    expect(find.text('alice'), findsNothing);
    expect(find.text('bob'), findsNothing);
  });

  testWidgets('a working radio with nobody about still says so', (tester) async {
    await tester.pumpWidget(
      _wrap(const NearbyView<String>(radioReason: null, devices: [], tile: _tile)),
    );

    expect(find.text(NearbyView.emptyText), findsOneWidget);
  });

  testWidgets('a working radio lists what it found', (tester) async {
    await tester.pumpWidget(
      _wrap(
        const NearbyView<String>(
          radioReason: null,
          devices: ['alice', 'bob'],
          tile: _tile,
        ),
      ),
    );

    expect(find.text('alice'), findsOneWidget);
    expect(find.text('bob'), findsOneWidget);
    expect(find.text(NearbyView.emptyText), findsNothing);
  });
}

void _radioReasonGroup() {
  group('radioReasonFrom', () {
    test('an unavailable radio with nothing to say is still unavailable', () {
      // The case that was written the wrong way round in Rust in this same
      // change: deciding from the reason reads this as a working radio.
      expect(radioReasonFrom(available: false, reason: null), isNotNull);
    });

    test('a recovery clears the sentence even carrying a stale reason', () {
      expect(radioReasonFrom(available: true, reason: 'Bluetooth is off'), isNull);
    });

    test('a reason is used when there is one', () {
      expect(
        radioReasonFrom(available: false, reason: 'Bluetooth is off'),
        'Bluetooth is off',
      );
    });
  });
}
