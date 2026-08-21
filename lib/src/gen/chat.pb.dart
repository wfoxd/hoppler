// This is a generated file - do not edit.
//
// Generated from chat.proto.

// @dart = 3.3

// ignore_for_file: annotate_overrides, camel_case_types, comment_references
// ignore_for_file: constant_identifier_names
// ignore_for_file: curly_braces_in_flow_control_structures
// ignore_for_file: deprecated_member_use_from_same_package, library_prefixes
// ignore_for_file: non_constant_identifier_names, prefer_relative_imports

import 'dart:core' as $core;

import 'package:fixnum/fixnum.dart' as $fixnum;
import 'package:protobuf/protobuf.dart' as $pb;

export 'package:protobuf/protobuf.dart' show GeneratedMessageGenericExtensions;

/// One chat message, as it travels inside an established session (T12, R0-F5).
///
/// This rides sealed: the Double Ratchet encrypts these bytes and the frame
/// carries a ratchet header followed by ciphertext. Nothing here is visible to
/// anyone watching the wire, which is why the fields can be as plain as they are.
///
/// # Why there is no thread_id
///
/// The task names the envelope `{thread_id, seq, msg_id, body}`, and three of
/// those travel. A session has exactly one peer, so the thread is whichever one
/// belongs to that peer — the receiver knows it before the message arrives, and
/// the sender's local row id would be meaningless to the other device anyway.
/// Putting it on the wire would be a number that could only ever be wrong.
class ChatMessage extends $pb.GeneratedMessage {
  factory ChatMessage({
    $fixnum.Int64? seq,
    $core.List<$core.int>? msgId,
    $core.List<$core.int>? body,
  }) {
    final result = create();
    if (seq != null) result.seq = seq;
    if (msgId != null) result.msgId = msgId;
    if (body != null) result.body = body;
    return result;
  }

  ChatMessage._();

  factory ChatMessage.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      create()..mergeFromBuffer(data, registry);
  factory ChatMessage.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      create()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ChatMessage',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'hoppler.v0'),
      createEmptyInstance: create)
    ..a<$fixnum.Int64>(1, _omitFieldNames ? '' : 'seq', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..a<$core.List<$core.int>>(
        2, _omitFieldNames ? '' : 'msgId', $pb.PbFieldType.OY)
    ..a<$core.List<$core.int>>(
        3, _omitFieldNames ? '' : 'body', $pb.PbFieldType.OY)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChatMessage clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChatMessage copyWith(void Function(ChatMessage) updates) =>
      super.copyWith((message) => updates(message as ChatMessage))
          as ChatMessage;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  static ChatMessage create() => ChatMessage._();
  @$core.override
  ChatMessage createEmptyInstance() => create();
  @$core.pragma('dart2js:noInline')
  static ChatMessage getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<ChatMessage>(create);
  static ChatMessage? _defaultInstance;

  /// Position in this sender's stream for this thread, from 1. Durable and
  /// never reused: it is what puts a reunion's worth of messages back in order,
  /// what makes a gap visible rather than silent, and what tells a resend from
  /// a new message.
  ///
  /// Per *sender*, so the two directions of one conversation number
  /// independently and neither has to know the other's count.
  @$pb.TagNumber(1)
  $fixnum.Int64 get seq => $_getI64(0);
  @$pb.TagNumber(1)
  set seq($fixnum.Int64 value) => $_setInt64(0, value);
  @$pb.TagNumber(1)
  $core.bool hasSeq() => $_has(0);
  @$pb.TagNumber(1)
  void clearSeq() => $_clearField(1);

  /// 16 random bytes, chosen once by the sender and kept with the outbox entry
  /// so a resend carries the same one.
  ///
  /// The store's global dedup key (`messages.msg_id UNIQUE`). Random rather
  /// than derived from the content: two people can legitimately send the same
  /// word twice, and an id that collided for them would silently drop the
  /// second one.
  @$pb.TagNumber(2)
  $core.List<$core.int> get msgId => $_getN(1);
  @$pb.TagNumber(2)
  set msgId($core.List<$core.int> value) => $_setBytes(1, value);
  @$pb.TagNumber(2)
  $core.bool hasMsgId() => $_has(1);
  @$pb.TagNumber(2)
  void clearMsgId() => $_clearField(2);

  /// The message itself. Bytes rather than string: v0 is text, but a peer
  /// sending invalid UTF-8 must not be able to make a whole envelope
  /// unparseable — proto3 `string` rejects it at decode, which would turn one
  /// bad message into a session that cannot be read at all.
  @$pb.TagNumber(3)
  $core.List<$core.int> get body => $_getN(2);
  @$pb.TagNumber(3)
  set body($core.List<$core.int> value) => $_setBytes(2, value);
  @$pb.TagNumber(3)
  $core.bool hasBody() => $_has(2);
  @$pb.TagNumber(3)
  void clearBody() => $_clearField(3);
}

const $core.bool _omitFieldNames =
    $core.bool.fromEnvironment('protobuf.omit_field_names');
const $core.bool _omitMessageNames =
    $core.bool.fromEnvironment('protobuf.omit_message_names');
