import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hoppler/features/nearby/nearby_tile.dart';
import 'package:hoppler/features/nearby/nearby_view.dart';
import 'package:hoppler/features/ping/ping_service.dart';
import 'package:hoppler/src/rust/api/core.dart';
import 'package:hoppler/src/rust/api/discovery.dart';
import 'package:hoppler/src/rust/api/events.dart';
import 'package:hoppler/src/rust/api/messaging.dart';
import 'package:hoppler/src/rust/api/transfers.dart';
import 'package:hoppler/src/rust/api/types.dart';
import 'package:hoppler/src/platform/host.dart';
import 'package:hoppler/src/rust/frb_generated.dart';
import 'package:path_provider/path_provider.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  // Before coreInit, which is when the core builds its transports and issues
  // its first commands. The bridge holds anything sent before this, so being
  // late costs latency rather than correctness — but there is no reason to be.
  await HostDispatcher.overBridge().start();
  final dir = await getApplicationSupportDirectory();
  // A BLE-only build for the radio acceptance, selected at build time:
  //   flutter build apk --debug --dart-define=HOPPLER_RADIO=ble
  // Default stays LAN, which is what every hardware run so far exercised.
  // One rung at a time on purpose — with both running, a peer found over Wi-Fi
  // is indistinguishable from one found over the air.
  const radio = String.fromEnvironment('HOPPLER_RADIO') == 'ble'
      ? RadioChoice.ble
      : RadioChoice.lan;
  final persona = await coreInit(supportDir: dir.path, radio: radio);
  runApp(HopplerApp(persona: persona));
}

class HopplerApp extends StatelessWidget {
  const HopplerApp({super.key, required this.persona});
  final PersonaDto persona;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Hoppler',
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: Colors.orange),
      ),
      home: HomePage(persona: persona),
    );
  }
}

class HomePage extends StatefulWidget {
  const HomePage({super.key, required this.persona});
  final PersonaDto persona;

  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage> {
  bool _discovery = false;
  List<NearbyDevice> _devices = [];

  /// Why the radio cannot be used, or null when it can.
  ///
  /// Held apart from [_devices] because an empty list is what "the radio is
  /// off" and "nobody is nearby" both look like, and R0-F2 turns on the user
  /// being able to tell those apart.
  String? _radioReason;
  final List<String> _log = [];
  double? _transfer; // 0..1 while a Drop is in flight
  StreamSubscription<CoreEvent>? _events;
  late final PingService _pingService;

  @override
  void initState() {
    super.initState();
    // One broadcast stream shared by the app and the feature modules.
    final stream = coreEventStream().asBroadcastStream();
    _pingService = CorePingService(stream);
    _events = stream.listen(_onEvent);
  }

  @override
  void dispose() {
    _events?.cancel();
    super.dispose();
  }

  void _onEvent(CoreEvent event) {
    setState(() {
      switch (event) {
        case CoreEvent_DiscoveryUpdated(:final devices):
          _devices = devices;
        case CoreEvent_Pinged(:final name):
          _log.insert(0, 'Ping from $name');
        // The answer to one of ours. The button shows it too, via PingService;
        // this is the log's record that the round trip closed.
        case CoreEvent_PingAcked():
          _log.insert(0, 'Ping answered');
        // Only ever raised when we could not *reach* the device. A blocked
        // peer accepts the pipe and goes quiet, so it produces nothing here —
        // which is what keeps "blocked" indistinguishable from "not there".
        case CoreEvent_PingFailed(:final reason):
          _log.insert(0, 'Ping failed: $reason');
        case CoreEvent_MessageReceived(:final text):
          _log.insert(0, 'Message: $text');
        case CoreEvent_TransferProgress(:final received, :final total):
          _transfer = total == BigInt.zero ? 0 : received / total;
        case CoreEvent_TransferCompleted(:final success):
          _transfer = null;
          _log.insert(0, success ? 'Drop complete' : 'Drop failed');
        case CoreEvent_RadioChanged(:final available, :final reason):
          _radioReason = radioReasonFrom(available: available, reason: reason);
      }
    });
  }

  Color get _personaColour => Color(0xFF000000 | widget.persona.colour);

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        backgroundColor: _personaColour,
        title: Text('Hoppler — ${widget.persona.name}'),
      ),
      body: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SwitchListTile(
            title: const Text('Discovery'),
            subtitle: Text(_discovery ? 'Visible — who\'s nearby?' : 'Off'),
            value: _discovery,
            onChanged: (v) async {
              await setDiscovery(enabled: v);
              setState(() => _discovery = v);
            },
          ),
          if (_transfer != null)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16),
              child: LinearProgressIndicator(value: _transfer),
            ),
          Expanded(
            child: NearbyView<NearbyDevice>(
              radioReason: _radioReason,
              devices: _devices,
              tile: _deviceTile,
            ),
          ),
          const Divider(height: 1),
          SizedBox(
            height: 140,
            child: ListView(
              padding: const EdgeInsets.all(8),
              children: _log.map((l) => Text(l)).toList(),
            ),
          ),
        ],
      ),
    );
  }

  Widget _deviceTile(NearbyDevice d) {
    return NearbyTile(
      device: d,
      pingService: _pingService,
      onChat: (text) => _run(() => sendChat(deviceId: d.deviceId, text: text)),
      onDrop: () => _run(
        () => offerDrop(
          deviceId: d.deviceId,
          name: 'photo.jpg',
          size: BigInt.from(5000000),
        ),
      ),
    );
  }

  /// Run an API call, surfacing any failure in the log instead of leaving an
  /// unhandled async error.
  Future<void> _run(Future<Object?> Function() call) async {
    try {
      await call();
    } catch (e) {
      if (mounted) setState(() => _log.insert(0, 'Error: $e'));
    }
  }
}
