import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hoppler/features/ping/ping_button.dart';
import 'package:hoppler/features/ping/ping_service.dart';
import 'package:hoppler/src/rust/api/core.dart';
import 'package:hoppler/src/rust/api/discovery.dart';
import 'package:hoppler/src/rust/api/events.dart';
import 'package:hoppler/src/rust/api/messaging.dart';
import 'package:hoppler/src/rust/api/transfers.dart';
import 'package:hoppler/src/rust/api/types.dart';
import 'package:hoppler/src/rust/frb_generated.dart';
import 'package:path_provider/path_provider.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  final dir = await getApplicationSupportDirectory();
  final persona = await coreInit(supportDir: dir.path);
  runApp(HopplerApp(persona: persona));
}

class HopplerApp extends StatelessWidget {
  const HopplerApp({super.key, required this.persona});
  final PersonaDto persona;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Hoppler',
      theme: ThemeData(colorScheme: ColorScheme.fromSeed(seedColor: Colors.orange)),
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
          _log.insert(0, 'Ping ↔ $name');
        case CoreEvent_MessageReceived(:final text):
          _log.insert(0, 'Message: $text');
        case CoreEvent_TransferProgress(:final received, :final total):
          _transfer = total == BigInt.zero ? 0 : received / total;
        case CoreEvent_TransferCompleted(:final success):
          _transfer = null;
          _log.insert(0, success ? 'Drop complete' : 'Drop failed');
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
            child: _devices.isEmpty
                ? const Center(child: Text('No one nearby. Turn on Discovery.'))
                : ListView(children: _devices.map(_deviceTile).toList()),
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
    return ListTile(
      leading: CircleAvatar(backgroundColor: Color(0xFF000000 | d.colour)),
      title: Text(d.name),
      subtitle: Text(d.paired ? 'paired' : 'nearby'),
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          PingButton(
            key: ValueKey(d.deviceId),
            service: _pingService,
            deviceId: d.deviceId,
          ),
          IconButton(
            icon: const Icon(Icons.chat_bubble_outline),
            tooltip: 'Chat',
            onPressed: () => _run(() => sendChat(deviceId: d.deviceId, text: 'Hey ${d.name}!')),
          ),
          IconButton(
            icon: const Icon(Icons.upload_file_outlined),
            tooltip: 'Drop',
            onPressed: () => _run(
              () => offerDrop(deviceId: d.deviceId, name: 'photo.jpg', size: BigInt.from(5000000)),
            ),
          ),
        ],
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
