import 'package:hoppler/src/rust/api/messaging.dart' as api;
import 'package:hoppler/src/rust/api/types.dart';

/// The Ping gesture as the UI consumes it — a thin seam over the Core API so
/// the module is unit-testable without the Rust bridge (tests inject a fake).
/// It imports only `src/rust/api/`, honouring the P3 boundary.
abstract interface class PingService {
  /// Send a ping to a device; throws if it isn't reachable.
  Future<void> ping(String deviceId);

  /// Acknowledgements as device ids — a ping is acked when its id appears here.
  Stream<String> get acks;
}

/// The real service, backed by the Core API. `acks` is derived from the core
/// event stream (passed in so the whole app shares one subscription).
class CorePingService implements PingService {
  CorePingService(Stream<CoreEvent> events)
      : assert(
          events.isBroadcast,
          'pass a broadcast stream — each PingButton opens its own acks listener',
        ),
        acks = events
            .where((e) => e is CoreEvent_Pinged)
            .cast<CoreEvent_Pinged>()
            .map((e) => e.deviceId);

  @override
  final Stream<String> acks;

  @override
  Future<void> ping(String deviceId) => api.ping(deviceId: deviceId);
}
