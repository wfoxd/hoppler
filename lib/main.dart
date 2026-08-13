import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hoppler/features/nearby/nearby_tile.dart';
import 'package:hoppler/features/nearby/nearby_view.dart';
import 'package:hoppler/features/pairing/pairing_code_view.dart';
import 'package:hoppler/features/pairing/sas_view.dart';
import 'package:hoppler/features/pairing/scanner.dart';
import 'package:hoppler/features/ping/ping_service.dart';
import 'package:hoppler/src/rust/api/core.dart';
import 'package:hoppler/src/rust/api/discovery.dart';
import 'package:hoppler/src/rust/api/events.dart';
import 'package:hoppler/src/rust/api/messaging.dart';
import 'package:hoppler/src/rust/api/pairing.dart';
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

  /// The ceremony in progress, or null.
  ///
  /// One at a time, deliberately. Pairing is an in-person act with one other
  /// person; a second concurrent ceremony would put two sets of colours on one
  /// screen, which is the one thing this screen must never be ambiguous about.
  _Pairing? _pairing;
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
        // this is the log's record that the round trip closed. Named, because
        // two pings can be in flight and a bare "answered" says nothing about
        // which one came back.
        case CoreEvent_PingAcked(:final deviceId):
          _log.insert(0, 'Ping answered by ${_nameFor(deviceId)}');
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
        // The pairing screens are the next piece of work; until then these
        // land in the log, which is honest — a person can read the colours and
        // see what happened, but there is nothing here to start a ceremony with
        // or to confirm one, so pairing is not yet reachable from the UI.
        //
        // Deliberately not folded into a `default:`. This switch is exhaustive
        // on purpose: a new core event should fail to compile here rather than
        // be silently dropped, which is the only reason these four are being
        // written at all.
        case CoreEvent_PairingSas(:final deviceId, :final colours, :final word):
          _pairing = _Pairing(
            deviceId: deviceId,
            peerName: _nameFor(deviceId),
            colours: [
              for (final c in colours) SasColour(name: c.name, rgb: c.rgb),
            ],
            word: word,
          );
        case CoreEvent_PairingPeerConfirmed(:final deviceId):
          // Guarded on the device id: an event for a ceremony this screen is
          // not showing must not light up the one it is.
          if (_pairing?.deviceId == deviceId) {
            _pairing = _pairing!.copyWith(peerConfirmed: true);
          }
        case CoreEvent_PairingCompleted(:final name):
          _pairing = null;
          _log.insert(0, 'Paired with $name');
        case CoreEvent_PairingFailed(:final deviceId, :final reason):
          if (_pairing?.deviceId == deviceId) _pairing = null;
          _log.insert(0, 'Pairing failed: $reason');
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
          if (_pairing == null)
            OverflowBar(
              alignment: MainAxisAlignment.center,
              children: [
                TextButton.icon(
                  onPressed: _showMyCode,
                  icon: const Icon(Icons.qr_code_2),
                  label: const Text('Show my code'),
                ),
                // Hidden rather than disabled where there is no camera. A
                // greyed-out button invites people to work out what they did
                // wrong; the code screen says plainly why this device can only
                // do half the ceremony.
                if (canScanQrCodes)
                  TextButton.icon(
                    onPressed: _scanACode,
                    icon: const Icon(Icons.photo_camera),
                    label: const Text('Scan a code'),
                  ),
              ],
            ),
          if (_transfer != null)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16),
              child: LinearProgressIndicator(value: _transfer),
            ),
          Expanded(
            child: _pairing == null
                ? NearbyView<NearbyDevice>(
                    discoveryOn: _discovery,
                    radioReason: _radioReason,
                    devices: _devices,
                    tile: _deviceTile,
                  )
                // The colours take the screen while there are colours. A
                // comparison offered alongside a scrolling list of other
                // people is one nobody makes properly, and it is the only
                // thing on this screen that has to be decided.
                : SasView(
                    colours: _pairing!.colours,
                    word: _pairing!.word,
                    peerName: _pairing!.peerName,
                    peerConfirmed: _pairing!.peerConfirmed,
                    weConfirmed: _pairing!.weConfirmed,
                    onConfirm: _confirmPairing,
                    onCancel: _cancelPairing,
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

  /// A device's name if we have learned one, otherwise its id.
  ///
  /// The id is not pretty, but it is what distinguishes two answers — and a
  /// peer whose persona has not arrived yet has no name to show (their tile
  /// says so too).
  /// Put this device's code on screen until the sheet is dismissed.
  ///
  /// `stopShowingInvite` on the way out, always. A code left showing keeps a
  /// Layer-2 key paired with the rung id this device is advertising under, and
  /// R0-F2 spends twelve minutes at a time making that pairing hard to
  /// observe — leaving it up undoes that for as long as the screen is on.
  Future<void> _showMyCode() async {
    final String code;
    try {
      code = await pairingInvite();
    } catch (e) {
      setState(() => _log.insert(0, 'Could not make a code: $e'));
      return;
    }
    if (!mounted) return;
    await showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      builder: (sheet) => PairingCodeView(
        code: code,
        canScan: canScanQrCodes,
        onDone: () => Navigator.of(sheet).pop(),
      ),
    );
    await stopShowingInvite();
  }

  /// Open the camera and hand whatever it reads to the core.
  ///
  /// The camera reports every QR code in front of it, most of which are not
  /// ours, so a rejected code is the normal case and not worth a message. The
  /// guard is on `_starting`: `onScan` fires per frame, and without it a single
  /// code held in view starts a ceremony and then floods "already pairing"
  /// errors behind it.
  Future<void> _scanACode() async {
    var starting = false;
    await showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      builder: (sheet) => SizedBox(
        height: MediaQuery.of(sheet).size.height * 0.8,
        child: QrScannerView(
          onCancel: () => Navigator.of(sheet).pop(),
          onCode: (code) async {
            if (starting) return;
            starting = true;
            try {
              await beginPairing(code: code);
              if (sheet.mounted) Navigator.of(sheet).pop();
            } catch (_) {
              // Not ours, or the device is not nearby. Keep looking — the
              // person is holding a camera at a screen and the next frame is
              // a fresh chance.
              starting = false;
            }
          },
        ),
      ),
    );
  }

  Future<void> _confirmPairing() async {
    final pairing = _pairing;
    if (pairing == null) return;
    // Marked as ours *before* the call so the button cannot be pressed twice
    // while the core is working. The screen switches to "waiting", which is
    // also the honest description of what has just happened.
    setState(() => _pairing = pairing.copyWith(weConfirmed: true));
    try {
      await confirmPairing(deviceId: pairing.deviceId);
    } catch (e) {
      setState(() {
        _pairing = null;
        _log.insert(0, 'Pairing failed: $e');
      });
    }
  }

  Future<void> _cancelPairing() async {
    final pairing = _pairing;
    if (pairing == null) return;
    setState(() => _pairing = null);
    await cancelPairing(deviceId: pairing.deviceId);
  }

  String _nameFor(String deviceId) {
    for (final d in _devices) {
      if (d.deviceId == deviceId && d.name.isNotEmpty) return d.name;
    }
    return deviceId;
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


/// The ceremony this screen is showing.
///
/// Immutable and replaced rather than mutated, so a rebuild always draws a
/// consistent set of colours: a half-updated ceremony on the one screen whose
/// job is exact comparison would be the worst possible place for a torn read.
class _Pairing {
  const _Pairing({
    required this.deviceId,
    required this.peerName,
    required this.colours,
    required this.word,
    this.peerConfirmed = false,
    this.weConfirmed = false,
  });

  final String deviceId;
  final String peerName;
  final List<SasColour> colours;
  final String word;
  final bool peerConfirmed;
  final bool weConfirmed;

  _Pairing copyWith({bool? peerConfirmed, bool? weConfirmed}) => _Pairing(
    deviceId: deviceId,
    peerName: peerName,
    colours: colours,
    word: word,
    peerConfirmed: peerConfirmed ?? this.peerConfirmed,
    weConfirmed: weConfirmed ?? this.weConfirmed,
  );
}
