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
    required this.onChat,
    required this.onDrop,
  });

  final NearbyDevice device;
  final PingService pingService;

  /// Given the greeting to send, so the caller keeps the Core API and this
  /// stays testable without it.
  final void Function(String text) onChat;
  final VoidCallback onDrop;

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

  @override
  Widget build(BuildContext context) {
    return ListTile(
      leading: CircleAvatar(
        // Colour arrives with the persona. Until then it is 0, which renders as
        // flat black and looks like a rendering fault rather than an absence.
        backgroundColor: isKnown
            ? Color(0xFF000000 | device.colour)
            : Colors.grey.shade400,
      ),
      title: Text(isKnown ? device.name : 'Unknown device'),
      // Three things to say, not two. A paired person stays listed when the
      // radio cannot see them — that is what makes writing to them possible at
      // all — so the tile has to distinguish "here" from "listed but away", or
      // it claims someone is in the room who is not.
      //
      // Away is said plainly rather than dressed up: a message written now will
      // wait, and somebody deciding whether to type is better served by the
      // truth than by a tile that looks the same either way.
      subtitle: Text(
        !isKnown
            ? 'Ping to connect'
            : !device.present
            ? 'paired — away, messages will wait'
            : device.paired
            ? 'paired'
            : 'nearby',
      ),
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          PingButton(
            key: ValueKey(device.deviceId),
            service: pingService,
            deviceId: device.deviceId,
          ),
          IconButton(
            icon: const Icon(Icons.chat_bubble_outline),
            tooltip: 'Chat',
            // Without the guard this greets an unnamed peer as "Hey !", which
            // is what actually arrived on the far phone during the LAN run.
            onPressed: () => onChat(isKnown ? 'Hey ${device.name}!' : 'Hey!'),
          ),
          IconButton(
            icon: const Icon(Icons.upload_file_outlined),
            tooltip: 'Drop',
            onPressed: onDrop,
          ),
        ],
      ),
    );
  }
}
