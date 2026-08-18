import 'package:flutter/services.dart';

/// The radio permissions, asked for at the moment somebody wants the radio.
///
/// Its own channel because asking needs an *Activity*, and `BleAdapter` holds
/// the application context on purpose so it can outlive one.
///
/// # Why not at startup
///
/// It used to be asked from `onResume`, so a fresh install met "allow Hoppler
/// to find, connect to and determine the relative position of nearby devices"
/// before the person had said who they were, let alone that they wanted to be
/// found. That is the prompt most likely to be denied, at the moment it makes
/// least sense, and Android's own guidance is to ask in context. Hoppler has
/// one: the Discovery switch.
const _channel = MethodChannel('org.hoppler/permissions');

/// Ask for the Bluetooth permissions if they are not already held.
///
/// Answers whether the radio may be used. `false` covers both "declined just
/// now" and "declined before" — after a refusal Android shows no dialog at all
/// and answers immediately, so a caller must be able to tell that nothing will
/// happen rather than wait for a prompt that never appears.
///
/// A platform with no such permissions — Linux desktop, where there is no
/// channel registered — answers `true`: nothing is standing in the way there,
/// and reporting "denied" would turn a working rung into a refusal.
Future<bool> ensureRadioPermission() async {
  try {
    return await _channel.invokeMethod<bool>('ensureRadio') ?? false;
  } on MissingPluginException {
    return true;
  }
}
