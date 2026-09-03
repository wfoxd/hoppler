import 'package:flutter/material.dart';
import 'package:hoppler/src/rust/api/types.dart';

/// One line of a conversation, as this screen needs it.
///
/// A plain record rather than the bridge's `ChatMessageDto` so the widget below
/// can be built in a test: what it says, which side it came from, and how far
/// it has got.
class ThreadLine {
  const ThreadLine({
    required this.text,
    required this.outgoing,
    this.state = MessageStateDto.delivered,
  });
  final String text;
  final bool outgoing;

  /// How far this one has got. Only meaningful on a line this device wrote —
  /// something that arrived is here by definition.
  final MessageStateDto state;

  /// What the row is entitled to claim, in words rather than a tick.
  ///
  /// A tick is the convention and it is the wrong one here. Ticks are read as
  /// "sent" and "read", and this app knows neither of those: it knows the bytes
  /// left, and it knows whether the other device said it stored them. Nobody
  /// has been told anyone read anything.
  ///
  /// `null` on an incoming line, and on a delivered one — a conversation that
  /// annotated every settled line would be noise around the only two that
  /// matter.
  String? get note {
    if (!outgoing) return null;
    switch (state) {
      case MessageStateDto.queued:
        return 'waiting to send';
      case MessageStateDto.sent:
        return 'not confirmed';
      case MessageStateDto.delivered:
        return null;
    }
  }

  /// Whether this line is *marked* rather than merely annotated.
  ///
  /// A message that left and was never acknowledged is not resent by itself —
  /// that is decided, so the mark is the whole of what happens next, and it has
  /// to be visible enough to act on. "Waiting to send" is a state the app is
  /// still working through and needs no mark; "not confirmed" is one it has
  /// stopped working on.
  ///
  /// Deliberately not an error colour. Nothing has gone wrong the moment a
  /// message leaves, and most are confirmed within a second — an alarm that
  /// fires on every message and clears itself teaches people to stop reading
  /// it, and the one time it stays is the time that mattered.
  bool get marked => outgoing && state == MessageStateDto.sent;
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
    this.onBlock,
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

  /// Block the person this conversation is with (R0-F10).
  ///
  /// `null` hides the menu entirely rather than showing it disabled. A
  /// conversation with nobody identifiable — no thread and no handle — has
  /// nothing to block, and offering the action anyway would be a tap that
  /// quietly fails on the one screen where failing quietly is worst.
  ///
  /// The confirming and the doing both belong to the caller: this widget stays
  /// buildable without the Rust bridge, which is what makes the menu testable
  /// at all.
  final VoidCallback? onBlock;

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
      appBar: AppBar(
        title: Text(widget.title),
        actions: [
          // Behind an overflow rather than out on the bar. It is destructive
          // and irreversible, and the bar of a conversation is somewhere a
          // thumb rests.
          if (widget.onBlock != null)
            PopupMenuButton<void>(
              itemBuilder: (context) => [
                PopupMenuItem<void>(
                  onTap: widget.onBlock,
                  child: const Text('Block'),
                ),
              ],
            ),
        ],
      ),
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
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.end,
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                Text(line.text),
                                if (line.note != null)
                                  Padding(
                                    padding: const EdgeInsets.only(top: 4),
                                    child: Row(
                                      mainAxisSize: MainAxisSize.min,
                                      children: [
                                        if (line.marked) ...[
                                          Icon(
                                            Icons.remove_circle_outline,
                                            size: 12,
                                            color: Theme.of(context).hintColor,
                                          ),
                                          const SizedBox(width: 4),
                                        ],
                                        Text(
                                          line.note!,
                                          style: Theme.of(context)
                                              .textTheme
                                              .labelSmall
                                              ?.copyWith(
                                                color: Theme.of(
                                                  context,
                                                ).hintColor,
                                              ),
                                        ),
                                      ],
                                    ),
                                  ),
                              ],
                            ),
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
