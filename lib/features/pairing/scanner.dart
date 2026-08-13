import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_zxing/flutter_zxing.dart';

/// Whether this build can read a QR code at all.
///
/// Android only, and not because of the decoder. `flutter_zxing` builds its
/// zxing-cpp decoder for Linux perfectly well; what does not exist is a camera.
/// Flutter's federated `camera` plugin has implementations for Android, iOS and
/// web, and none for Linux — so on the desktop build there is no frame source
/// to decode, whichever scanning package is chosen.
///
/// iOS is excluded here for a different reason: T01/T02 are parked, so no iOS
/// build is exercised and claiming support for one nobody has run would be a
/// promise made on paper. Adding it back is one word.
///
/// Stated as a function rather than a constant so tests can reason about both
/// branches, and so the reason lives next to the answer.
bool get canScanQrCodes => Platform.isAndroid;

/// The camera, behind a seam.
///
/// Everything else in this feature is a pure widget precisely so it can be
/// tested; a scanner cannot be, because there is no camera in a widget test and
/// no honest way to fake one deeper down. So the boundary is here: the camera
/// produces strings, and every decision about what a string *means* — is it an
/// invite, whose is it, what happens next — lives in the core and in
/// `HomePage`, where it can be exercised.
///
/// The widget therefore reports raw text and nothing else. It deliberately does
/// not parse, validate or filter: a scanner that decided which codes were
/// interesting would be a second implementation of `Invite::parse`, in the
/// layer with no tests.
class QrScannerView extends StatelessWidget {
  const QrScannerView({
    super.key,
    required this.onCode,
    required this.onCancel,
  });

  /// Called with whatever the camera decoded. May be called repeatedly, and
  /// with codes that have nothing to do with Hoppler — a camera reads every QR
  /// in front of it. The caller is expected to ignore what it cannot use.
  final void Function(String code) onCode;

  final VoidCallback onCancel;

  static const instruction = 'Point this at their code.';

  @override
  Widget build(BuildContext context) {
    if (!canScanQrCodes) {
      return _Unavailable(onCancel: onCancel);
    }
    return _Scanner(onCode: onCode, onCancel: onCancel);
  }
}

/// The camera, and what to say when there is not one.
class _Scanner extends StatefulWidget {
  const _Scanner({required this.onCode, required this.onCancel});

  final void Function(String code) onCode;
  final VoidCallback onCancel;

  @override
  State<_Scanner> createState() => _ScannerState();
}

class _ScannerState extends State<_Scanner> {
  /// Why the camera did not open, if it did not.
  ///
  /// The plugin asks for the permission itself and reports a refusal by
  /// failing to build a controller. Without this the screen is simply black:
  /// no camera, no message, and nothing to suggest the person is looking at
  /// the consequence of their own answer to a prompt. That is the same
  /// availability lie R0-N6 rules out for the radio, and it would have been
  /// found on two phones rather than here.
  String? _problem;

  @override
  Widget build(BuildContext context) {
    final problem = _problem;
    if (problem != null) {
      return _Blocked(reason: problem, onCancel: widget.onCancel);
    }
    return Column(
      children: [
        Expanded(
          child: ReaderWidget(
            onControllerCreated: (controller, error) {
              if (error == null) return;
              setState(() {
                _problem = error.toString().contains('AccessDenied')
                    ? 'Hoppler needs the camera to read a code. You can turn '
                          'it on in Settings, or show your own code instead.'
                    : 'The camera could not be opened. Show your own code '
                          'instead and let the other phone scan it.';
              });
            },
            // Only QR. A ceremony code is a QR code, and accepting every
            // barcode format would mean decoding work on every frame for
            // formats this app has no use for — battery spent for nothing
            // (R0-N4), on the one screen where the camera is running.
            codeFormat: Format.qrCode,
            showGallery: false,
            showFlashlight: true,
            showToggleCamera: false,
            onScan: (result) {
              final text = result.text;
              if (text != null && text.isNotEmpty) widget.onCode(text);
            },
          ),
        ),
        Padding(
          padding: const EdgeInsets.all(16),
          child: Column(
            children: [
              const Text(
                QrScannerView.instruction,
                textAlign: TextAlign.center,
              ),
              const SizedBox(height: 8),
              TextButton(
                onPressed: widget.onCancel,
                child: const Text('Cancel'),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// What the desktop build shows instead of a camera.
class _Unavailable extends StatelessWidget {
  const _Unavailable({required this.onCancel});

  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context) => _Blocked(
    reason:
        'This device cannot scan a code. Show yours instead and let the '
        'other phone scan it.',
    onCancel: onCancel,
  );
}

/// A camera that is not going to appear, and why.
///
/// One widget for both reasons — no camera on this platform, and a camera this
/// person has declined — because the only thing that differs is the sentence,
/// and the way out is the same.
class _Blocked extends StatelessWidget {
  const _Blocked({required this.reason, required this.onCancel});

  final String reason;
  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Text(reason, textAlign: TextAlign.center),
          const SizedBox(height: 16),
          TextButton(onPressed: onCancel, child: const Text('Back')),
        ],
      ),
    );
  }
}
