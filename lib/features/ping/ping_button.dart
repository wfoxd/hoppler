import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hoppler/features/ping/ping_service.dart';

/// The primitive gesture (R0-F3): one tap reaches a nearby device and shows the
/// acknowledgement coming back. Idle → pinging → acked, then back to idle.
///
/// Ping is a gesture, not a messenger — no payload (tech spec §7). Real
/// cross-device delivery + the receiver-side rate limiter arrive with the
/// session layer (T09/T10) behind the same [PingService] seam.
class PingButton extends StatefulWidget {
  const PingButton({super.key, required this.service, required this.deviceId});

  final PingService service;
  final String deviceId;

  @override
  State<PingButton> createState() => _PingButtonState();
}

enum _Phase { idle, pinging, acked }

class _PingButtonState extends State<PingButton> {
  _Phase _phase = _Phase.idle;
  StreamSubscription<String>? _ackSub;
  Timer? _reset;

  @override
  void initState() {
    super.initState();
    _ackSub = widget.service.acks.where((id) => id == widget.deviceId).listen((_) {
      if (_phase != _Phase.pinging) return;
      setState(() => _phase = _Phase.acked);
      _reset?.cancel();
      _reset = Timer(const Duration(seconds: 2), () {
        if (mounted) setState(() => _phase = _Phase.idle);
      });
    });
  }

  @override
  void dispose() {
    _ackSub?.cancel();
    _reset?.cancel();
    super.dispose();
  }

  Future<void> _ping() async {
    setState(() => _phase = _Phase.pinging);
    try {
      await widget.service.ping(widget.deviceId);
    } catch (e) {
      if (!mounted) return;
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
