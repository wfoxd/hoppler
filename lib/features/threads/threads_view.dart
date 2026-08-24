import 'package:flutter/material.dart';

/// The list of conversations.
///
/// Generic over the row type for the same reason [NearbyView] is: the thing
/// worth getting right here — what "no conversations yet" should say, and that
/// a conversation is reachable at all — cannot be tested while the widget needs
/// the Rust bridge to build.
///
/// # Why this screen exists
///
/// Every conversation the core could hold was unreachable before it. Messages
/// were stored, delivered on reunion and written to the debug log, and the only
/// door into a thread was tapping Chat on a nearby tile — so a message that
/// arrived while its sender was out of range could not be read by anybody. Two
/// phones proved the delivery half and had nowhere to show the result.
class ThreadsView<T> extends StatelessWidget {
  const ThreadsView({super.key, required this.threads, required this.tile});

  final List<T> threads;

  /// How to draw one conversation. A callback so this widget needs nothing
  /// from the bridge, which is what lets it be tested at all.
  final Widget Function(T thread) tile;

  /// Shown before anyone has said anything.
  ///
  /// Names the way in, because there isn't one from this screen: a conversation
  /// starts by writing to somebody on the nearby list, and a bare "nothing
  /// here" would leave a person looking for a button that does not exist.
  static const emptyText = 'No conversations yet.\nSay hello to someone nearby.';

  @override
  Widget build(BuildContext context) {
    if (threads.isEmpty) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Text(emptyText, textAlign: TextAlign.center),
        ),
      );
    }
    return ListView.builder(
      itemCount: threads.length,
      itemBuilder: (_, i) => tile(threads[i]),
    );
  }
}
