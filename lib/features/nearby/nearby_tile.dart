import 'package:flutter/material.dart';
import 'package:hoppler/features/ping/ping_button.dart';
import 'package:hoppler/features/ping/ping_service.dart';
import 'package:hoppler/src/rust/api/types.dart';

/// One row of the nearby list.
///
/// Extracted from `main.dart` for the same reason `PingButton` was: the logic
/// worth getting right here — what a device with no name yet looks like — is
/// unreachable in a test while it lives inside a page that needs the Rust
/// bridge to build.
class NearbyTile extends StatelessWidget {
  const NearbyTile({
    super.key,
    required this.device,
    required this.pingService,
    required this.onOpen,
    required this.onDrop,
    this.onBlock,
  });

  final NearbyDevice device;
  final PingService pingService;

  /// Open the conversation with this person.
  ///
  /// The row itself is the way in — their name and their colour, which is what
  /// somebody looks at when they mean "talk to them". It replaced a Chat button
  /// that sent a fixed greeting, so the only thing you could say to a person on
  /// this screen was `Hey <name>!` and saying it twice sent it twice.
  ///
  /// Kept as a callback rather than the Core API, so this stays testable
  /// without the Rust bridge.
  final VoidCallback onOpen;
  final VoidCallback onDrop;

  /// Block this person (R0-F10), on a long press.
  ///
  /// Not a third icon button. The row already carries Ping and Drop, and a
  /// short tap opens the conversation — putting a destructive, irreversible
  /// action one mis-tap from a chat is the wrong trade for discoverability.
  ///
  /// Long press is the Material convention for exactly this, and the action is
  /// reachable a second way from inside the conversation, which is where
  /// somebody is most likely to be when they decide.
  final VoidCallback? onBlock;

  /// Whether we know who this is yet.
  ///
  /// An advertisement says a device is *there* and never who it is — the
  /// payload is deliberately empty, so a passive scanner in range learns
  /// nothing about anyone. A name is disclosed only over a pipe, to a peer that
  /// has identified itself first, and a Ping is what opens that pipe.
  ///
  /// So a nameless device is the protocol working exactly as intended. A blank
  /// tile, though, reads as a bug — it sent us looking for one — so this says
  /// what is actually true and what to do about it.
  bool get isKnown => device.name.isNotEmpty;

  /// Whether the radio can reach them this second.
  ///
  /// Derived from the handle rather than a flag beside it: the handle is what
  /// Ping and Drop are dialled with, so its absence *is* "cannot be reached",
  /// and there is no second fact to fall out of step with it.
  bool get isHere => device.deviceId != null;

  /// What a paired friend who is not in range can still be told.
  ///
  /// R0-F5 says a message written out of range is kept and delivered at the
  /// next encounter, so the conversation stays open. Ping and Drop do not: a
  /// Ping is a nudge someone answers now, and a Drop needs both phones
  /// present. Offering them anyway would mean a tap that quietly does
  /// nothing.
  static const awayHint = 'Ping and Drop need them nearby.';

  /// A disabled control that still swallows the tap that lands on it.
  ///
  /// An `IconButton` with no callback registers no gesture, so the tap carries
  /// straight through to whatever is behind — here the row, which would open a
  /// conversation. That is the one reading a person would least expect from a
  /// control shown greyed out beside the words "Ping and Drop need them
  /// nearby": they tapped the thing that says it is unavailable, and something
  /// else happened.
  ///
  /// A `GestureDetector` rather than an `AbsorbPointer`, so the long press that
  /// shows the tooltip still gets through. Taps compete in the gesture arena
  /// and the deepest recogniser wins, which is this one.
  ///
  /// `excludeFromSemantics` because the handler exists only to swallow. Without
  /// it the wrapper advertises a tap action, so a screen reader offers to
  /// activate a control the same tree describes as disabled — and activating it
  /// does nothing, which is a worse answer than the button gives on its own.
  /// What is silenced is this wrapper, not the button: the `IconButton` keeps
  /// its own semantics, including that it is unavailable.
  static Widget _unavailable(Widget button) =>
      GestureDetector(onTap: () {}, excludeFromSemantics: true, child: button);

  @override
  Widget build(BuildContext context) {
    return ListTile(
      // The whole row bar the buttons: avatar, name and subtitle. An enabled
      // control keeps its own hit target and wins the gesture arena by being
      // deeper in the tree — a *disabled* one does not, which is why the two
      // below are wrapped. See [`_unavailable`].
      onTap: onOpen,
      onLongPress: onBlock,
      leading: CircleAvatar(
        // Colour arrives with the persona. Until then it is 0, which renders as
        // flat black and looks like a rendering fault rather than an absence.
        backgroundColor: isKnown
            ? Color(0xFF000000 | device.colour)
            : Colors.grey.shade400,
      ),
      title: Text(isKnown ? device.name : 'Unknown device'),
      subtitle: Text(
        !isHere
            ? 'away · $awayHint'
            : isKnown
            ? (device.paired ? 'paired' : 'nearby')
            : 'Ping to connect',
      ),
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (isHere)
            PingButton(
              key: ValueKey(device.deviceId),
              service: pingService,
              deviceId: device.deviceId!,
            )
          else
            // Deliberately shown and disabled rather than removed. A row that
            // loses its buttons when somebody walks away looks broken; one
            // whose buttons are visibly unavailable says what changed.
            _unavailable(
              const IconButton(
                icon: Icon(Icons.notifications_none),
                tooltip: awayHint,
                onPressed: null,
              ),
            ),
          if (isHere)
            IconButton(
              icon: const Icon(Icons.upload_file_outlined),
              tooltip: 'Drop',
              onPressed: onDrop,
            )
          else
            _unavailable(
              const IconButton(
                icon: Icon(Icons.upload_file_outlined),
                tooltip: awayHint,
                onPressed: null,
              ),
            ),
        ],
      ),
    );
  }
}
