import 'package:flutter/material.dart';

/// One line of a conversation, as this screen needs it.
///
/// A plain record rather than the bridge's `ChatMessageDto` so the widget below
/// can be built in a test. The two fields are the whole of what a message looks
/// like on screen: what it says, and which side it came from.
class ThreadLine {
  const ThreadLine({required this.text, required this.outgoing});
  final String text;
  final bool outgoing;
}

/// One conversation: what has been said, and a box to say more.
///
/// Ordering is the caller's — the core returns messages in the store's causal
/// insertion order, which is deliberately not the wall clock (two phones
/// disagree by seconds) and not `seq` (which is per-sender, so both sides
/// number from 1 and interleaving them by number would shuffle the
/// conversation).
class ThreadView extends StatefulWidget {
  const ThreadView({
    super.key,
    required this.title,
    required this.lines,
    required this.onSend,
    this.canSend = true,
    this.cannotSendReason,
  });

  final String title;
  final List<ThreadLine> lines;
  final void Function(String text) onSend;

  /// Whether anything can be written here at all.
  ///
  /// Sending is nearly always possible — R0-F5 keeps a message for somebody out
  /// of range and delivers it at the next encounter, so being away is not a
  /// reason to refuse. This exists for the case that is not about distance:
  /// a conversation whose contact is gone.
  final bool canSend;
  final String? cannotSendReason;

  @override
  State<ThreadView> createState() => _ThreadViewState();
}

/// Stateful for one reason: the box has to remember what is half-typed.
///
/// A controller built in `build` is a new controller on every rebuild, and a
/// rebuild here is exactly what an *arriving message* causes — so a reply
/// landing mid-sentence would take the sentence with it. In an app whose whole
/// subject is two people writing to each other while the radio comes and goes,
/// that is the moment the draft matters most.
///
/// It is also the only thing here worth owning: the lines come from the store
/// on every change, deliberately, so this holds no copy of the conversation.
class _ThreadViewState extends State<ThreadView> {
  final _controller = TextEditingController();

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final controller = _controller;
    final lines = widget.lines;
    final canSend = widget.canSend;
    final cannotSendReason = widget.cannotSendReason;
    return Scaffold(
      appBar: AppBar(title: Text(widget.title)),
      body: Column(
        children: [
          Expanded(
            child: lines.isEmpty
                ? const Center(child: Text('Nothing yet.'))
                : ListView.builder(
                    padding: const EdgeInsets.all(8),
                    itemCount: lines.length,
                    itemBuilder: (_, i) {
                      final line = lines[i];
                      return Align(
                        alignment: line.outgoing
                            ? Alignment.centerRight
                            : Alignment.centerLeft,
                        child: Card(
                          child: Padding(
                            padding: const EdgeInsets.all(10),
                            child: Text(line.text),
                          ),
                        ),
                      );
                    },
                  ),
          ),
          const Divider(height: 1),
          Padding(
            padding: const EdgeInsets.all(8),
            child: Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: controller,
                    enabled: canSend,
                    decoration: InputDecoration(
                      hintText: canSend ? 'Message' : cannotSendReason ?? '',
                      border: const OutlineInputBorder(),
                    ),
                    onSubmitted: (t) => _send(controller, t),
                  ),
                ),
                IconButton(
                  icon: const Icon(Icons.send),
                  tooltip: canSend ? 'Send' : cannotSendReason,
                  onPressed: canSend
                      ? () => _send(controller, controller.text)
                      : null,
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  /// Nothing is sent for an empty box, and the box is cleared on the way.
  ///
  /// The guard is here rather than at the call site because both routes in —
  /// the button and the keyboard's send key — would otherwise need it, and one
  /// of them would eventually not have it.
  void _send(TextEditingController controller, String text) {
    final trimmed = text.trim();
    if (trimmed.isEmpty) return;
    controller.clear();
    widget.onSend(trimmed);
  }
}
