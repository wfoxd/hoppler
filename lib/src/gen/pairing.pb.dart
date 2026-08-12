// This is a generated file - do not edit.
//
// Generated from pairing.proto.

// @dart = 3.3

// ignore_for_file: annotate_overrides, camel_case_types, comment_references
// ignore_for_file: constant_identifier_names
// ignore_for_file: curly_braces_in_flow_control_structures
// ignore_for_file: deprecated_member_use_from_same_package, library_prefixes
// ignore_for_file: non_constant_identifier_names, prefer_relative_imports

import 'dart:core' as $core;

import 'package:protobuf/protobuf.dart' as $pb;

export 'package:protobuf/protobuf.dart' show GeneratedMessageGenericExtensions;

/// What the pairing QR code — and the NFC tap, which carries the identical
/// payload — puts in front of the other person's camera (tech spec §6, R0-F4).
///
/// Deliberately NOT signed. A signature over these fields would prove the shower
/// holds `l2_pub`, which the ceremony's Noise handshake proves anyway, and it
/// would stop nothing: an attacker who substitutes the whole code substitutes
/// their own `l2_pub` too and signs it validly. The authenticity of this payload
/// comes from the physical channel — a person holding a screen up — and from the
/// human-checked SAS at the end. Adding 64 bytes to say otherwise would make the
/// code denser and the guarantee no stronger.
class PairingInvite extends $pb.GeneratedMessage {
  factory PairingInvite({
    $core.List<$core.int>? l2Pub,
    $core.String? bleHint,
    $core.List<$core.int>? ceremonyNonce,
  }) {
    final result = create();
    if (l2Pub != null) result.l2Pub = l2Pub;
    if (bleHint != null) result.bleHint = bleHint;
    if (ceremonyNonce != null) result.ceremonyNonce = ceremonyNonce;
    return result;
  }

  PairingInvite._();

  factory PairingInvite.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      create()..mergeFromBuffer(data, registry);
  factory PairingInvite.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      create()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'PairingInvite',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'hoppler.v0'),
      createEmptyInstance: create)
    ..a<$core.List<$core.int>>(
        1, _omitFieldNames ? '' : 'l2Pub', $pb.PbFieldType.OY)
    ..aOS(2, _omitFieldNames ? '' : 'bleHint')
    ..a<$core.List<$core.int>>(
        3, _omitFieldNames ? '' : 'ceremonyNonce', $pb.PbFieldType.OY)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PairingInvite clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PairingInvite copyWith(void Function(PairingInvite) updates) =>
      super.copyWith((message) => updates(message as PairingInvite))
          as PairingInvite;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  static PairingInvite create() => PairingInvite._();
  @$core.override
  PairingInvite createEmptyInstance() => create();
  @$core.pragma('dart2js:noInline')
  static PairingInvite getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<PairingInvite>(create);
  static PairingInvite? _defaultInstance;

  /// Ed25519 Layer-2 public key (32 bytes). The scanner needs it to address the
  /// ceremony and to check, at the end, that the identity it verified is the one
  /// whose code it read.
  @$pb.TagNumber(1)
  $core.List<$core.int> get l2Pub => $_getN(0);
  @$pb.TagNumber(1)
  set l2Pub($core.List<$core.int> value) => $_setBytes(0, value);
  @$pb.TagNumber(1)
  $core.bool hasL2Pub() => $_has(0);
  @$pb.TagNumber(1)
  void clearL2Pub() => $_clearField(1);

  /// The rung-level id this device is advertising as *right now* — a shortcut so
  /// the scanner can dial without waiting for a discovery sweep.
  ///
  /// A hint and nothing more. It rotates every twelve minutes under R0-F2, so a
  /// code left on a screen goes stale, and a scanner that cannot reach it must
  /// fall back to discovery rather than fail. Nothing may be trusted on the
  /// strength of this field: it is unauthenticated and only ever an address.
  @$pb.TagNumber(2)
  $core.String get bleHint => $_getSZ(1);
  @$pb.TagNumber(2)
  set bleHint($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasBleHint() => $_has(1);
  @$pb.TagNumber(2)
  void clearBleHint() => $_clearField(2);

  /// 32 random bytes binding this ceremony to this code. Mixed into the
  /// handshake as its prologue, so a handshake for one code cannot be replayed
  /// into another, and folded into the SAS the humans compare.
  @$pb.TagNumber(3)
  $core.List<$core.int> get ceremonyNonce => $_getN(2);
  @$pb.TagNumber(3)
  set ceremonyNonce($core.List<$core.int> value) => $_setBytes(2, value);
  @$pb.TagNumber(3)
  $core.bool hasCeremonyNonce() => $_has(2);
  @$pb.TagNumber(3)
  void clearCeremonyNonce() => $_clearField(3);
}

/// A device's Layer-1 public identity, and proof it holds the matching secret.
///
/// This is the thing the whole ceremony exists to move, and the only message in
/// Hoppler that carries Layer-1 material (R0-F1). It travels inside the verified
/// ceremony channel, and only after **both** people have confirmed the SAS.
///
/// The signature is required even though the channel is already authenticated.
/// The channel proves who is *speaking* — a Layer-2 persona — and a Layer-1 key
/// is a separate claim: without a signature by that key, anyone could hand over
/// somebody else's Layer-1 public key and have it written into the other
/// device's contacts as their own. What is signed is a domain-separated message
/// over the ceremony's transcript hash and both keys, so the proof binds Layer-1
/// to Layer-2 and cannot be lifted into a different ceremony.
class L1Proof extends $pb.GeneratedMessage {
  factory L1Proof({
    $core.List<$core.int>? l1Pub,
    $core.List<$core.int>? signature,
  }) {
    final result = create();
    if (l1Pub != null) result.l1Pub = l1Pub;
    if (signature != null) result.signature = signature;
    return result;
  }

  L1Proof._();

  factory L1Proof.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      create()..mergeFromBuffer(data, registry);
  factory L1Proof.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      create()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'L1Proof',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'hoppler.v0'),
      createEmptyInstance: create)
    ..a<$core.List<$core.int>>(
        1, _omitFieldNames ? '' : 'l1Pub', $pb.PbFieldType.OY)
    ..a<$core.List<$core.int>>(
        2, _omitFieldNames ? '' : 'signature', $pb.PbFieldType.OY)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  L1Proof clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  L1Proof copyWith(void Function(L1Proof) updates) =>
      super.copyWith((message) => updates(message as L1Proof)) as L1Proof;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  static L1Proof create() => L1Proof._();
  @$core.override
  L1Proof createEmptyInstance() => create();
  @$core.pragma('dart2js:noInline')
  static L1Proof getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<L1Proof>(create);
  static L1Proof? _defaultInstance;

  @$pb.TagNumber(1)
  $core.List<$core.int> get l1Pub => $_getN(0);
  @$pb.TagNumber(1)
  set l1Pub($core.List<$core.int> value) => $_setBytes(0, value);
  @$pb.TagNumber(1)
  $core.bool hasL1Pub() => $_has(0);
  @$pb.TagNumber(1)
  void clearL1Pub() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.List<$core.int> get signature => $_getN(1);
  @$pb.TagNumber(2)
  set signature($core.List<$core.int> value) => $_setBytes(1, value);
  @$pb.TagNumber(2)
  $core.bool hasSignature() => $_has(1);
  @$pb.TagNumber(2)
  void clearSignature() => $_clearField(2);
}

const $core.bool _omitFieldNames =
    $core.bool.fromEnvironment('protobuf.omit_field_names');
const $core.bool _omitMessageNames =
    $core.bool.fromEnvironment('protobuf.omit_message_names');
