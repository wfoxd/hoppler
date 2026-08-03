import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/src/ble/ble_channel.dart';
import 'package:hoppler/src/platform/host.dart';
import 'package:hoppler/src/rust/api/platform.dart';

/// Records what the adapter was asked to do, and fails on demand.
class _FakeRadio {
  final List<String> calls = [];
  final Set<String> failing = {};

  Future<Object?> handle(MethodCall call) async {
    calls.add(call.method);
    if (failing.contains(call.method)) {
      throw PlatformException(code: 'unavailable', message: 'radio is off');
    }
    return null;
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late _FakeRadio radio;
  late StreamController<HostCommand> commands;
  late StreamController<dynamic> radioEvents;
  late List<HostFact> reported;
  late HostDispatcher dispatcher;

  setUp(() async {
    radio = _FakeRadio();
    commands = StreamController<HostCommand>();
    radioEvents = StreamController<dynamic>.broadcast();
    reported = [];

    const method = MethodChannel('hoppler/ble');
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(method, radio.handle);

    dispatcher = HostDispatcher(
      commands: commands.stream,
      report: reported.add,
      ble: BleChannel(
        commands: method,
        events: _StubEvents(radioEvents.stream),
      ),
    );
    await dispatcher.start();
  });

  tearDown(() async {
    await dispatcher.stop();
    await commands.close();
    await radioEvents.close();
  });

  Future<void> issue(HostCommand c) async {
    commands.add(c);
    await Future<void>.delayed(Duration.zero);
  }

  // A command that never reaches the radio is invisible: the core believes it
  // was accepted, the radio does nothing, and the symptom is two phones that
  // cannot see each other with no error on either. That is the failure mode
  // this whole seam exists to make debuggable, so it gets checked one by one.
  test('every command reaches the radio', () async {
    await issue(const HostCommand.bleSetLocalId(localId: 'abc'));
    await issue(
      HostCommand.bleStartAdvertising(payload: Uint8List.fromList([1])),
    );
    await issue(const HostCommand.bleStopAdvertising());
    await issue(const HostCommand.bleStartScanning());
    await issue(const HostCommand.bleStopScanning());
    await issue(const HostCommand.bleConnect(peer: 'p'));
    await issue(HostCommand.bleSend(peer: 'p', bytes: Uint8List.fromList([2])));
    await issue(const HostCommand.bleDisconnect(peer: 'p'));
    await issue(const HostCommand.bleShutdown());

    expect(radio.calls, [
      'setLocalId',
      'startAdvertising',
      'stopAdvertising',
      'startScanning',
      'stopScanning',
      'connect',
      'send',
      'disconnect',
      'shutdown',
    ]);
  });

  // The core is not waiting on the future — the seam is one-way — so a throw
  // here would surface as an unhandled async error in the app while the core
  // waited for a fact that never came.
  test('a failing command is reported rather than thrown', () async {
    radio.failing.add('startAdvertising');
    await issue(
      HostCommand.bleStartAdvertising(payload: Uint8List.fromList([1])),
    );

    expect(reported, hasLength(1));
    expect(
      reported.single,
      isA<HostFact_BleAvailability>().having(
        (f) => f.available,
        'available',
        false,
      ),
    );
  });

  // A dial is the one command with a peer waiting on a specific answer. Told
  // only that the radio is unavailable, the core would leave that peer
  // half-open until something else disturbed it.
  test(
    'a dial that cannot be attempted fails that peer, not the radio',
    () async {
      radio.failing.add('connect');
      await issue(const HostCommand.bleConnect(peer: 'p'));

      expect(
        reported.single,
        isA<HostFact_BlePipeFailed>().having((f) => f.peer, 'peer', 'p'),
      );
    },
  );

  test('radio events are reported back as facts', () async {
    radioEvents.add(<String, Object?>{'type': 'pipeOpened', 'peer': 'p'});
    await Future<void>.delayed(Duration.zero);

    expect(
      reported.single,
      isA<HostFact_BlePipeOpened>().having((f) => f.peer, 'peer', 'p'),
    );
  });

  // Stopping has to be real. A dispatcher still forwarding commands after the
  // app let it go is the R0-F2 failure the Discovery switch exists to prevent,
  // one layer down.
  test('nothing is forwarded after stop', () async {
    await dispatcher.stop();
    commands.add(const HostCommand.bleStartScanning());
    await Future<void>.delayed(Duration.zero);

    expect(radio.calls, isEmpty);
  });
}

/// Feeds a stream in place of the platform's `EventChannel`.
class _StubEvents implements EventChannel {
  _StubEvents(this._stream);
  final Stream<dynamic> _stream;

  @override
  Stream<dynamic> receiveBroadcastStream([dynamic arguments]) => _stream;

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}
