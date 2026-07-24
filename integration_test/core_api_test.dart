import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/src/rust/api/core.dart';
import 'package:hoppler/src/rust/api/discovery.dart';
import 'package:hoppler/src/rust/api/events.dart';
import 'package:hoppler/src/rust/api/messaging.dart';
import 'package:hoppler/src/rust/api/transfers.dart';
import 'package:hoppler/src/rust/api/types.dart';
import 'package:hoppler/src/rust/frb_generated.dart';
import 'package:integration_test/integration_test.dart';

// Drives the real Core API over the bridge on a device, verifying the event
// stream carries discovery, message, and transfer events (Done-when #1).
void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  setUpAll(() async {
    await RustLib.init();
    final dir = Directory.systemTemp.createTempSync('hoppler-it');
    await coreInit(supportDir: dir.path);
  });

  test('discovery toggle emits DiscoveryUpdated with nearby devices', () async {
    final next = coreEventStream().firstWhere((e) => e is CoreEvent_DiscoveryUpdated);
    await setDiscovery(enabled: true);
    final event = await next as CoreEvent_DiscoveryUpdated;
    expect(event.devices, isNotEmpty);
    expect(event.devices.any((d) => d.name == 'Sam'), isTrue);
  });

  test('send_chat stores the message and emits a reply', () async {
    final reply = coreEventStream().firstWhere((e) => e is CoreEvent_MessageReceived);
    final sent = await sendChat(deviceId: 'fake-sam', text: 'hello');
    expect(sent.outgoing, isTrue);
    final event = await reply as CoreEvent_MessageReceived;
    expect(event.text, contains('hello'));
  });

  test('offer_drop completes over the event stream', () async {
    final done = coreEventStream().firstWhere((e) => e is CoreEvent_TransferCompleted);
    await offerDrop(deviceId: 'fake-sam', name: 'photo.jpg', size: BigInt.from(1000));
    final event = await done as CoreEvent_TransferCompleted;
    expect(event.success, isTrue);
  });
}
