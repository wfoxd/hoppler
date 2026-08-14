import 'package:flutter/material.dart';
import 'package:qr_flutter/qr_flutter.dart';

/// The code one person holds up for the other to scan (R0-F4, tech spec §6).
///
/// A QR code and nothing clever. The payload is already a URI the core minted,
/// so this widget knows nothing about pairing beyond how to draw a string —
/// which is what lets it be tested without a core, a camera or a second phone.
///
/// # Why the copy says what it says
///
/// A code on screen pairs a Layer-2 key with the rung id this device is
/// advertising under, and anyone who photographs it can link the two until the
/// next rotation (R0-F2 otherwise prevents exactly that). That is inherent to
/// showing a code to someone in the room and it is bounded by the rotation
/// period — but it is the reason the screen tells people to put it away rather
/// than leaving it up, and the reason `HomePage` stops showing it on close.
class PairingCodeView extends StatelessWidget {
  const PairingCodeView({
    super.key,
    required this.code,
    required this.onDone,
    this.canScan = true,
    this.canTap = false,
  });

  /// The invite URI, straight from `pairingInvite()`.
  final String code;

  final VoidCallback onDone;

  /// Whether this build can scan a code as well as show one.
  ///
  /// False on desktop, and said out loud rather than hidden: Flutter has no
  /// camera plugin for Linux, so the pairing screen there can only ever be one
  /// half of the ceremony. A build that silently offered no scan button would
  /// leave a person hunting for one; R0-N6's rule is that the UI states its
  /// reach honestly, and this is that rule applied to a platform gap rather
  /// than a radio one.
  final bool canScan;

  /// Whether this device can also pair by tapping phones together.
  ///
  /// Additive, never a replacement: the tap is offered *beside* the code, not
  /// instead of it. Plenty of phones have no NFC and the QR is the path that
  /// always works, so a screen that mentioned only tapping would strand them.
  final bool canTap;

  static const instruction = 'Have them scan this with Hoppler.';
  static const tapInstruction = 'Or hold the two phones back to back.';
  static const noCameraHere =
      'This device cannot scan a code — it has no camera Hoppler can use. '
      'Show this one instead.';

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Center(
            child: QrImageView(
              data: code,
              size: 240,
              // Fixed white behind the code, in both themes. A QR reader needs
              // the contrast the spec assumes, and a dark-mode surface behind
              // dark modules is a code that will not scan — which would look
              // like the ceremony failing rather than the theme doing it.
              backgroundColor: Colors.white,
              padding: const EdgeInsets.all(12),
            ),
          ),
          const SizedBox(height: 24),
          Text(
            canScan ? instruction : noCameraHere,
            textAlign: TextAlign.center,
          ),
          if (canTap) ...[
            const SizedBox(height: 8),
            Text(tapInstruction, textAlign: TextAlign.center),
          ],
          const SizedBox(height: 24),
          Center(
            child: TextButton(onPressed: onDone, child: const Text('Done')),
          ),
        ],
      ),
    );
  }
}
