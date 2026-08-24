import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/threads/thread_view.dart';

void main() {
  Future<List<String>> pump(
    WidgetTester tester, {
    List<ThreadLine> lines = const [],
    bool canSend = true,
  }) async {
    final sent = <String>[];
    await tester.pumpWidget(
      MaterialApp(
        home: ThreadView(
          title: 'Wren',
          lines: lines,
          canSend: canSend,
          cannotSendReason: 'This contact is gone.',
          onSend: sent.add,
        ),
      ),
    );
    return sent;
  }

  testWidgets('both sides of a conversation are shown', (tester) async {
    await pump(
      tester,
      lines: const [
        ThreadLine(text: 'are you there?', outgoing: false),
        ThreadLine(text: 'back in a bit', outgoing: true),
      ],
    );

    expect(find.text('are you there?'), findsOneWidget);
    expect(find.text('back in a bit'), findsOneWidget);
  });

  testWidgets('typing and sending hands the text over and clears the box', (
    tester,
  ) async {
    final sent = await pump(tester);

    await tester.enterText(find.byType(TextField), 'hello');
    await tester.tap(find.byTooltip('Send'));
    await tester.pump();

    expect(sent, ['hello']);
    expect(
      tester.widget<TextField>(find.byType(TextField)).controller?.text,
      '',
      reason: 'the box kept the message after sending it',
    );
  });

  // An empty send is the easiest thing to do by accident — a stray tap on the
  // button — and an empty line in somebody's conversation cannot be taken back.
  testWidgets('an empty message is not sent', (tester) async {
    final sent = await pump(tester);

    await tester.enterText(find.byType(TextField), '   ');
    await tester.tap(find.byTooltip('Send'));
    await tester.pump();

    expect(sent, isEmpty);
  });

  // Being out of range is *not* a reason to refuse: R0-F5 keeps the message and
  // delivers it at the next encounter. This is the other case — a conversation
  // that cannot be written to at all — and it has to say which.
  testWidgets('when sending is impossible, it says why', (tester) async {
    await pump(tester, canSend: false);

    final button = tester.widget<IconButton>(find.byType(IconButton).last);
    expect(button.onPressed, isNull);
    expect(button.tooltip, 'This contact is gone.');
    expect(tester.widget<TextField>(find.byType(TextField)).enabled, isFalse);
  });
}
