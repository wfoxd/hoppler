import 'package:flutter/material.dart';

/// One person on the blocked list, as this screen needs them.
///
/// A plain record rather than the bridge's `BlockedPersonDto`, for the same
/// reason `ThreadLine` is one: the widget stays buildable in a test without the
/// Rust bridge.
class BlockedPerson {
  const BlockedPerson({
    required this.contactId,
    required this.name,
    required this.colour,
  });
  final int contactId;
  final String name;
  final int colour;
}

/// Everyone this device is refusing, and the way back.
///
/// Nothing here says how durable a block is. That is a fact about our key
/// material rather than about the person, and there is nothing a reader could
/// do with it — see T18b and T18c.
class BlockedView extends StatelessWidget {
  const BlockedView({
    super.key,
    required this.people,
    required this.onUnblock,
  });

  final List<BlockedPerson> people;

  /// Called with the contact id. A callback rather than the Core API so this
  /// screen is testable without the bridge.
  final void Function(BlockedPerson) onUnblock;

  /// What an empty list says.
  ///
  /// Not "No blocked people", which reads as a failure to load. This says the
  /// state is fine and, quietly, where blocking is done from — nobody arrives
  /// here to learn that, but somebody who arrived expecting a name and found
  /// none needs to know they are in the right place.
  static const emptyHint = "You haven't blocked anyone.";

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Blocked')),
      body: people.isEmpty
          ? const Center(child: Text(emptyHint))
          : ListView(
              children: [
                for (final p in people)
                  ListTile(
                    key: ValueKey(p.contactId),
                    leading: CircleAvatar(
                      backgroundColor: Color(0xFF000000 | p.colour),
                    ),
                    title: Text(p.name.isEmpty ? 'Unknown device' : p.name),
                    trailing: TextButton(
                      onPressed: () => onUnblock(p),
                      child: const Text('Unblock'),
                    ),
                  ),
              ],
            ),
    );
  }
}
