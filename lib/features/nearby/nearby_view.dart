import 'package:flutter/material.dart';

/// The sentence to put where the nearby list goes, or null when there is none.
///
/// `available` decides; `reason` only supplies the words. Deciding from the
/// reason instead gets the state backwards in both directions — an unavailable
/// radio with nothing to say reads as working, and a recovery that arrives
/// carrying a stale reason leaves the old sentence on screen.
///
/// `permissionDenied` overrides both, for the reason below.
///
/// A pure function so all of that is testable. It was once stated in a comment
/// and written the wrong way round one file over, in the same change.
///
/// # Why a refused permission outranks the radio's own report
///
/// Both end in an empty list, and the empty list is the thing R0-F2 says must
/// never be read as "nobody is nearby". But they are not equally useful to say.
/// A refusal is the *cause* — the radio has nothing to report because it was
/// never allowed to look — and it is the only one of the two a person can do
/// something about. So it wins, and it wins even when the radio is reporting
/// itself available, because a permission we do not hold makes that report
/// meaningless.
///
/// Before this, a refusal put one line in the activity log and left the nearby
/// area saying "No one nearby" — which is the false claim about the room that
/// this whole function exists to prevent, arrived at by the one route nothing
/// was watching.
String? radioReasonFrom({
  required bool available,
  String? reason,
  bool permissionDenied = false,
}) {
  if (permissionDenied) {
    return 'Hoppler needs permission to use Bluetooth. You can grant it in '
        'Settings.';
  }
  return available ? null : (reason ?? 'The radio is unavailable');
}

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
    required this.discoveryOn,
    required this.devices,
    required this.tile,
  });

  /// Why the radio is unusable, or null when it is fine.
  final String? radioReason;

  /// Whether Discovery is switched on.
  ///
  /// Needed only to pick the empty-list wording, and that is reason enough:
  /// without it the screen told a user whose Discovery was already on to turn
  /// on Discovery. A message that contradicts the switch three inches above it
  /// teaches people to stop reading the messages.
  final bool discoveryOn;

  /// The devices last reported. May be non-empty even with [radioReason] set:
  /// the radio can go down before the list is cleared.
  final List<T> devices;

  /// How to draw one device. A callback so this widget needs nothing from the
  /// bridge, which is what lets it be tested at all.
  final Widget Function(T device) tile;

  /// Shown when Discovery is off: an empty list is expected, and the way out
  /// of it is the switch.
  static const emptyOffText = 'No one nearby. Turn on Discovery.';

  /// Shown when Discovery is on and the room really is empty. It says nothing
  /// about *why*, because with the radio fine and Discovery on there is nothing
  /// to explain — an instruction here would be an instruction to do nothing.
  static const emptyOnText = 'No one nearby yet.';

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
      return Center(child: Text(discoveryOn ? emptyOnText : emptyOffText));
    }
    return ListView(children: devices.map(tile).toList());
  }
}
