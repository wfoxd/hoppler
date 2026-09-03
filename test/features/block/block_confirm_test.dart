import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/block/block_confirm.dart';

/// Opens the dialog and reports what it answered, the way a caller does.
Future<bool?> ask(WidgetTester tester, {String name = 'sam'}) async {
  bool? answer;
  await tester.pumpWidget(
    MaterialApp(
      home: Builder(
        builder: (context) => ElevatedButton(
          onPressed: () async => answer = await showBlockConfirm(context, name),
          child: const Text('go'),
        ),
      ),
    ),
  );
  await tester.tap(find.text('go'));
  await tester.pumpAndSettle();
  return answer;
}

void main() {
  testWidgets('it says what cannot be undone, not just "are you sure"', (
    tester,
  ) async {
    await ask(tester);
    expect(find.text('Block sam?'), findsOneWidget);
    // The whole reason the dialog exists: unblocking does not restore the
    // pairing, and somebody agreeing to this needs to have been told.
    expect(
      find.textContaining('undoes your pairing'),
      findsOneWidget,
      reason: 'the irreversible part was not stated',
    );
    expect(find.textContaining("pair again"), findsOneWidget);
    // And that it is silent — R0-F10 turns on the blocked party learning
    // nothing, which is a promise to the person blocking too.
    expect(find.textContaining("won't be told"), findsOneWidget);
  });

  testWidgets('cancelling is a no, and blocking is a yes', (tester) async {
    for (final (button, expected) in [('Cancel', false), ('Block', true)]) {
      bool? answer;
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => ElevatedButton(
              onPressed: () async =>
                  answer = await showBlockConfirm(context, 'sam'),
              child: const Text('go'),
            ),
          ),
        ),
      );
      await tester.tap(find.text('go'));
      await tester.pumpAndSettle();
      await tester.tap(find.text(button));
      await tester.pumpAndSettle();
      expect(answer, expected, reason: '$button answered wrongly');
    }
  });

  /// The trap `showBlockConfirm` exists to close.
  ///
  /// `showDialog` completes with `null` when somebody taps outside or presses
  /// back. A call site reading that as anything but "no" blocks the person who
  /// was trying to get *out* of the dialog.
  testWidgets('dismissing without choosing is a no', (tester) async {
    bool? answer;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => ElevatedButton(
            onPressed: () async =>
                answer = await showBlockConfirm(context, 'sam'),
            child: const Text('go'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('go'));
    await tester.pumpAndSettle();
    // Outside the dialog, which is how a barrier dismissal happens.
    await tester.tapAt(const Offset(10, 10));
    await tester.pumpAndSettle();
    expect(
      answer,
      isFalse,
      reason: 'a dismissed dialog must never read as consent to block',
    );
  });

  testWidgets('a device with no name is still nameable', (tester) async {
    await ask(tester, name: '');
    expect(find.text('Block this device?'), findsOneWidget);
  });
}
