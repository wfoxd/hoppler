import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/src/nfc/nfc_channel.dart';

/// The Dart half of the tap channel, against events the adapter might send —
/// including the ones it should never send.
///
/// The point of the malformed cases is not tidiness. `Stream.map` that throws
/// kills the *stream*, not the event: one bad message and every later tap goes
/// silent, on the screen where silence is indistinguishable from not having
/// tapped at all. Two people would stand there touching phones together with
/// nothing happening and no way to tell why.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const channel = EventChannel('org.hoppler/nfc/events');

  /// Push a list of platform messages through the real channel and collect what
  /// comes out the Dart end.
  Future<List<NfcEvent>> decode(List<Object?> messages) async {
    const codec = StandardMethodCodec();
    final binding = TestDefaultBinaryMessengerBinding.instance;
    binding.defaultBinaryMessenger.setMockMessageHandler(channel.name, (
      message,
    ) async {
      final call = codec.decodeMethodCall(message);
      if (call.method == 'listen') {
        for (final m in messages) {
          await binding.defaultBinaryMessenger.handlePlatformMessage(
            channel.name,
            codec.encodeSuccessEnvelope(m),
            (_) {},
          );
        }
      }
      return codec.encodeSuccessEnvelope(null);
    });
    addTearDown(
      () => binding.defaultBinaryMessenger.setMockMessageHandler(
        channel.name,
        null,
      ),
    );
    return nfcEvents().take(messages.length).toList();
  }

  test('a code read off a tap arrives as text, unparsed', () async {
    final events = await decode([
      {'type': 'codeRead', 'value': 'HOPPLER://PAIR/MZXW6YTBOI'},
    ]);
    expect(events.single, isA<NfcCodeRead>());
    expect(
      (events.single as NfcCodeRead).code,
      'HOPPLER://PAIR/MZXW6YTBOI',
      reason:
          'the channel must hand the code on exactly as it arrived — deciding '
          'what it means is the core\'s job, and doing it here would be a '
          'second implementation of Invite::parse in the layer with no tests',
    );
  });

  test('a failed tap keeps its reason', () async {
    final events = await decode([
      {'type': 'unreadable', 'value': 'no code to read on that phone'},
    ]);
    expect(events.single, isA<NfcUnreadable>());
    expect(
      (events.single as NfcUnreadable).reason,
      'no code to read on that phone',
    );
  });

  test('rubbish does not kill the stream', () async {
    // Every one of these would have thrown out of the old `as Map` cast, and a
    // throw inside `map` ends the subscription: the first malformed event would
    // have been the last event of any kind.
    final events = await decode([
      'not a map',
      42,
      null,
      <String, Object?>{},
      {'type': 'codeRead'}, // a code with no code in it
      {'type': 'somethingNewer', 'value': 'from a future adapter'},
      {'type': 'codeRead', 'value': 'HOPPLER://PAIR/AA'},
    ]);

    expect(
      events, hasLength(7),
      reason: 'the stream died partway: only ${events.length} events survived',
    );
    // The last one still gets through, which is the whole point.
    expect(events.last, isA<NfcCodeRead>());
    // And everything before it is a failed tap rather than a dropped event.
    for (final event in events.take(6)) {
      expect(event, isA<NfcUnreadable>());
      expect((event as NfcUnreadable).reason, isNotEmpty);
    }
  });

  test('an empty code is a failed tap, not a code', () async {
    // Passed on, it would reach the core, be refused as a malformed invite, and
    // surface as nothing at all — a tap that silently did nothing.
    final events = await decode([
      {'type': 'codeRead', 'value': ''},
    ]);
    expect(events.single, isA<NfcUnreadable>());
  });
}
