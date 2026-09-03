import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/threads/thread_view.dart';
import 'package:hoppler/src/rust/api/types.dart';

void main() {
  Future<List<String>> pump(
    WidgetTester tester, {
    List<ThreadLine> lines = const [],
    bool canSend = true,
    VoidCallback? onBlock,
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
          onBlock: onBlock,
        ),
      ),
    );
    return sent;
  }

  /// Block is reachable from inside a conversation (R0-F10), and behind an
  /// overflow rather than out on the bar — it is destructive, irreversible, and
  /// the app bar of a chat is where a thumb rests.
  testWidgets('a conversation offers Block, behind the overflow', (
    tester,
  ) async {
    final blocked = <int>[];
    await pump(tester, onBlock: () => blocked.add(1));

    // Nothing on the bar itself until it is opened.
    expect(find.text('Block'), findsNothing);

    await tester.tap(find.byType(PopupMenuButton<void>));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Block'));
    await tester.pumpAndSettle();

    expect(blocked, hasLength(1));
  });

  /// A conversation with nobody identifiable has nothing to block, and the menu
  /// goes rather than sitting there disabled — a tap that quietly fails is the
  /// worst outcome on the one screen where the action is irreversible.
  testWidgets('with nobody to block, the menu is not offered at all', (
    tester,
  ) async {
    await pump(tester);
    expect(find.byType(PopupMenuButton<void>), findsNothing);
  });

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

  // The bug this is here for: a `TextEditingController` built inside `build`
  // is a *new* controller on every rebuild, and a rebuild is exactly what an
  // arriving message causes. So a reply landing mid-sentence took the sentence
  // with it — in an app about two people writing to each other while the radio
  // comes and goes, that is the moment a draft matters most.
  testWidgets('a message arriving does not wipe what is half-typed', (
    tester,
  ) async {
    final sent = <String>[];
    Widget withLines(List<ThreadLine> lines) => MaterialApp(
      home: ThreadView(title: 'Wren', lines: lines, onSend: sent.add),
    );

    await tester.pumpWidget(withLines(const []));
    await tester.enterText(find.byType(TextField), 'on my way');

    // She replies while the sentence is still being typed.
    await tester.pumpWidget(
      withLines(const [ThreadLine(text: 'where are you?', outgoing: false)]),
    );
    await tester.pump();

    expect(find.text('where are you?'), findsOneWidget);
    expect(
      tester.widget<TextField>(find.byType(TextField)).controller?.text,
      'on my way',
      reason: 'an arriving message threw away the draft',
    );
  });

  // T14a on screen. Until an acknowledgement existed, `Sent` was terminal, so a
  // message the other person is reading looked exactly like one they refused —
  // and one they refused is a thing that happened, silently, on two phones.
  testWidgets('an outgoing line says how far it has got, until it arrives', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: ThreadView(
          title: 'Ada',
          lines: const [
            ThreadLine(
              text: 'still on this device',
              outgoing: true,
              state: MessageStateDto.queued,
            ),
            ThreadLine(
              text: 'left, and nobody has said anything',
              outgoing: true,
              state: MessageStateDto.sent,
            ),
            ThreadLine(
              text: 'she has it',
              outgoing: true,
              state: MessageStateDto.delivered,
            ),
          ],
          onSend: (_) {},
        ),
      ),
    );

    expect(find.text('waiting to send'), findsOneWidget);
    expect(find.text('not confirmed'), findsOneWidget);
    // Nothing on the settled one: a conversation that annotated every line
    // would be noise around the only two that matter.
    expect(find.textContaining('delivered'), findsNothing);

    // And the unconfirmed one is *marked*, not merely annotated. It will not be
    // resent by itself, so the mark is the whole of what happens next — a
    // person has to see it to act on it.
    //
    // Asserted against the line it belongs to, not counted. Counting passes for
    // a mark on the *wrong* line, because there is exactly one of each state on
    // screen — which is what the first version of this did.
    expect(
      find.descendant(
        of: find.ancestor(
          of: find.text('not confirmed'),
          matching: find.byType(Row),
        ),
        matching: find.byIcon(Icons.remove_circle_outline),
      ),
      findsOneWidget,
      reason: 'an unacknowledged message was not marked',
    );
    // And the one still being worked through carries none: nothing is being
    // asked of anybody yet.
    expect(
      find.descendant(
        of: find.ancestor(
          of: find.text('waiting to send'),
          matching: find.byType(Row),
        ),
        matching: find.byIcon(Icons.remove_circle_outline),
      ),
      findsNothing,
      reason: 'a message still on its way out was marked as unconfirmed',
    );
  });

  // Something that arrived is here by definition, so a note on it would be
  // describing this device's own bookkeeping back at the person.
  testWidgets('an incoming line carries no delivery note', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: ThreadView(
          title: 'Ada',
          lines: const [
            ThreadLine(
              text: 'from her',
              outgoing: false,
              state: MessageStateDto.sent,
            ),
          ],
          onSend: (_) {},
        ),
      ),
    );

    expect(find.text('from her'), findsOneWidget);
    expect(find.text('sent'), findsNothing);
  });
}
