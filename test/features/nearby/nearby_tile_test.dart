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

NearbyDevice _device({
  required String name,
  int colour = 0x0088ff,
  bool paired = false,
  bool present = true,
}) => NearbyDevice(
  deviceId: 'dev-1',
  name: name,
  colour: colour,
  paired: paired,
  present: present,
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
  /// R0-F2 leaves "connections with paired people unaffected" by Discovery, so
  /// a paired person stays listed when the radio cannot see them. The tile then
  /// has to say which, or it claims somebody is in the room who is not.
  ///
  /// Measured on two phones: the Samsung went dark and Wren vanished from the
  /// Pixel seconds after they paired, leaving no way to write to her at all.
  testWidgets('a paired device that is away says so', (tester) async {
    await pump(
      tester,
      _device(name: 'Wren', paired: true, present: false),
    );
    expect(find.textContaining('away'), findsOneWidget);
    expect(
      find.text('paired'),
      findsNothing,
      reason: 'an absent peer read exactly like one standing next to you',
    );
  });

  /// And presence is not paired-ness. Both are shown, and confusing them makes
  /// the tile lie in one direction or the other.
  testWidgets('a paired device that is here does not say away', (tester) async {
    await pump(tester, _device(name: 'Wren', paired: true, present: true));
    expect(find.text('paired'), findsOneWidget);
    expect(find.textContaining('away'), findsNothing);
  });

  testWidgets('an unpaired device that is here still says nearby', (
    tester,
  ) async {
    await pump(tester, _device(name: 'Wren'));
    expect(find.text('nearby'), findsOneWidget);
  });

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
}
