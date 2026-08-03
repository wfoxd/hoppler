import 'dart:async';
import 'dart:typed_data';

import 'package:hoppler/src/ble/ble_channel.dart';
import 'package:hoppler/src/rust/api/platform.dart';

/// Runs the host half of the platform seam.
///
/// The core issues [HostCommand]s and expects [HostFact]s back. This does the
/// routing and nothing else: no retries, no state, no interpretation. Every
/// decision that could be made in Rust already was, and a dispatcher that
/// starts deciding things is a second implementation of the transport contract
/// in a language with no tests for it.
///
/// One command *is* handled here rather than passed on — see [_run] on why a
/// failed command becomes a fact instead of an exception.
class HostDispatcher {
  /// The seam is injected rather than reached for, so this is testable without
  /// the Rust bridge — the same reason `PingService` is a seam. A dispatcher
  /// that called `platformCommandStream()` directly could only be exercised on
  /// a device, which for the layer that historically carried the untested
  /// defects is the wrong way round.
  HostDispatcher({
    required Stream<HostCommand> commands,
    required void Function(HostFact fact) report,
    BleChannel? ble,
  }) : _commandStream = commands,
       _report_ = report,
       _ble = ble ?? BleChannel();

  /// Wired to the real bridge.
  factory HostDispatcher.overBridge({BleChannel? ble}) => HostDispatcher(
    commands: platformCommandStream(),
    report: (fact) => platformFact(fact: fact),
    ble: ble,
  );

  final Stream<HostCommand> _commandStream;
  final void Function(HostFact fact) _report_;
  final BleChannel _ble;
  StreamSubscription<HostCommand>? _commands;
  StreamSubscription<BleEvent>? _events;

  /// Begin serving the core.
  ///
  /// Called **before** `coreInit`. Commands issued before this are held by the
  /// bridge rather than dropped, so a late start costs latency and not
  /// correctness — but the core builds its transports during `coreInit`, and
  /// starting first is what keeps the first `BleSetLocalId` from waiting behind
  /// the rest of startup.
  Future<void> start() async {
    _events = _ble.events.listen(_onRadioEvent);
    _commands = _commandStream.listen(_dispatch);
  }

  Future<void> stop() async {
    await _commands?.cancel();
    await _events?.cancel();
    _commands = null;
    _events = null;
  }

  void _dispatch(HostCommand command) {
    switch (command) {
      case HostCommand_BleSetLocalId(:final localId):
        _run(() => _ble.setLocalId(localId));
      case HostCommand_BleStartAdvertising(:final payload):
        _run(() => _ble.startAdvertising(Uint8List.fromList(payload)));
      case HostCommand_BleStopAdvertising():
        _run(_ble.stopAdvertising);
      case HostCommand_BleStartScanning():
        _run(_ble.startScanning);
      case HostCommand_BleStopScanning():
        _run(_ble.stopScanning);
      case HostCommand_BleConnect(:final peer):
        // A dial that cannot even be attempted is still a dial that failed, and
        // the core is waiting on a fact for it. Reported rather than swallowed,
        // or the peer stays "connecting" until something else disturbs it.
        _run(
          () => _ble.connect(peer),
          onError: (why) => HostFact.blePipeFailed(peer: peer, why: why),
        );
      case HostCommand_BleSend(:final peer, :final bytes):
        _run(() => _ble.send(peer, Uint8List.fromList(bytes)));
      case HostCommand_BleDisconnect(:final peer):
        _run(() => _ble.disconnect(peer));
      case HostCommand_BleShutdown():
        _run(_ble.shutdown);
    }
  }

  /// Run a command, and never let its failure escape as an unhandled async
  /// error.
  ///
  /// The core is not waiting on this future — the seam is one-way by design —
  /// so throwing here would surface as an unhandled exception in the app while
  /// the core sat waiting for a fact that never came. Anything worth knowing
  /// goes back as a fact; the rest is reported as the radio being unavailable,
  /// which is a state the UI can already show.
  void _run(
    Future<void> Function() call, {
    HostFact Function(String why)? onError,
  }) {
    call().catchError((Object e) {
      final why = e is BleError ? e.message : e.toString();
      _report_(
        onError?.call(why) ??
            HostFact.bleAvailability(available: false, reason: why),
      );
    });
  }

  void _onRadioEvent(BleEvent event) {
    final fact = switch (event) {
      PeerFound(:final peer, :final payload) => HostFact.blePeerFound(
        peer: peer,
        payload: payload,
      ),
      PeerLost(:final peer) => HostFact.blePeerLost(peer: peer),
      PipeOpened(:final peer) => HostFact.blePipeOpened(peer: peer),
      PipeFailed(:final peer, :final why) => HostFact.blePipeFailed(
        peer: peer,
        why: why,
      ),
      PipeClosed(:final peer) => HostFact.blePipeClosed(peer: peer),
      Received(:final peer, :final bytes) => HostFact.bleReceived(
        peer: peer,
        bytes: bytes,
      ),
      Availability(:final available, :final reason) => HostFact.bleAvailability(
        available: available,
        reason: reason,
      ),
      WriteComplete(:final peer, :final bytes) => HostFact.bleWriteComplete(
        peer: peer,
        bytes: BigInt.from(bytes),
      ),
    };
    _report_(fact);
  }
}
