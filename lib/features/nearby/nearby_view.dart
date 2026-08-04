import 'package:flutter/material.dart';

/// The sentence to show for a radio report, or null when the radio is fine.
///
/// `available` decides; `reason` only supplies the words. Deciding from the
/// reason instead gets the state backwards in both directions — an unavailable
/// radio with nothing to say reads as working, and a recovery that arrives
/// carrying a stale reason leaves the old sentence on screen.
///
/// A pure function so that rule is testable. It was stated in a comment and
/// got written the wrong way round one file over, in the same change.
String? radioReasonFrom({required bool available, String? reason}) =>
    available ? null : (reason ?? 'The radio is unavailable');

/// What fills the nearby area: the devices, or the reason there are none.
///
/// Split out of `HomePage` so the one rule that matters here can be tested
/// without a core, a radio, or two phones.
///
/// That rule is R0-F2. "No one nearby" is a claim about the room, and an empty
/// list is *equally* what an unusable radio produces — so making that claim
/// while Bluetooth is off tells the user something false about the world around
/// them. The reason therefore wins whenever there is one, even if devices are
/// still listed from before the radio went away.
/// Generic in the device type so the caller keeps its own: a `List<Object>`
/// here would push a down-cast back onto `HomePage`, and a cast is a mistake
/// that waits until runtime to happen.
class NearbyView<T> extends StatelessWidget {
  const NearbyView({
    super.key,
    required this.radioReason,
    required this.devices,
    required this.tile,
  });

  /// Why the radio is unusable, or null when it is fine.
  final String? radioReason;

  /// The devices last reported. May be non-empty even with [radioReason] set:
  /// the radio can go down before the list is cleared.
  final List<T> devices;

  /// How to draw one device. A callback so this widget needs nothing from the
  /// bridge, which is what lets it be tested at all.
  final Widget Function(T device) tile;

  static const emptyText = 'No one nearby. Turn on Discovery.';

  @override
  Widget build(BuildContext context) {
    final reason = radioReason;
    if (reason != null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Text(reason, textAlign: TextAlign.center),
        ),
      );
    }
    if (devices.isEmpty) {
      return const Center(child: Text(emptyText));
    }
    return ListView(children: devices.map(tile).toList());
  }
}
