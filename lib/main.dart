import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hoppler/features/nearby/nearby_tile.dart';
import 'package:hoppler/features/onboarding/name_view.dart';
import 'package:hoppler/features/nearby/nearby_view.dart';
import 'package:hoppler/features/pairing/pairing_code_view.dart';
import 'package:hoppler/features/pairing/pairing_surface.dart';
import 'package:hoppler/features/pairing/sas_view.dart';
import 'package:hoppler/features/pairing/scanner.dart';
import 'package:hoppler/features/ping/ping_service.dart';
import 'package:hoppler/src/rust/api/core.dart';
import 'package:hoppler/src/rust/api/discovery.dart';
import 'package:hoppler/src/rust/api/events.dart';
import 'package:hoppler/src/rust/api/identity.dart';
import 'package:hoppler/src/rust/api/messaging.dart';
import 'package:hoppler/src/rust/api/pairing.dart';
import 'package:hoppler/src/rust/api/transfers.dart';
import 'package:hoppler/src/rust/api/types.dart';
import 'package:hoppler/src/nfc/nfc_channel.dart';
import 'package:hoppler/src/platform/host.dart';
import 'package:hoppler/src/platform/permissions.dart';
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
  // Only when it is going to be shown. The palette is a constant in the core,
  // so this is one bridge call, but a launch that is not asking anything has
  // no reason to make it.
  final palette = persona.needsName ? await personaColours() : const <PersonaColourDto>[];
  runApp(HopplerApp(persona: persona, palette: palette));
}

class HopplerApp extends StatefulWidget {
  const HopplerApp({super.key, required this.persona, required this.palette});
  final PersonaDto persona;
  final List<PersonaColourDto> palette;

  @override
  State<HopplerApp> createState() => _HopplerAppState();
}

class _HopplerAppState extends State<HopplerApp> {
  late PersonaDto _persona;

  @override
  void initState() {
    super.initState();
    _persona = widget.persona;
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Hoppler',
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: Colors.orange),
      ),
      // The name step stands in front of the app rather than over it. A device
      // with no chosen name would otherwise be discoverable as "Me" while its
      // owner is still deciding, and R0-F1's answer to who this is would be
      // decided by how long they took.
      home: _persona.needsName
          ? NameView(
              colour: Color(0xFF000000 | _persona.colour),
              palette: widget.palette,
              onChosen: (name, colour) async {
                final chosen = await updatePersona(name: name, colour: colour);
                // Only after the core has stored it. `updatePersona` writes
                // before it makes the name live and throws if it cannot, so
                // reaching here means the next launch will agree with this one.
                setState(() => _persona = chosen);
              },
            )
          : HomePage(persona: _persona),
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

  /// This device's own code, while it is on screen.
  ///
  /// Held as state rather than pushed as a route. It was a modal sheet, and on
  /// two phones that meant the person showing the code reached the colours and
  /// never saw them — their own QR was still on top of the page drawing them.
  /// A route cannot be reasoned about from the body's state; see
  /// [PairingSurface].
  String? _myCode;

  /// Whether the camera is open. Same reason.
  bool _scanning = false;

  /// Guards `onScan` firing once per camera frame — and a tap arriving in the
  /// middle of one.
  bool _startingCeremony = false;

  /// Whether this device can pair by tap at all. Asked once, at startup.
  bool _canTap = false;
  StreamSubscription<NfcEvent>? _taps;
  StreamSubscription<CoreEvent>? _events;
  late final PingService _pingService;

  @override
  void initState() {
    super.initState();
    // One broadcast stream shared by the app and the feature modules.
    final stream = coreEventStream().asBroadcastStream();
    _pingService = CorePingService(stream);
    _events = stream.listen(_onEvent);
    // Asked rather than assumed, and *before* listening. A "tap to pair" line on
    // a phone with no NFC is an instruction nobody can follow — R0-N6's rule
    // about stating reach honestly, applied to a second radio — but the reason
    // the subscription waits is harder: on a platform where the plugin is not
    // registered at all, which is every desktop build, listening to the event
    // channel throws `MissingPluginException` and takes the app down at
    // startup. `nfcAvailable` already answers false there; nothing may listen
    // until it has.
    nfcAvailable().then((yes) {
      if (!mounted || !yes) return;
      setState(() => _canTap = true);
      _taps = nfcEvents().listen(_onTap);
    });
  }

  /// A code read off another phone, or a tap that produced nothing.
  ///
  /// Deliberately the same path as the camera's: a tap yields text, and every
  /// question about what that text *is* has an answer in the core already.
  /// Two front doors, one hallway.
  void _onTap(NfcEvent event) {
    switch (event) {
      case NfcCodeRead(:final code):
        _onCode(code);
      case NfcUnreadable(:final reason):
        // Said out loud. Two people holding phones together need to know it did
        // not take — silence here is indistinguishable from not having tapped.
        setState(() => _log.insert(0, reason));
    }
  }

  @override
  void dispose() {
    _events?.cancel();
    _taps?.cancel();
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
          // Whatever else was on screen, the colours replace it — and if this
          // device was showing a code, that code is now spent and comes off
          // the air. `PairingSurface` would draw the colours over it anyway;
          // this is what stops the advertising.
          if (_myCode != null) unawaited(_stopShowingCode());
          if (_scanning && _canTap) unawaited(stopReadingTaps());
          _scanning = false;
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
    // Back closes whatever pairing surface is open, instead of leaving the app.
    //
    // This is the cost of holding those surfaces as state rather than routes,
    // and it has to be paid explicitly: with a modal sheet the system back
    // popped the sheet and disposed the camera inside the app, and without one
    // the same gesture exits — with the camera still delivering frames into an
    // engine that has detached. That is a hard crash, and it is what a person
    // does the moment a code will not decode.
    //
    // Backing out of the colours cancels the ceremony, which is the right
    // reading of the gesture: someone leaving that screen has not agreed to
    // anything.
    return PopScope(
      canPop: _pairing == null && _myCode == null && !_scanning,
      onPopInvokedWithResult: (didPop, _) {
        if (didPop) return;
        if (_pairing != null) {
          _cancelPairing();
        } else if (_scanning) {
          _stopScanning();
        } else if (_myCode != null) {
          _stopShowingCode();
        }
      },
      child: _scaffold(context),
    );
  }

  Widget _scaffold(BuildContext context) {
    // The camera gets the whole screen, with nothing above or below it.
    //
    // Not a style choice. The scanner's overlay sizes its cutout from the
    // screen while the decoder reads the centre of the camera image, so any
    // chrome sharing the page slides the drawn rectangle away from the region
    // actually being read — and a person aims at the rectangle. Found on two
    // phones, by aiming carefully at a box and having nothing happen.
    if (_scanning) {
      return Scaffold(
        body: QrScannerView(onCode: _onCode, onCancel: _stopScanning),
      );
    }
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
              // Asked here, which is the moment somebody said they wanted to be
              // found. Turning *off* asks nothing: withdrawing does not need a
              // permission, and prompting to stop being discoverable would be
              // absurd.
              if (v && !await ensureRadioPermission()) {
                // Declined. The switch goes back rather than sitting on while
                // nothing works — an "on" that discovers nobody is the failure
                // this app cannot tell apart from an empty room.
                if (!mounted) return;
                setState(() {
                  _discovery = false;
                  _log.insert(
                    0,
                    'Discovery needs permission to use Bluetooth. '
                    'You can grant it in Settings.',
                  );
                });
                return;
              }
              await setDiscovery(enabled: v);
              if (mounted) setState(() => _discovery = v);
            },
          ),
          if (_pairing == null && _myCode == null && !_scanning)
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
                    onPressed: _startScanning,
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
            child: PairingSurface(
              sas: _pairing == null
                  ? null
                  : SasView(
                      colours: _pairing!.colours,
                      word: _pairing!.word,
                      peerName: _pairing!.peerName,
                      peerConfirmed: _pairing!.peerConfirmed,
                      weConfirmed: _pairing!.weConfirmed,
                      onConfirm: _confirmPairing,
                      onCancel: _cancelPairing,
                    ),
              // Always null here: while scanning the camera has the whole
              // screen and this body is not built at all. Kept as a slot so
              // the precedence stays one ordered decision — the day a scanner
              // does share the page, the colours must still win.
              scanner: null,
              code: _myCode == null
                  ? null
                  : PairingCodeView(
                      code: _myCode!,
                      canScan: canScanQrCodes,
                      canTap: _canTap,
                      onDone: _stopShowingCode,
                    ),
              nearby: NearbyView<NearbyDevice>(
                discoveryOn: _discovery,
                radioReason: _radioReason,
                devices: _devices,
                tile: _deviceTile,
              ),
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
  /// Put this device's code on screen.
  ///
  /// Kept as state rather than pushed as a route, so that when the colours
  /// arrive the body simply draws them instead — see [PairingSurface] for the
  /// two-phone failure that taught this.
  Future<void> _showMyCode() async {
    final String code;
    try {
      code = await pairingInvite();
    } catch (e) {
      setState(() => _log.insert(0, 'Could not make a code: $e'));
      return;
    }
    if (!mounted) return;
    setState(() => _myCode = code);
    // The same string, offered both ways. A tap is narrower than a screen — it
    // needs contact rather than a line of sight — so it is the better half of
    // R0-F4 where the hardware allows, and the QR remains the one that always
    // works.
    //
    // Sharing only. Showing a code makes this phone the *card*, and a card
    // cannot also hold a reader field open: enabling reader mode here would
    // stop the other phone being able to select us at all. The roles are the
    // QR path's roles, and they are exclusive on the radio as well as on paper.
    if (_canTap) await shareCode(code);
  }

  /// Take the code off the screen, and off the air.
  ///
  /// A code left showing keeps a Layer-2 key paired with the rung id this
  /// device is advertising under, and R0-F2 spends twelve minutes at a time
  /// making that pairing hard to observe. Called both when the person taps
  /// Done and when a ceremony reaches the colours — by which point the code has
  /// been read and is of no further use to anyone but a photographer.
  Future<void> _stopShowingCode() async {
    if (_myCode == null) return;
    setState(() => _myCode = null);
    await stopShowingInvite();
    // Off the air by both routes. A code still answering taps after its owner
    // put the phone down keeps a Layer-2 key paired with the rung id this
    // device advertises under — the link R0-F2's rotation exists to break, and
    // one a reader at pocket height could collect.
    if (_canTap) await stopSharingCode();
  }

  /// Open the camera — and the NFC reader, which is the same act by another
  /// route. This is the *reading* role, so it is the only place reader mode
  /// goes on.
  void _startScanning() {
    setState(() {
      _startingCeremony = false;
      _scanning = true;
    });
    if (_canTap) unawaited(startReadingTaps());
  }

  void _stopScanning() {
    setState(() => _scanning = false);
    if (_canTap) unawaited(stopReadingTaps());
  }

  /// Whatever the camera decoded.
  ///
  /// The camera reports every QR code in front of it, most of which are not
  /// ours, so a rejected code is the normal case and not worth a message. The
  /// guard exists because `onScan` fires per frame: without it a single code
  /// held in view starts a ceremony and then floods "already pairing" errors
  /// behind it.
  Future<void> _onCode(String code) async {
    if (_startingCeremony) return;
    _startingCeremony = true;
    try {
      await beginPairing(code: code);
      if (mounted) setState(() => _scanning = false);
    } catch (_) {
      // Not ours, or not nearby. Keep looking — the next frame is a fresh
      // chance and the person is still holding a camera at a screen.
      _startingCeremony = false;
    }
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
