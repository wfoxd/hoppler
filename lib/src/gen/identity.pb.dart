// This is a generated file - do not edit.
//
// Generated from identity.proto.

// @dart = 3.3

// ignore_for_file: annotate_overrides, camel_case_types, comment_references
// ignore_for_file: constant_identifier_names
// ignore_for_file: curly_braces_in_flow_control_structures
// ignore_for_file: deprecated_member_use_from_same_package, library_prefixes
// ignore_for_file: non_constant_identifier_names, prefer_relative_imports

import 'dart:core' as $core;

import 'package:protobuf/protobuf.dart' as $pb;

export 'package:protobuf/protobuf.dart' show GeneratedMessageGenericExtensions;

/// The signable body of a persona record (tech spec §3). These four fields are
/// what the Layer-2 key signs; the encoded bytes are signed verbatim.
class PersonaBody extends $pb.GeneratedMessage {
  factory PersonaBody({
    $core.List<$core.int>? l2Pub,
    $core.String? name,
    $core.int? colour,
    $core.int? version,
  }) {
    final result = create();
    if (l2Pub != null) result.l2Pub = l2Pub;
    if (name != null) result.name = name;
    if (colour != null) result.colour = colour;
    if (version != null) result.version = version;
    return result;
  }

  PersonaBody._();

  factory PersonaBody.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      create()..mergeFromBuffer(data, registry);
  factory PersonaBody.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      create()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'PersonaBody',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'hoppler.v0'),
      createEmptyInstance: create)
    ..a<$core.List<$core.int>>(
        1, _omitFieldNames ? '' : 'l2Pub', $pb.PbFieldType.OY)
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aI(3, _omitFieldNames ? '' : 'colour', fieldType: $pb.PbFieldType.OU3)
    ..aI(4, _omitFieldNames ? '' : 'version', fieldType: $pb.PbFieldType.OU3)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PersonaBody clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PersonaBody copyWith(void Function(PersonaBody) updates) =>
      super.copyWith((message) => updates(message as PersonaBody))
          as PersonaBody;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  static PersonaBody create() => PersonaBody._();
  @$core.override
  PersonaBody createEmptyInstance() => create();
  @$core.pragma('dart2js:noInline')
  static PersonaBody getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<PersonaBody>(create);
  static PersonaBody? _defaultInstance;

  @$pb.TagNumber(1)
  $core.List<$core.int> get l2Pub => $_getN(0);
  @$pb.TagNumber(1)
  set l2Pub($core.List<$core.int> value) => $_setBytes(0, value);
  @$pb.TagNumber(1)
  $core.bool hasL2Pub() => $_has(0);
  @$pb.TagNumber(1)
  void clearL2Pub() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.int get colour => $_getIZ(2);
  @$pb.TagNumber(3)
  set colour($core.int value) => $_setUnsignedInt32(2, value);
  @$pb.TagNumber(3)
  $core.bool hasColour() => $_has(2);
  @$pb.TagNumber(3)
  void clearColour() => $_clearField(3);

  @$pb.TagNumber(4)
  $core.int get version => $_getIZ(3);
  @$pb.TagNumber(4)
  set version($core.int value) => $_setUnsignedInt32(3, value);
  @$pb.TagNumber(4)
  $core.bool hasVersion() => $_has(3);
  @$pb.TagNumber(4)
  void clearVersion() => $_clearField(4);
}

/// A PersonaBody plus a self-signature by the l2_pub it contains. The signature
/// covers `body` exactly as carried, so verification never re-encodes.
class SignedPersona extends $pb.GeneratedMessage {
  factory SignedPersona({
    $core.List<$core.int>? body,
    $core.List<$core.int>? signature,
  }) {
    final result = create();
    if (body != null) result.body = body;
    if (signature != null) result.signature = signature;
    return result;
  }

  SignedPersona._();

  factory SignedPersona.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      create()..mergeFromBuffer(data, registry);
  factory SignedPersona.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      create()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SignedPersona',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'hoppler.v0'),
      createEmptyInstance: create)
    ..a<$core.List<$core.int>>(
        1, _omitFieldNames ? '' : 'body', $pb.PbFieldType.OY)
    ..a<$core.List<$core.int>>(
        2, _omitFieldNames ? '' : 'signature', $pb.PbFieldType.OY)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SignedPersona clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SignedPersona copyWith(void Function(SignedPersona) updates) =>
      super.copyWith((message) => updates(message as SignedPersona))
          as SignedPersona;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  static SignedPersona create() => SignedPersona._();
  @$core.override
  SignedPersona createEmptyInstance() => create();
  @$core.pragma('dart2js:noInline')
  static SignedPersona getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<SignedPersona>(create);
  static SignedPersona? _defaultInstance;

  @$pb.TagNumber(1)
  $core.List<$core.int> get body => $_getN(0);
  @$pb.TagNumber(1)
  set body($core.List<$core.int> value) => $_setBytes(0, value);
  @$pb.TagNumber(1)
  $core.bool hasBody() => $_has(0);
  @$pb.TagNumber(1)
  void clearBody() => $_clearField(1);

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
