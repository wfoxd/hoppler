import 'package:flutter/material.dart';

/// The question asked before a block, and the sentence that makes it a
/// question worth asking.
///
/// Blocking is the only irreversible thing in this app short of the R0-F9 wipe.
/// It revokes a pairing, and R0-F10 says unblocking restores stranger-level
/// status and *not* the pairing — so the dialog's job is to say the part that
/// cannot be undone, rather than to ask "are you sure" and hope.
///
/// What it deliberately does **not** say is how durable the block will be. A
/// block binds to the strongest handle this device holds and that is sometimes
/// a weak one (T18b), but a person cannot act on that: they cannot make their
/// phone know somebody better, and the alternative to a weak block is no block.
/// A caveat here would be a warning nobody can use, at the moment they least
/// want one.
///
/// Returns `true` only on the explicit Block. Dismissing by tapping outside
/// gives `null`, which the caller must read as "no" — hence [showBlockConfirm]
/// rather than leaving every call site to remember.
class BlockConfirm extends StatelessWidget {
  const BlockConfirm({super.key, required this.name});

  /// Who is about to be blocked. Empty for somebody whose persona this device
  /// never learned — they are still blockable, and "this device" is the
  /// truthful way to name one.
  final String name;

  String get _who => name.isEmpty ? 'this device' : name;

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text('Block $_who?'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text("They won't be able to reach you, and they won't be told."),
          const SizedBox(height: 12),
          // The whole reason this dialog exists. Stated as what happens to
          // *them and you*, not as "pairing state is revoked".
          Text(
            "This undoes your pairing. Unblocking later doesn't bring it "
            "back — you'd have to pair again, in person.",
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(false),
          child: const Text('Cancel'),
        ),
        TextButton(
          onPressed: () => Navigator.of(context).pop(true),
          child: const Text('Block'),
        ),
      ],
    );
  }
}

/// Ask, and read a dismissal as no.
///
/// `showDialog` completes with `null` when somebody taps outside it or presses
/// back. Every call site would have to remember that a null means don't, and
/// the one that forgets blocks somebody who tried to get out of the dialog.
Future<bool> showBlockConfirm(BuildContext context, String name) async {
  final answer = await showDialog<bool>(
    context: context,
    builder: (_) => BlockConfirm(name: name),
  );
  return answer ?? false;
}
