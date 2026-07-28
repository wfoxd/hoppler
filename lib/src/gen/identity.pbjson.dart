// This is a generated file - do not edit.
//
// Generated from identity.proto.

// @dart = 3.3

// ignore_for_file: annotate_overrides, camel_case_types, comment_references
// ignore_for_file: constant_identifier_names
// ignore_for_file: curly_braces_in_flow_control_structures
// ignore_for_file: deprecated_member_use_from_same_package, library_prefixes
// ignore_for_file: non_constant_identifier_names, prefer_relative_imports
// ignore_for_file: unused_import

import 'dart:convert' as $convert;
import 'dart:core' as $core;
import 'dart:typed_data' as $typed_data;

@$core.Deprecated('Use personaBodyDescriptor instead')
const PersonaBody$json = {
  '1': 'PersonaBody',
  '2': [
    {'1': 'l2_pub', '3': 1, '4': 1, '5': 12, '10': 'l2Pub'},
    {'1': 'name', '3': 2, '4': 1, '5': 9, '10': 'name'},
    {'1': 'colour', '3': 3, '4': 1, '5': 13, '10': 'colour'},
    {'1': 'version', '3': 4, '4': 1, '5': 13, '10': 'version'},
    {'1': 'session_pub', '3': 5, '4': 1, '5': 12, '10': 'sessionPub'},
  ],
};

/// Descriptor for `PersonaBody`. Decode as a `google.protobuf.DescriptorProto`.
final $typed_data.Uint8List personaBodyDescriptor = $convert.base64Decode(
    'CgtQZXJzb25hQm9keRIVCgZsMl9wdWIYASABKAxSBWwyUHViEhIKBG5hbWUYAiABKAlSBG5hbW'
    'USFgoGY29sb3VyGAMgASgNUgZjb2xvdXISGAoHdmVyc2lvbhgEIAEoDVIHdmVyc2lvbhIfCgtz'
    'ZXNzaW9uX3B1YhgFIAEoDFIKc2Vzc2lvblB1Yg==');

@$core.Deprecated('Use signedPersonaDescriptor instead')
const SignedPersona$json = {
  '1': 'SignedPersona',
  '2': [
    {'1': 'body', '3': 1, '4': 1, '5': 12, '10': 'body'},
    {'1': 'signature', '3': 2, '4': 1, '5': 12, '10': 'signature'},
  ],
};

/// Descriptor for `SignedPersona`. Decode as a `google.protobuf.DescriptorProto`.
final $typed_data.Uint8List signedPersonaDescriptor = $convert.base64Decode(
    'Cg1TaWduZWRQZXJzb25hEhIKBGJvZHkYASABKAxSBGJvZHkSHAoJc2lnbmF0dXJlGAIgASgMUg'
    'lzaWduYXR1cmU=');
