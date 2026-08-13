import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/pairing/sas_view.dart';

/// The SAS screen is the only place in Hoppler where a person, not a proof,
/// decides something. These tests are about that: what it shows, what it
/// refuses to claim, and that it cannot decide on anyone's behalf.
void main() {
  const colours = [
    SasColour(name: 'blue', rgb: 0x4363D8),
    SasColour(name: 'navy', rgb: 0x000075),
  ];

  Widget host({
    List<SasColour> palette = colours,
    bool peerConfirmed = false,
    bool weConfirmed = false,
    VoidCallback? onConfirm,
    VoidCallback? onCancel,
  }) => MaterialApp(
    home: Scaffold(
      body: SasView(
        colours: palette,
        word: 'yeast',
        peerName: 'Ada',
        peerConfirmed: peerConfirmed,
        weConfirmed: weConfirmed,
        onConfirm: onConfirm ?? () {},
        onCancel: onCancel ?? () {},
      ),
    ),
  );

  testWidgets('shows every colour by name as well as by swatch', (t) async {
    await t.pumpWidget(host());
    // The names are not decoration. Two people on a phone call cannot compare
    // swatches, and a colour-blind person cannot compare them at all — this is
    // the step that decides whether their pairing is safe, so a check some
    // people cannot perform is one they will skip.
    expect(find.text('blue'), findsOneWidget);
    expect(find.text('navy'), findsOneWidget);
    expect(find.text('yeast'), findsOneWidget);
  });

  testWidgets('renders however many colours it is given', (t) async {
    // How many colours there are is a security parameter, and the number is
    // still open — each one is four bits an active relay has to guess. A screen
    // built for exactly two would have to be rebuilt to change it.
    await t.pumpWidget(
      host(
        palette: const [
          SasColour(name: 'red', rgb: 0xE6194B),
          SasColour(name: 'mint', rgb: 0xAAFFC3),
          SasColour(name: 'slate', rgb: 0xA9A9A9),
        ],
      ),
    );
    expect(find.text('red'), findsOneWidget);
    expect(find.text('mint'), findsOneWidget);
    expect(find.text('slate'), findsOneWidget);
  });

  testWidgets('says to stop if they do not match', (t) async {
    await t.pumpWidget(host());
    // The one instruction on this screen that matters. A mismatch is not a
    // retry prompt — it is the signal that somebody is relaying the ceremony.
    expect(find.text(SasView.mismatchHint), findsOneWidget);
  });

  testWidgets('the other person confirming does not confirm for us', (t) async {
    var confirmed = false;
    await t.pumpWidget(host(peerConfirmed: true, onConfirm: () => confirmed = true));

    // Their tap is reported...
    expect(find.textContaining('They have confirmed'), findsOneWidget);
    // ...and changes nothing: the buttons are still the only way through, and
    // nothing has been decided here. R0-F4 needs both people, and a screen that
    // drifted toward "they said yes, so..." would be quietly undoing that.
    expect(find.text('They match'), findsOneWidget);
    expect(find.text(SasView.waiting), findsNothing);
    expect(confirmed, isFalse);
  });

  testWidgets('once we confirm, there is nothing left to press', (t) async {
    await t.pumpWidget(host(weConfirmed: true));
    // No second confirmation to give, and none to take back by tapping again.
    expect(find.text('They match'), findsNothing);
    expect(find.text("They don't match"), findsNothing);
    expect(find.text(SasView.waiting), findsOneWidget);
  });

  testWidgets('waiting still offers a way out', (t) async {
    var cancelled = false;
    await t.pumpWidget(host(weConfirmed: true, onCancel: () => cancelled = true));
    // Someone who confirms and then changes their mind — or realises the other
    // phone is showing something else — must not be stuck on this screen with
    // a ceremony running.
    await t.tap(find.text('Stop'));
    expect(cancelled, isTrue);
  });

  testWidgets('both answers reach the caller', (t) async {
    var confirmed = false;
    var cancelled = false;
    await t.pumpWidget(
      host(onConfirm: () => confirmed = true, onCancel: () => cancelled = true),
    );
    await t.tap(find.text('They match'));
    expect(confirmed, isTrue);
    await t.tap(find.text("They don't match"));
    expect(cancelled, isTrue);
  });
}
