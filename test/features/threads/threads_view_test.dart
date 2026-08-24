import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/threads/threads_view.dart';

void main() {
  Future<void> pump(WidgetTester tester, List<String> threads) =>
      tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: ThreadsView<String>(
              threads: threads,
              tile: (t) => ListTile(title: Text(t)),
            ),
          ),
        ),
      );

  // The empty state has to name the way in, because there isn't one from this
  // screen: a conversation starts by writing to somebody on the nearby list.
  // "Nothing here" alone sends a person looking for a button that does not
  // exist.
  testWidgets('with nothing to show, it says how a conversation starts', (
    tester,
  ) async {
    await pump(tester, []);

    expect(find.text(ThreadsView.emptyText), findsOneWidget);
    expect(find.textContaining('nearby'), findsOneWidget);
  });

  testWidgets('every conversation is listed', (tester) async {
    await pump(tester, ['Wren', 'Ada']);

    expect(find.text('Wren'), findsOneWidget);
    expect(find.text('Ada'), findsOneWidget);
    expect(find.text(ThreadsView.emptyText), findsNothing);
  });
}
