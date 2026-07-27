/// The Dart half of the BLE platform channel (T08b).
///
/// Carries the contract in `docs/BLE_CHANNEL.md` and adds nothing to it. This
/// layer decides no behaviour: it turns commands into method calls and platform
/// facts into typed events, so that every rule lives either in Rust (where it is
/// tested) or in the adapter (where it must be), and never here in between.
library;

import 'dart:async';

import 'package:flutter/services.dart';

/// Contract version. Must match the adapter's `version`; see
/// `docs/BLE_CHANNEL.md`.
const int bleChannelVersion = 1;

const MethodChannel _commands = MethodChannel('org.hoppler/ble');
const EventChannel _events = EventChannel('org.hoppler/ble/events');

/// An error code from the channel's closed set (§4). Anything unrecognised
/// becomes [BleErrorCode.io], so a newer adapter cannot make an older core
/// throw something it has no branch for.
enum BleErrorCode {
  unavailable,
  noSuchPeer,
  payloadTooLarge,
  wouldBlock,
  io;

  static BleErrorCode parse(String? code) => switch (code) {
    'unavailable' => BleErrorCode.unavailable,
    'no_such_peer' => BleErrorCode.noSuchPeer,
    'payload_too_large' => BleErrorCode.payloadTooLarge,
    'would_block' => BleErrorCode.wouldBlock,
    _ => BleErrorCode.io,
  };
}

/// A command the adapter refused.
class BleError implements Exception {
  BleError(this.code, this.message);
  final BleErrorCode code;
  final String message;

  @override
  String toString() => 'BleError(${code.name}: $message)';
}

/// A fact from the radio (§3). Sealed so the core's handler cannot silently
/// miss a case when the contract grows.
sealed class BleEvent {
  const BleEvent();
}

class PeerFound extends BleEvent {
  const PeerFound(this.peer, this.payload);
  final String peer;
  final Uint8List payload;
}

class PeerLost extends BleEvent {
  const PeerLost(this.peer);
  final String peer;
}

class PipeOpened extends BleEvent {
  const PipeOpened(this.peer);
  final String peer;
}

class PipeFailed extends BleEvent {
  const PipeFailed(this.peer, this.why);
  final String peer;
  final String why;
}

class PipeClosed extends BleEvent {
  const PipeClosed(this.peer);
  final String peer;
}

class Received extends BleEvent {
  const Received(this.peer, this.bytes);
  final String peer;
  final Uint8List bytes;
}

class Availability extends BleEvent {
  const Availability(this.available, this.reason);
  final bool available;
  final String? reason;
}

class WriteComplete extends BleEvent {
  const WriteComplete(this.peer, this.bytes);
  final String peer;
  final int bytes;
}

/// The adapter, as the core sees it.
class BleChannel {
  BleChannel({MethodChannel? commands, EventChannel? events})
    : _method = commands ?? _commands,
      _event = events ?? _events;

  final MethodChannel _method;
  final EventChannel _event;

  /// Fails fast if the adapter speaks a different version of the contract. A
  /// mismatch is a build-time mistake; discovering it as a misbehaving radio
  /// hours later is the outcome this prevents.
  Future<void> checkVersion() async {
    final int? version = await _invoke<int>('version');
    if (version != bleChannelVersion) {
      throw BleError(
        BleErrorCode.unavailable,
        'BLE adapter speaks channel v$version, core expects v$bleChannelVersion',
      );
    }
  }

  /// How we are named. Sent at startup and on every rotation, whether or not
  /// we advertise — a node with Discovery off still dials paired peers and has
  /// to introduce itself (contract §5.6).
  Future<void> setLocalId(String localId) =>
      _invoke<void>('setLocalId', {'localId': localId});

  Future<void> startAdvertising(Uint8List payload) =>
      _invoke<void>('startAdvertising', {'payload': payload});

  Future<void> stopAdvertising() => _invoke<void>('stopAdvertising');

  Future<void> startScanning() => _invoke<void>('startScanning');

  Future<void> stopScanning() => _invoke<void>('stopScanning');

  Future<void> connect(String peer) => _invoke<void>('connect', {'peer': peer});

  Future<void> send(String peer, Uint8List bytes) =>
      _invoke<void>('send', {'peer': peer, 'bytes': bytes});

  Future<void> disconnect(String peer) =>
      _invoke<void>('disconnect', {'peer': peer});

  Future<void> shutdown() => _invoke<void>('shutdown');

  /// Facts from the radio. Malformed events are dropped rather than thrown:
  /// one bad frame from an OEM stack must not tear down the whole stream and
  /// leave the core blind.
  Stream<BleEvent> get events => _event
      .receiveBroadcastStream()
      .map(_decode)
      .where((e) => e != null)
      .cast<BleEvent>();

  Future<T?> _invoke<T>(String method, [Map<String, Object?>? args]) async {
    try {
      return await _method.invokeMethod<T>(method, args);
    } on PlatformException catch (e) {
      throw BleError(BleErrorCode.parse(e.code), e.message ?? e.code);
    } on MissingPluginException {
      // No adapter registered — a desktop build, or an app half-way through a
      // hot restart. Unavailable is exactly right and the UI already says so.
      throw BleError(
        BleErrorCode.unavailable,
        'no BLE adapter on this platform',
      );
    }
  }

  static BleEvent? _decode(Object? raw) {
    if (raw is! Map) return null;
    final map = raw.cast<Object?, Object?>();
    final type = map['type'];
    String? peer() => map['peer'] as String?;
    Uint8List bytes(String key) {
      final v = map[key];
      return v is Uint8List ? v : Uint8List(0);
    }

    return switch (type) {
      'peerFound' when peer() != null => PeerFound(peer()!, bytes('payload')),
      'peerLost' when peer() != null => PeerLost(peer()!),
      'pipeOpened' when peer() != null => PipeOpened(peer()!),
      'pipeFailed' when peer() != null => PipeFailed(
        peer()!,
        map['why'] as String? ?? 'unspecified',
      ),
      'pipeClosed' when peer() != null => PipeClosed(peer()!),
      'received' when peer() != null => Received(peer()!, bytes('bytes')),
      'availability' => Availability(
        map['available'] as bool? ?? false,
        map['reason'] as String?,
      ),
      'writeComplete' when peer() != null => WriteComplete(
        peer()!,
        map['bytes'] as int? ?? 0,
      ),
      // Unknown types are ignored by contract (§3), so a newer adapter can add
      // events without breaking an older core.
      _ => null,
    };
  }
}
