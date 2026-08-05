import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/features/ping/ping_service.dart';
import 'package:hoppler/src/rust/api/types.dart';

/// What `acks` is derived from, which is the whole of `CorePingService`.
///
/// Untested until now, and it was wrong: `acks` came from `CoreEvent_Pinged` —
/// someone nudging *us* — so a tap was only ever acknowledged when the other
/// person happened to nudge back. An ordinary ping timed out, and an unrelated
/// incoming one was mistaken for the answer to one's own.
void main() {
  late StreamController<CoreEvent> events;
  late PingService service;

  setUp(() {
    events = StreamController<CoreEvent>.broadcast();
    service = CorePingService(events.stream);
  });

  tearDown(() => events.close());

  test('the answer to our ping is an ack', () async {
    final acked = expectLater(service.acks, emits('peer-one'));
    events.add(const CoreEvent.pingAcked(deviceId: 'peer-one'));
    await acked;
  });

  test('someone nudging us is not an ack', () async {
    // The regression this file exists for. A ping arriving from a peer says
    // nothing about whether ours got there.
    final seen = <String>[];
    final sub = service.acks.listen(seen.add);

    events.add(const CoreEvent.pinged(deviceId: 'peer-one', name: 'Alice'));
    await Future<void>.delayed(Duration.zero);

    expect(
      seen,
      isEmpty,
      reason: 'an incoming ping was counted as the answer to one of ours',
    );
    await sub.cancel();
  });

  test('an ack names the device it came from', () async {
    // Two pings can be in flight at once; a bare "something was answered"
    // would let one peer's answer clear another peer's button.
    final seen = <String>[];
    final sub = service.acks.listen(seen.add);

    events.add(const CoreEvent.pingAcked(deviceId: 'peer-one'));
    events.add(const CoreEvent.pingAcked(deviceId: 'peer-two'));
    await Future<void>.delayed(Duration.zero);

    expect(seen, ['peer-one', 'peer-two']);
    await sub.cancel();
  });
}
