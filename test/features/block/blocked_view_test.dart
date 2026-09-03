import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/block/blocked_view.dart';

Widget wrap(List<BlockedPerson> people, void Function(BlockedPerson) onUnblock) =>
    MaterialApp(home: BlockedView(people: people, onUnblock: onUnblock));

void main() {
  testWidgets('an empty list says so without sounding broken', (tester) async {
    await tester.pumpWidget(wrap(const [], (_) {}));
    expect(find.text(BlockedView.emptyHint), findsOneWidget);
  });

  testWidgets('each person gets a row and an Unblock', (tester) async {
    final unblocked = <int>[];
    await tester.pumpWidget(
      wrap(const [
        BlockedPerson(contactId: 1, name: 'sam', colour: 0x00ff00),
        BlockedPerson(contactId: 2, name: 'mal', colour: 0x0000ff),
      ], (p) => unblocked.add(p.contactId)),
    );
    expect(find.text('sam'), findsOneWidget);
    expect(find.text('mal'), findsOneWidget);

    // The second one, so a handler wired to the wrong row is visible.
    await tester.tap(
      find.descendant(
        of: find.widgetWithText(ListTile, 'mal'),
        matching: find.text('Unblock'),
      ),
    );
    await tester.pumpAndSettle();
    expect(unblocked, [2], reason: 'Unblock lifted the wrong person');
  });

  testWidgets('somebody whose name we never learned is still listed', (
    tester,
  ) async {
    await tester.pumpWidget(
      wrap(const [
        BlockedPerson(contactId: 7, name: '', colour: 0),
      ], (_) {}),
    );
    // Blank would read as a rendering fault, and the row still has to be
    // unblockable.
    expect(find.text('Unknown device'), findsOneWidget);
    expect(find.text('Unblock'), findsOneWidget);
  });
}
