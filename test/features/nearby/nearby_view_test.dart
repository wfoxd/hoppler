import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/nearby/nearby_view.dart';

Widget _wrap(Widget child) => MaterialApp(home: Scaffold(body: child));

Widget _tile(String d) => ListTile(key: ValueKey(d), title: Text(d));

void main() {
  _radioReasonGroup();

  testWidgets(
    'an unusable radio says why instead of claiming the room is empty',
    (tester) async {
      await tester.pumpWidget(
        _wrap(
          const NearbyView<String>(
            discoveryOn: true,
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
      expect(find.text(NearbyView.emptyOnText), findsNothing);
    },
  );

  testWidgets('the reason wins even while stale devices are still listed', (
    tester,
  ) async {
    await tester.pumpWidget(
      _wrap(
        const NearbyView<String>(
          discoveryOn: true,
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

  testWidgets('a working radio with nobody about still says so', (
    tester,
  ) async {
    await tester.pumpWidget(
      _wrap(
        const NearbyView<String>(
          radioReason: null,
          discoveryOn: true,
          devices: [],
          tile: _tile,
        ),
      ),
    );

    expect(find.text(NearbyView.emptyOnText), findsOneWidget);
  });

  testWidgets('a working radio lists what it found', (tester) async {
    await tester.pumpWidget(
      _wrap(
        const NearbyView<String>(
          discoveryOn: true,
          radioReason: null,
          devices: ['alice', 'bob'],
          tile: _tile,
        ),
      ),
    );

    expect(find.text('alice'), findsOneWidget);
    expect(find.text('bob'), findsOneWidget);
    expect(find.text(NearbyView.emptyOnText), findsNothing);
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
      expect(
        radioReasonFrom(available: true, reason: 'Bluetooth is off'),
        isNull,
      );
    });

    test('a reason is used when there is one', () {
      expect(
        radioReasonFrom(available: false, reason: 'Bluetooth is off'),
        'Bluetooth is off',
      );
    });

    /// A refusal used to put one line in the activity log and leave the nearby
    /// area saying "No one nearby" — R0-F2's false claim about the room,
    /// reached by the one route nothing was watching.
    test('a refused permission is said where the empty list would be', () {
      final said = radioReasonFrom(available: false, permissionDenied: true);
      expect(said, isNotNull);
      expect(
        said,
        contains('permission'),
        reason: 'the sentence has to name the thing the person can fix',
      );
    });

    /// The refusal is the cause: the radio has nothing to report because it was
    /// never allowed to look. So it outranks the radio's own words, and it
    /// outranks them even when the radio calls itself available — a permission
    /// we do not hold makes that report meaningless.
    test('a refusal outranks what the radio says about itself', () {
      expect(
        radioReasonFrom(
          available: false,
          reason: 'Bluetooth is off',
          permissionDenied: true,
        ),
        contains('permission'),
      );
      expect(
        radioReasonFrom(available: true, permissionDenied: true),
        contains('permission'),
        reason: 'an available radio we may not use is not an available radio',
      );
    });

    /// And it stops being said the moment it stops being true, or the screen
    /// keeps blaming a permission that has since been granted in Settings.
    test('granting it clears the sentence', () {
      expect(radioReasonFrom(available: true, permissionDenied: false), isNull);
    });
  });

  /// The empty list has two meanings and needs two sentences.
  ///
  /// It said "No one nearby. Turn on Discovery." in both, so a user whose
  /// Discovery was already on was told to turn on Discovery, while the switch
  /// above it read "Visible". Found on a real phone rather than in review.
  /// A message that contradicts the control next to it teaches people to stop
  /// reading the messages — expensive for an app whose whole job in an empty
  /// room is to explain itself.
  testWidgets('an empty room does not tell you to turn on what is already on', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: NearbyView<String>(
          radioReason: null,
          discoveryOn: true,
          devices: const [],
          tile: (d) => Text(d),
        ),
      ),
    );
    expect(find.text(NearbyView.emptyOnText), findsOneWidget);
    expect(find.text(NearbyView.emptyOffText), findsNothing);
    expect(
      NearbyView.emptyOnText.toLowerCase().contains('turn on'),
      isFalse,
      reason: 'the on-state wording still instructs the user to turn it on',
    );
  });

  testWidgets('with Discovery off the way out is still named', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: NearbyView<String>(
          radioReason: null,
          discoveryOn: false,
          devices: const [],
          tile: (d) => Text(d),
        ),
      ),
    );
    expect(find.text(NearbyView.emptyOffText), findsOneWidget);
    expect(find.text(NearbyView.emptyOnText), findsNothing);
  });
}
