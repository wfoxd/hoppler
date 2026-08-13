import 'package:flutter/material.dart';

/// One colour of the short authentication string: what to paint, and what to
/// call it.
///
/// A local type rather than the bridge's `SasColourDto`, so this widget can be
/// tested without a core — the same reason `NearbyView` is generic over its
/// device type. `HomePage` does the one-line mapping.
class SasColour {
  const SasColour({required this.name, required this.rgb});

  final String name;

  /// Packed 0xRRGGBB, as everything else in this app carries colour.
  final int rgb;

  Color get colour => Color(0xFF000000 | rgb);
}

/// The screen where pairing is actually decided (R0-F4).
///
/// Everything before this is cryptography that cannot tell whether the person
/// on the other end of the handshake is the person in the room. An attacker
/// relaying between two devices passes every check the protocol makes — and
/// produces a *different* transcript on each side, so the two renderings
/// differ. The comparison is what catches it, and the two people are the only
/// participants in a position to make it.
///
/// Which is why the confirm button cannot be pressed for them, why there is no
/// "trust automatically", and why this widget refuses to say anything that
/// sounds like a verdict. It reports a state and offers two answers.
class SasView extends StatelessWidget {
  const SasView({
    super.key,
    required this.colours,
    required this.word,
    required this.peerName,
    required this.peerConfirmed,
    required this.weConfirmed,
    required this.onConfirm,
    required this.onCancel,
  });

  /// Rendered in order. The order carries information — "teal then amber" is
  /// not "amber then teal" — so this is never sorted, and the count is whatever
  /// arrives: how many colours there are is a security parameter still under
  /// discussion, and a screen built for exactly two would have to be rebuilt to
  /// change it.
  final List<SasColour> colours;

  final String word;
  final String peerName;

  /// Whether the other person has confirmed. A cue only: it is *not* progress
  /// toward pairing on this side, and the copy below is careful not to imply
  /// that someone else's tap has decided anything here.
  final bool peerConfirmed;

  /// Whether this person has confirmed and is now waiting.
  final bool weConfirmed;

  final VoidCallback onConfirm;
  final VoidCallback onCancel;

  static const prompt = 'Do these match on both phones?';
  static const waiting = 'Waiting for the other phone';
  static const mismatchHint =
      "If they don't match, stop. Someone is in the way.";

  @override
  Widget build(BuildContext context) {
    // Scrollable, because at large text scales this content is genuinely
    // taller than a phone and there is nothing here that can be dropped to
    // make it fit: the colours, the word, the question and the two answers are
    // all load-bearing. `minHeight` keeps it centred whenever there *is* room,
    // so the ordinary case looks the same as before.
    return LayoutBuilder(
      builder: (context, constraints) => SingleChildScrollView(
        child: ConstrainedBox(
          constraints: BoxConstraints(minHeight: constraints.maxHeight),
          child: Padding(
            padding: const EdgeInsets.all(24),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  'Pairing with $peerName',
                  textAlign: TextAlign.center,
                  style: Theme.of(context).textTheme.titleMedium,
                ),
                const SizedBox(height: 24),
                // `Wrap`, not `Row`, so the colour count is genuinely free
                // rather than free up to four. A row of 64px swatches fits four
                // on a 320dp screen and overflows on five — no proposal goes
                // that far, but the reason this is a list at all is that the
                // number is unsettled, and a layout with a quiet ceiling in it
                // is the sort of thing found much later by someone changing a
                // constant in Rust.
                Wrap(
                  alignment: WrapAlignment.center,
                  spacing: 24,
                  runSpacing: 16,
                  children: [for (final c in colours) _Swatch(colour: c)],
                ),
                const SizedBox(height: 16),
                Text(
                  word,
                  textAlign: TextAlign.center,
                  style: Theme.of(context).textTheme.headlineSmall,
                ),
                const SizedBox(height: 24),
                Text(prompt, textAlign: TextAlign.center),
                const SizedBox(height: 8),
                // Said plainly, and not in a colour that reads as an error. A person
                // who sees a mismatch is being told to stop, which is the one
                // instruction on this screen that matters.
                Text(
                  mismatchHint,
                  textAlign: TextAlign.center,
                  style: Theme.of(context).textTheme.bodySmall,
                ),
                const SizedBox(height: 24),
                if (peerConfirmed && !weConfirmed)
                  // Shown, because a person waiting on someone who has already tapped
                  // should not think the other phone is broken. Deliberately not
                  // phrased as encouragement to press the button.
                  const Padding(
                    padding: EdgeInsets.only(bottom: 8),
                    child: Text(
                      'They have confirmed on their phone.',
                      textAlign: TextAlign.center,
                    ),
                  ),
                if (weConfirmed)
                  const Center(child: Text(waiting))
                else
                  // `OverflowBar`, not `Row`, and this is where the overflow actually
                  // was: "They don't match" is a long label, and two of these side by
                  // side do not fit a 320dp screen even before anyone turns text size
                  // up. It falls back to a column rather than clipping — and the
                  // fallback keeps the destructive answer *first*, so the button
                  // under a thumb is never the one that confirms.
                  OverflowBar(
                    alignment: MainAxisAlignment.spaceEvenly,
                    overflowAlignment: OverflowBarAlignment.center,
                    overflowSpacing: 8,
                    children: [
                      TextButton(
                        onPressed: onCancel,
                        child: const Text("They don't match"),
                      ),
                      FilledButton(
                        onPressed: onConfirm,
                        child: const Text('They match'),
                      ),
                    ],
                  ),
                if (weConfirmed)
                  Padding(
                    padding: const EdgeInsets.only(top: 16),
                    child: Center(
                      child: TextButton(
                        onPressed: onCancel,
                        child: const Text('Stop'),
                      ),
                    ),
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A colour, with its name under it.
///
/// Both, always. A swatch on its own cannot be compared over a phone call and
/// cannot be compared at all by someone who is colour-blind — and this is the
/// step that decides whether their pairing is safe. A verification some people
/// cannot perform is one they will skip.
class _Swatch extends StatelessWidget {
  const _Swatch({required this.colour});

  final SasColour colour;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 64,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 64,
            height: 64,
            decoration: BoxDecoration(
              color: colour.colour,
              borderRadius: BorderRadius.circular(8),
              // Outlined, or a pale swatch vanishes into the background and a
              // dark one vanishes in dark mode. The palette deliberately spans
              // both ends, so neither case is hypothetical.
              border: Border.all(color: Theme.of(context).dividerColor),
            ),
          ),
          const SizedBox(height: 6),
          // Centred and allowed to wrap within the swatch's width, so a long
          // name at a large text scale grows downward rather than sideways
          // into its neighbour.
          Text(colour.name, textAlign: TextAlign.center),
        ],
      ),
    );
  }
}
