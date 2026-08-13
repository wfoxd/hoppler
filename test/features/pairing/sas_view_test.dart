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

  testWidgets('lays out on a narrow phone, at any colour count', (t) async {
    // The claim above — that this survives a change to the colour count — is
    // only true if the layout survives it too.
    //
    // Six colours, which is past anything anyone has proposed, and that is the
    // point: four still fit a plain row on a 320dp screen, so a test at four
    // would pass whether or not the swatches could wrap and would have proved
    // nothing about the count being free. Mutation testing said exactly that.
    t.view.physicalSize = const Size(320, 640);
    t.view.devicePixelRatio = 1.0;
    addTearDown(t.view.reset);

    await t.pumpWidget(
      host(
        palette: const [
          SasColour(name: 'lavender', rgb: 0xDCBEFF),
          SasColour(name: 'magenta', rgb: 0xF032E6),
          SasColour(name: 'slate', rgb: 0xA9A9A9),
          SasColour(name: 'brown', rgb: 0x9A6324),
          SasColour(name: 'teal', rgb: 0x469990),
          SasColour(name: 'mint', rgb: 0xAAFFC3),
        ],
      ),
    );
    expect(t.takeException(), isNull);
    expect(find.text('lavender'), findsOneWidget);
    expect(find.text('mint'), findsOneWidget);
  });

  testWidgets('lays out at a large text scale', (t) async {
    // Someone who has turned text size up is exactly the person who needs the
    // colour *names*, so this is the case least able to afford an overflow.
    t.view.physicalSize = const Size(320, 640);
    t.view.devicePixelRatio = 1.0;
    addTearDown(t.view.reset);

    await t.pumpWidget(
      MediaQuery(
        data: const MediaQueryData(textScaler: TextScaler.linear(2.0)),
        child: host(
          palette: const [
            SasColour(name: 'lavender', rgb: 0xDCBEFF),
            SasColour(name: 'magenta', rgb: 0xF032E6),
            SasColour(name: 'slate', rgb: 0xA9A9A9),
          ],
        ),
      ),
    );
    expect(t.takeException(), isNull);
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
