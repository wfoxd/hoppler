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
    if (fail) throw 'unreachable';
    pinged.add(deviceId);
  }

  void ack(String deviceId) => _acks.add(deviceId);
}

Widget _host(PingService svc, String deviceId) =>
    MaterialApp(home: Scaffold(body: PingButton(service: svc, deviceId: deviceId)));

void main() {
  testWidgets('idle → pinging → acked', (tester) async {
    final svc = FakePingService();
    await tester.pumpWidget(_host(svc, 'd1'));
    expect(find.byIcon(Icons.waving_hand_outlined), findsOneWidget);

    await tester.tap(find.byType(IconButton));
    await tester.pump();
    expect(svc.pinged, ['d1']);
    expect(find.byIcon(Icons.more_horiz), findsOneWidget);

    svc.ack('d1');
    await tester.pump();
    expect(find.byIcon(Icons.check_circle), findsOneWidget);
  });

  testWidgets('an ack for a different device is ignored', (tester) async {
    final svc = FakePingService();
    await tester.pumpWidget(_host(svc, 'd1'));
    await tester.tap(find.byType(IconButton));
    await tester.pump();

    svc.ack('other');
    await tester.pump();
    expect(find.byIcon(Icons.more_horiz), findsOneWidget); // still pinging
  });

  testWidgets('a failed ping shows a snackbar and returns to idle', (tester) async {
    final svc = FakePingService()..fail = true;
    await tester.pumpWidget(_host(svc, 'd1'));
    await tester.tap(find.byType(IconButton));
    await tester.pump(); // begin
    await tester.pump(); // the async ping rejects
    expect(find.byIcon(Icons.waving_hand_outlined), findsOneWidget);
    expect(find.textContaining('Ping failed'), findsOneWidget);
  });
}
