
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/src/ble/ble_channel.dart';

/// The Dart client carries the BLE contract and decides nothing, so these tests
/// are about faithfulness: every error code routes, every event decodes, and
/// the two failure modes an OEM stack actually produces — a malformed frame and
/// a missing plugin — degrade instead of taking the stream down.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const method = MethodChannel('test/ble');
  const event = EventChannel('test/ble/events');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  BleChannel channelWith(Future<Object?>? Function(MethodCall) handler) {
    messenger.setMockMethodCallHandler(method, handler);
    return BleChannel(commands: method, events: event);
  }

  tearDown(() => messenger.setMockMethodCallHandler(method, null));

  group('commands', () {
    test('arguments reach the adapter in the documented shape', () async {
      final calls = <MethodCall>[];
      final ble = channelWith((call) async {
        calls.add(call);
        return null;
      });

      final payload = Uint8List.fromList([1, 2, 3]);
      await ble.setLocalId('node-a');
      await ble.startAdvertising(payload);
      await ble.send('peer-b', Uint8List.fromList([9]));
      await ble.connect('peer-b');
      await ble.stopAdvertising();

      expect(calls.map((c) => c.method), [
        'setLocalId',
        'startAdvertising',
        'send',
        'connect',
        'stopAdvertising',
      ]);
      expect(calls[0].arguments, {'localId': 'node-a'});
      expect(calls[1].arguments, {'payload': payload});
      expect(calls[2].arguments['peer'], 'peer-b');
      expect(calls[4].arguments, isNull);
    });

    test(
      'a version mismatch is fatal at startup, not a mystery later',
      () async {
        final ble = channelWith((call) async => 2);
        await expectLater(
          ble.checkVersion(),
          throwsA(
            isA<BleError>()
                .having((e) => e.code, 'code', BleErrorCode.unavailable)
                .having((e) => e.message, 'message', contains('v2')),
          ),
        );

        final ok = channelWith((call) async => bleChannelVersion);
        await expectLater(ok.checkVersion(), completes);
      },
    );

    test('every documented error code routes to its own branch', () async {
      const codes = {
        'unavailable': BleErrorCode.unavailable,
        'no_such_peer': BleErrorCode.noSuchPeer,
        'payload_too_large': BleErrorCode.payloadTooLarge,
        'would_block': BleErrorCode.wouldBlock,
        'io': BleErrorCode.io,
      };
      for (final entry in codes.entries) {
        final ble = channelWith(
          (call) async =>
              throw PlatformException(code: entry.key, message: 'x'),
        );
        await expectLater(
          ble.stopAdvertising(),
          throwsA(
            isA<BleError>().having((e) => e.code, entry.key, entry.value),
          ),
        );
      }
    });

    test(
      'an unrecognised code degrades to io rather than escaping untyped',
      () async {
        final ble = channelWith(
          (call) async =>
              throw PlatformException(code: 'brand_new', message: 'x'),
        );
        await expectLater(
          ble.stopAdvertising(),
          throwsA(
            isA<BleError>().having((e) => e.code, 'code', BleErrorCode.io),
          ),
        );
      },
    );

    test('no adapter on this platform reads as unavailable', () async {
      // Desktop builds and hot restarts both land here. Anything other than
      // unavailable would surface as a crash rather than "Bluetooth is off".
      final ble = BleChannel(
        commands: const MethodChannel('test/ble/unregistered'),
        events: event,
      );
      await expectLater(
        ble.startScanning(),
        throwsA(
          isA<BleError>().having(
            (e) => e.code,
            'code',
            BleErrorCode.unavailable,
          ),
        ),
      );
    });
  });

  group('events', () {
    /// Push raw frames through the real EventChannel codec, so the decoder is
    /// tested against what the platform actually sends.
    Stream<BleEvent> decoded(List<Object?> frames) {
      messenger.setMockStreamHandler(event, _Frames(frames));
      return BleChannel(commands: method, events: event).events;
    }

    test('every documented event type decodes', () async {
      final events = await decoded([
        {
          'type': 'peerFound',
          'peer': 'a',
          'payload': Uint8List.fromList([7]),
        },
        {'type': 'peerLost', 'peer': 'a'},
        {'type': 'pipeOpened', 'peer': 'a'},
        {'type': 'pipeFailed', 'peer': 'a', 'why': 'refused'},
        {'type': 'pipeClosed', 'peer': 'a'},
        {
          'type': 'received',
          'peer': 'a',
          'bytes': Uint8List.fromList([1, 2]),
        },
        {'type': 'availability', 'available': false, 'reason': 'bluetooth off'},
        {'type': 'writeComplete', 'peer': 'a', 'bytes': 42},
      ]).toList();

      expect(events, hasLength(8));
      expect((events[0] as PeerFound).payload, [7]);
      expect((events[3] as PipeFailed).why, 'refused');
      expect((events[5] as Received).bytes, [1, 2]);
      expect((events[6] as Availability).reason, 'bluetooth off');
      expect((events[7] as WriteComplete).bytes, 42);
    });

    test('a malformed frame is dropped, not fatal to the stream', () async {
      // One bad frame from an OEM stack must not blind the core to every
      // sighting that follows.
      final events = await decoded([
        {'type': 'peerFound'}, // no peer
        'not a map',
        {'type': 'somethingNewer', 'peer': 'a'}, // forward compatibility (§3)
        {'type': 'pipeOpened', 'peer': 'survivor'},
      ]).toList();

      expect(events, hasLength(1));
      expect((events.single as PipeOpened).peer, 'survivor');
    });

    test('a missing why or reason falls back rather than throwing', () async {
      final events = await decoded([
        {'type': 'pipeFailed', 'peer': 'a'},
        {'type': 'availability', 'available': true},
      ]).toList();

      expect((events[0] as PipeFailed).why, 'unspecified');
      expect((events[1] as Availability).reason, isNull);
    });
  });
}

/// Replays canned frames onto an EventChannel.
class _Frames extends MockStreamHandler {
  _Frames(this.frames);
  final List<Object?> frames;

  @override
  void onListen(Object? arguments, MockStreamHandlerEventSink events) {
    for (final f in frames) {
      events.success(f);
    }
    events.endOfStream();
  }

  @override
  void onCancel(Object? arguments) {}
}
