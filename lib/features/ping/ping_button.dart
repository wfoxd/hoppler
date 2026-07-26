import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hoppler/features/ping/ping_service.dart';

/// The primitive gesture (R0-F3): one tap reaches a nearby device and shows the
/// acknowledgement coming back. Idle → pinging → acked, then back to idle.
///
/// Ping is a gesture, not a messenger — no payload (tech spec §7). Real
/// cross-device delivery + the receiver-side rate limiter arrive with the
/// session layer (T09/T10) behind the same [PingService] seam.
///
/// Give each button a stable `ValueKey(deviceId)` in a list so Flutter never
/// recycles one device's state onto another; `didUpdateWidget` is the
/// belt-and-suspenders if it does.
class PingButton extends StatefulWidget {
  const PingButton({super.key, required this.service, required this.deviceId});

  final PingService service;
  final String deviceId;

  @override
  State<PingButton> createState() => _PingButtonState();
}

enum _Phase { idle, pinging, acked }

class _PingButtonState extends State<PingButton> {
  static const _ackTimeout = Duration(seconds: 5);
  static const _ackedHold = Duration(seconds: 2);

  _Phase _phase = _Phase.idle;
  StreamSubscription<String>? _ackSub;
  Timer? _reset; // holds the acked state briefly, then returns to idle
  Timer? _watchdog; // reverts pinging to idle if no ack arrives

  @override
  void initState() {
    super.initState();
    _subscribe();
  }

  @override
  void didUpdateWidget(PingButton old) {
    super.didUpdateWidget(old);
    if (old.deviceId != widget.deviceId || old.service != widget.service) {
      // This state was recycled onto a different device — start clean.
      _clearTimers();
      _phase = _Phase.idle;
      _subscribe();
    }
  }

  void _subscribe() {
    _ackSub?.cancel();
    // Acks correlate only by deviceId — fine for a payload-less gesture. Chat
    // and Drop, which copy this pattern, will need a per-message token instead.
    _ackSub = widget.service.acks.where((id) => id == widget.deviceId).listen((_) {
      if (!mounted || _phase != _Phase.pinging) return;
      _watchdog?.cancel();
      _reset?.cancel();
      setState(() => _phase = _Phase.acked);
      _reset = Timer(_ackedHold, () {
        if (mounted && _phase == _Phase.acked) setState(() => _phase = _Phase.idle);
      });
    });
  }

  void _clearTimers() {
    _reset?.cancel();
    _watchdog?.cancel();
  }

  @override
  void dispose() {
    _ackSub?.cancel();
    _clearTimers();
    super.dispose();
  }

  Future<void> _ping() async {
    _clearTimers(); // drop any pending acked-hold from a previous ping
    setState(() => _phase = _Phase.pinging);
    _watchdog = Timer(_ackTimeout, () {
      // No ack (dropped or absent) — don't stay disabled forever.
      if (mounted && _phase == _Phase.pinging) setState(() => _phase = _Phase.idle);
    });
    try {
      await widget.service.ping(widget.deviceId);
    } catch (e) {
      if (!mounted) return;
      _clearTimers();
      setState(() => _phase = _Phase.idle);
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('Ping failed: $e')));
    }
  }

  @override
  Widget build(BuildContext context) {
    final (icon, colour, tooltip) = switch (_phase) {
      _Phase.idle => (Icons.waving_hand_outlined, null, 'Ping'),
      _Phase.pinging => (Icons.more_horiz, Colors.orange, 'Pinging…'),
      _Phase.acked => (Icons.check_circle, Colors.green, 'Acked'),
    };
    return IconButton(
      icon: Icon(icon, color: colour),
      tooltip: tooltip,
      onPressed: _phase == _Phase.pinging ? null : _ping,
    );
  }
}
