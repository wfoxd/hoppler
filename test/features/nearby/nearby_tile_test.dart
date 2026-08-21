import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/nearby/nearby_tile.dart';
import 'package:hoppler/features/ping/ping_service.dart';
import 'package:hoppler/src/rust/api/types.dart';

class _StubPingService implements PingService {
  final _acks = StreamController<String>.broadcast();

  @override
  Stream<String> get acks => _acks.stream;

  @override
  Future<void> ping(String deviceId) async {}
}

NearbyDevice _device({required String name, int colour = 0x0088ff}) => NearbyDevice(
  deviceId: 'dev-1',
  threadId: null,
  name: name,
  colour: colour,
  paired: false,
);

/// Someone we have paired with who is not in range: no transport handle, and a
/// conversation that outlives every id they ever advertised under.
NearbyDevice _away({required String name}) => NearbyDevice(
  deviceId: null,
  threadId: 7,
  name: name,
  colour: 0x0088ff,
  paired: true,
);

void main() {
  Future<List<String>> pump(WidgetTester tester, NearbyDevice device) async {
    final chats = <String>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: NearbyTile(
            device: device,
            pingService: _StubPingService(),
            onChat: chats.add,
            onDrop: () {},
          ),
        ),
      ),
    );
    return chats;
  }

  // A device is listed before anyone knows who it is: the advertisement carries
  // no persona on purpose, so a scanner in range learns only that something is
  // there. The name arrives with the session a Ping opens.
  //
  // That is the protocol working, but rendered as a blank line it reads as a
  // fault — it sent us hunting for one during the two-phone run.
  testWidgets('a device with no name yet says so, and says what to do', (tester) async {
    await pump(tester, _device(name: ''));

    expect(find.text('Unknown device'), findsOneWidget);
    expect(find.text('Ping to connect'), findsOneWidget);
    expect(find.text('nearby'), findsNothing);
  });

  testWidgets('a named device shows its name and presence', (tester) async {
    await pump(tester, _device(name: 'Margo'));

    expect(find.text('Margo'), findsOneWidget);
    expect(find.text('nearby'), findsOneWidget);
    expect(find.text('Unknown device'), findsNothing);
    expect(find.text('Ping to connect'), findsNothing);
  });

  // Colour arrives with the persona, so an unknown device has none. Left alone
  // that renders as flat black, which looks like a broken avatar rather than a
  // missing one.
  testWidgets('an unknown device is not drawn as flat black', (tester) async {
    await pump(tester, _device(name: '', colour: 0));

    final avatar = tester.widget<CircleAvatar>(find.byType(CircleAvatar));
    expect(avatar.backgroundColor, isNot(const Color(0xFF000000)));
  });

  // "Hey !" is what actually arrived on the far phone during the LAN run.
  testWidgets('chatting to an unnamed device does not greet an empty name', (tester) async {
    final chats = await pump(tester, _device(name: ''));

    await tester.tap(find.byTooltip('Chat'));
    expect(chats, ['Hey!']);
  });

  testWidgets('chatting to a named device greets them by name', (tester) async {
    final chats = await pump(tester, _device(name: 'Margo'));

    await tester.tap(find.byTooltip('Chat'));
    expect(chats, ['Hey Margo!']);
  });

  // R0-F2 rotates ids and R0-F4 makes pairing durable, so a paired friend is on
  // the list for most of the day without being reachable. The row has to say
  // which of those two things is true right now.
  testWidgets('an away friend is shown as away, not as nearby', (tester) async {
    await pump(tester, _away(name: 'Wren'));

    expect(find.text('Wren'), findsOneWidget);
    expect(find.textContaining('away'), findsOneWidget);
    expect(find.text('paired'), findsNothing);
    expect(find.text('nearby'), findsNothing);
  });

  // Chat keeps working because R0-F5 says what happens to it: kept, and
  // delivered when they next meet. Ping and Drop cannot be kept — a nudge
  // answered tomorrow is not a nudge — so they are visibly unavailable rather
  // than live buttons that quietly do nothing.
  testWidgets('an away friend can be written to but not pinged or dropped', (
    tester,
  ) async {
    final chats = await pump(tester, _away(name: 'Wren'));

    await tester.tap(find.byTooltip('Chat'));
    await tester.pump();
    expect(chats, ['Hey Wren!'], reason: 'writing to an away friend was lost');

    // Both Ping and Drop, found by the reason they carry rather than by
    // position, so a reordered row still tests the right two buttons.
    final blocked = tester
        .widgetList<IconButton>(find.byType(IconButton))
        .where((b) => b.tooltip == NearbyTile.awayHint)
        .toList();
    expect(
      blocked.length,
      2,
      reason: 'Ping and Drop should both be unavailable, and say why',
    );
    for (final b in blocked) {
      expect(b.onPressed, isNull, reason: 'an away row offered a live tap');
    }
  });
}
