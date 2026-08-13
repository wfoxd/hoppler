import 'package:flutter/material.dart';

/// What the main area shows, in one ordered decision.
///
/// # Why this exists
///
/// The code used to be a modal bottom sheet while the colours were drawn in the
/// page body underneath it. On two phones that meant the person *showing* the
/// code reached the SAS — the log said so — and never saw it, because their own
/// QR was still on top. They could not confirm, so the ceremony could not
/// complete, and the half of the app that worked (the scanner pops its sheet on
/// a successful scan) hid the half that did not.
///
/// Nothing caught it: both views passed their own widget tests, and the defect
/// was in the composition, which was a race between a route and a piece of
/// state and so had no test at all. Putting every pairing surface in one
/// ordered expression is what makes that class of bug unrepresentable — a route
/// on top of the body cannot be reasoned about from the body's own state, and
/// this has no routes.
///
/// The order below is the whole content of the fix, so it is worth stating: the
/// colours win over the code, always. Reaching the SAS means the code has
/// already been read and is of no further use, and it is the only screen here
/// that is waiting on a person.
class PairingSurface extends StatelessWidget {
  const PairingSurface({
    super.key,
    required this.sas,
    required this.code,
    required this.scanner,
    required this.nearby,
  });

  /// The colours screen, if a ceremony has reached them.
  final Widget? sas;

  /// This device's own code, if it is being shown.
  final Widget? code;

  /// The camera, if it is open.
  final Widget? scanner;

  /// Everything else: who is nearby.
  final Widget nearby;

  @override
  Widget build(BuildContext context) {
    // Colours first. Every other surface here is something a person opened and
    // can close again; this one is a ceremony in progress with someone else
    // waiting on the other end of it.
    if (sas != null) return sas!;
    if (scanner != null) return scanner!;
    if (code != null) return code!;
    return nearby;
  }
}
