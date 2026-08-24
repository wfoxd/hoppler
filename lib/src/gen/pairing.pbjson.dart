// This is a generated file - do not edit.
//
// Generated from pairing.proto.

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

@$core.Deprecated('Use pairingInviteDescriptor instead')
const PairingInvite$json = {
  '1': 'PairingInvite',
  '2': [
    {'1': 'l2_pub', '3': 1, '4': 1, '5': 12, '10': 'l2Pub'},
    {'1': 'ble_hint', '3': 2, '4': 1, '5': 9, '10': 'bleHint'},
    {'1': 'ceremony_nonce', '3': 3, '4': 1, '5': 12, '10': 'ceremonyNonce'},
  ],
};

/// Descriptor for `PairingInvite`. Decode as a `google.protobuf.DescriptorProto`.
final $typed_data.Uint8List pairingInviteDescriptor = $convert.base64Decode(
    'Cg1QYWlyaW5nSW52aXRlEhUKBmwyX3B1YhgBIAEoDFIFbDJQdWISGQoIYmxlX2hpbnQYAiABKA'
    'lSB2JsZUhpbnQSJQoOY2VyZW1vbnlfbm9uY2UYAyABKAxSDWNlcmVtb255Tm9uY2U=');

@$core.Deprecated('Use l1ProofDescriptor instead')
const L1Proof$json = {
  '1': 'L1Proof',
  '2': [
    {'1': 'l1_pub', '3': 1, '4': 1, '5': 12, '10': 'l1Pub'},
    {'1': 'signature', '3': 2, '4': 1, '5': 12, '10': 'signature'},
    {'1': 'ratchet_pub', '3': 3, '4': 1, '5': 12, '10': 'ratchetPub'},
  ],
};

/// Descriptor for `L1Proof`. Decode as a `google.protobuf.DescriptorProto`.
final $typed_data.Uint8List l1ProofDescriptor = $convert.base64Decode(
    'CgdMMVByb29mEhUKBmwxX3B1YhgBIAEoDFIFbDFQdWISHAoJc2lnbmF0dXJlGAIgASgMUglzaW'
    'duYXR1cmUSHwoLcmF0Y2hldF9wdWIYAyABKAxSCnJhdGNoZXRQdWI=');
