import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/ping/ping_button.dart';
import 'package:hoppler/features/ping/ping_service.dart';

/// Pure-Dart fake of the Core API seam — lets the module be widget-tested in
/// `flutter test` without the Rust bridge.
class FakePingService implements PingService {
  final _acks = StreamController<String>.broadcast();
  final List<String> pinged = [];
  bool fail = false;

  @override
  Stream<String> get acks => _acks.stream;

  @override
  Future<void> ping(String deviceId) async {
    if (fail) throw Exception('unreachable');
    pinged.add(deviceId);
  }

  void ack(String deviceId) => _acks.add(deviceId);
  Future<void> dispose() => _acks.close();
}

void main() {
  late FakePingService svc;

  setUp(() {
    svc = FakePingService();
    addTearDown(svc.dispose);
  });

  Widget host(String deviceId) => MaterialApp(
        home: Scaffold(body: PingButton(key: ValueKey(deviceId), service: svc, deviceId: deviceId)),
      );

  testWidgets('idle → pinging → acked → idle', (tester) async {
    await tester.pumpWidget(host('d1'));
    expect(find.byIcon(Icons.waving_hand_outlined), findsOneWidget);

    await tester.tap(find.byType(IconButton));
    await tester.pump();
    expect(svc.pinged, ['d1']);
    expect(find.byIcon(Icons.more_horiz), findsOneWidget);

    svc.ack('d1');
    await tester.pump();
    expect(find.byIcon(Icons.check_circle), findsOneWidget);

    // Acked state auto-resets to idle.
    await tester.pump(const Duration(seconds: 2));
    expect(find.byIcon(Icons.waving_hand_outlined), findsOneWidget);
  });

  testWidgets('an ack for a different device is ignored', (tester) async {
    await tester.pumpWidget(host('d1'));
    await tester.tap(find.byType(IconButton));
    await tester.pump();

    svc.ack('other');
    await tester.pump();
    expect(find.byIcon(Icons.more_horiz), findsOneWidget); // still pinging
  });

  testWidgets('a failed ping shows a snackbar and returns to idle', (tester) async {
    svc.fail = true;
    await tester.pumpWidget(host('d1'));
    await tester.tap(find.byType(IconButton));
    await tester.pump(); // begin
    await tester.pump(); // the async ping rejects
    expect(find.byIcon(Icons.waving_hand_outlined), findsOneWidget);
    expect(find.textContaining('Ping failed'), findsOneWidget);
  });

  testWidgets('while pinging, tapping again does not send a second ping', (tester) async {
    await tester.pumpWidget(host('d1'));
    await tester.tap(find.byType(IconButton));
    await tester.pump();
    // Button is disabled in the pinging phase.
    await tester.tap(find.byType(IconButton), warnIfMissed: false);
    await tester.pump();
    expect(svc.pinged, ['d1']);
  });

  testWidgets('no ack: the watchdog reverts pinging to idle', (tester) async {
    await tester.pumpWidget(host('d1'));
    await tester.tap(find.byType(IconButton));
    await tester.pump();
    expect(find.byIcon(Icons.more_horiz), findsOneWidget);
    // No ack arrives; after the timeout the button recovers instead of hanging.
    await tester.pump(const Duration(seconds: 5));
    expect(find.byIcon(Icons.waving_hand_outlined), findsOneWidget);
  });

  testWidgets('a recycled state resets when its deviceId changes', (tester) async {
    // Same key/slot, first device pinged and acked.
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: PingButton(key: const ValueKey('slot'), service: svc, deviceId: 'd1'),
        ),
      ),
    );
    await tester.tap(find.byType(IconButton));
    await tester.pump();
    svc.ack('d1');
    await tester.pump();
    expect(find.byIcon(Icons.check_circle), findsOneWidget);

    // The slot is reused for a different device — must start clean, and a stale
    // ack for the old device must not affect it.
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: PingButton(key: const ValueKey('slot'), service: svc, deviceId: 'd2'),
        ),
      ),
    );
    expect(find.byIcon(Icons.waving_hand_outlined), findsOneWidget);
    svc.ack('d1');
    await tester.pump();
    expect(find.byIcon(Icons.waving_hand_outlined), findsOneWidget);
  });

  testWidgets('disposing mid-ping does not throw when a late ack arrives', (tester) async {
    await tester.pumpWidget(host('d1'));
    await tester.tap(find.byType(IconButton));
    await tester.pump();

    // Dispose the button, then deliver a late ack + let timers elapse.
    await tester.pumpWidget(const MaterialApp(home: Scaffold(body: SizedBox())));
    svc.ack('d1');
    await tester.pump(const Duration(seconds: 5));
    // No exception = pass.
  });
}
