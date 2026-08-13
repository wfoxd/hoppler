// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'types.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$CoreEvent {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'CoreEvent()';
}


}

/// @nodoc
class $CoreEventCopyWith<$Res>  {
$CoreEventCopyWith(CoreEvent _, $Res Function(CoreEvent) __);
}


/// Adds pattern-matching-related methods to [CoreEvent].
extension CoreEventPatterns on CoreEvent {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( CoreEvent_DiscoveryUpdated value)?  discoveryUpdated,TResult Function( CoreEvent_Pinged value)?  pinged,TResult Function( CoreEvent_PingAcked value)?  pingAcked,TResult Function( CoreEvent_PingFailed value)?  pingFailed,TResult Function( CoreEvent_MessageReceived value)?  messageReceived,TResult Function( CoreEvent_TransferProgress value)?  transferProgress,TResult Function( CoreEvent_TransferCompleted value)?  transferCompleted,TResult Function( CoreEvent_PairingSas value)?  pairingSas,TResult Function( CoreEvent_PairingPeerConfirmed value)?  pairingPeerConfirmed,TResult Function( CoreEvent_PairingCompleted value)?  pairingCompleted,TResult Function( CoreEvent_PairingFailed value)?  pairingFailed,TResult Function( CoreEvent_RadioChanged value)?  radioChanged,required TResult orElse(),}){
final _that = this;
switch (_that) {
case CoreEvent_DiscoveryUpdated() when discoveryUpdated != null:
return discoveryUpdated(_that);case CoreEvent_Pinged() when pinged != null:
return pinged(_that);case CoreEvent_PingAcked() when pingAcked != null:
return pingAcked(_that);case CoreEvent_PingFailed() when pingFailed != null:
return pingFailed(_that);case CoreEvent_MessageReceived() when messageReceived != null:
return messageReceived(_that);case CoreEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that);case CoreEvent_TransferCompleted() when transferCompleted != null:
return transferCompleted(_that);case CoreEvent_PairingSas() when pairingSas != null:
return pairingSas(_that);case CoreEvent_PairingPeerConfirmed() when pairingPeerConfirmed != null:
return pairingPeerConfirmed(_that);case CoreEvent_PairingCompleted() when pairingCompleted != null:
return pairingCompleted(_that);case CoreEvent_PairingFailed() when pairingFailed != null:
return pairingFailed(_that);case CoreEvent_RadioChanged() when radioChanged != null:
return radioChanged(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( CoreEvent_DiscoveryUpdated value)  discoveryUpdated,required TResult Function( CoreEvent_Pinged value)  pinged,required TResult Function( CoreEvent_PingAcked value)  pingAcked,required TResult Function( CoreEvent_PingFailed value)  pingFailed,required TResult Function( CoreEvent_MessageReceived value)  messageReceived,required TResult Function( CoreEvent_TransferProgress value)  transferProgress,required TResult Function( CoreEvent_TransferCompleted value)  transferCompleted,required TResult Function( CoreEvent_PairingSas value)  pairingSas,required TResult Function( CoreEvent_PairingPeerConfirmed value)  pairingPeerConfirmed,required TResult Function( CoreEvent_PairingCompleted value)  pairingCompleted,required TResult Function( CoreEvent_PairingFailed value)  pairingFailed,required TResult Function( CoreEvent_RadioChanged value)  radioChanged,}){
final _that = this;
switch (_that) {
case CoreEvent_DiscoveryUpdated():
return discoveryUpdated(_that);case CoreEvent_Pinged():
return pinged(_that);case CoreEvent_PingAcked():
return pingAcked(_that);case CoreEvent_PingFailed():
return pingFailed(_that);case CoreEvent_MessageReceived():
return messageReceived(_that);case CoreEvent_TransferProgress():
return transferProgress(_that);case CoreEvent_TransferCompleted():
return transferCompleted(_that);case CoreEvent_PairingSas():
return pairingSas(_that);case CoreEvent_PairingPeerConfirmed():
return pairingPeerConfirmed(_that);case CoreEvent_PairingCompleted():
return pairingCompleted(_that);case CoreEvent_PairingFailed():
return pairingFailed(_that);case CoreEvent_RadioChanged():
return radioChanged(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( CoreEvent_DiscoveryUpdated value)?  discoveryUpdated,TResult? Function( CoreEvent_Pinged value)?  pinged,TResult? Function( CoreEvent_PingAcked value)?  pingAcked,TResult? Function( CoreEvent_PingFailed value)?  pingFailed,TResult? Function( CoreEvent_MessageReceived value)?  messageReceived,TResult? Function( CoreEvent_TransferProgress value)?  transferProgress,TResult? Function( CoreEvent_TransferCompleted value)?  transferCompleted,TResult? Function( CoreEvent_PairingSas value)?  pairingSas,TResult? Function( CoreEvent_PairingPeerConfirmed value)?  pairingPeerConfirmed,TResult? Function( CoreEvent_PairingCompleted value)?  pairingCompleted,TResult? Function( CoreEvent_PairingFailed value)?  pairingFailed,TResult? Function( CoreEvent_RadioChanged value)?  radioChanged,}){
final _that = this;
switch (_that) {
case CoreEvent_DiscoveryUpdated() when discoveryUpdated != null:
return discoveryUpdated(_that);case CoreEvent_Pinged() when pinged != null:
return pinged(_that);case CoreEvent_PingAcked() when pingAcked != null:
return pingAcked(_that);case CoreEvent_PingFailed() when pingFailed != null:
return pingFailed(_that);case CoreEvent_MessageReceived() when messageReceived != null:
return messageReceived(_that);case CoreEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that);case CoreEvent_TransferCompleted() when transferCompleted != null:
return transferCompleted(_that);case CoreEvent_PairingSas() when pairingSas != null:
return pairingSas(_that);case CoreEvent_PairingPeerConfirmed() when pairingPeerConfirmed != null:
return pairingPeerConfirmed(_that);case CoreEvent_PairingCompleted() when pairingCompleted != null:
return pairingCompleted(_that);case CoreEvent_PairingFailed() when pairingFailed != null:
return pairingFailed(_that);case CoreEvent_RadioChanged() when radioChanged != null:
return radioChanged(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( List<NearbyDevice> devices)?  discoveryUpdated,TResult Function( String deviceId,  String name)?  pinged,TResult Function( String deviceId)?  pingAcked,TResult Function( String deviceId,  String reason)?  pingFailed,TResult Function( PlatformInt64 threadId,  String msgId,  String text)?  messageReceived,TResult Function( String transferId,  BigInt received,  BigInt total)?  transferProgress,TResult Function( String transferId,  bool success)?  transferCompleted,TResult Function( String deviceId,  SasDto sas)?  pairingSas,TResult Function( String deviceId)?  pairingPeerConfirmed,TResult Function( String deviceId,  PlatformInt64 threadId,  String name,  int colour)?  pairingCompleted,TResult Function( String deviceId,  String reason)?  pairingFailed,TResult Function( bool available,  String? reason)?  radioChanged,required TResult orElse(),}) {final _that = this;
switch (_that) {
case CoreEvent_DiscoveryUpdated() when discoveryUpdated != null:
return discoveryUpdated(_that.devices);case CoreEvent_Pinged() when pinged != null:
return pinged(_that.deviceId,_that.name);case CoreEvent_PingAcked() when pingAcked != null:
return pingAcked(_that.deviceId);case CoreEvent_PingFailed() when pingFailed != null:
return pingFailed(_that.deviceId,_that.reason);case CoreEvent_MessageReceived() when messageReceived != null:
return messageReceived(_that.threadId,_that.msgId,_that.text);case CoreEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that.transferId,_that.received,_that.total);case CoreEvent_TransferCompleted() when transferCompleted != null:
return transferCompleted(_that.transferId,_that.success);case CoreEvent_PairingSas() when pairingSas != null:
return pairingSas(_that.deviceId,_that.sas);case CoreEvent_PairingPeerConfirmed() when pairingPeerConfirmed != null:
return pairingPeerConfirmed(_that.deviceId);case CoreEvent_PairingCompleted() when pairingCompleted != null:
return pairingCompleted(_that.deviceId,_that.threadId,_that.name,_that.colour);case CoreEvent_PairingFailed() when pairingFailed != null:
return pairingFailed(_that.deviceId,_that.reason);case CoreEvent_RadioChanged() when radioChanged != null:
return radioChanged(_that.available,_that.reason);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( List<NearbyDevice> devices)  discoveryUpdated,required TResult Function( String deviceId,  String name)  pinged,required TResult Function( String deviceId)  pingAcked,required TResult Function( String deviceId,  String reason)  pingFailed,required TResult Function( PlatformInt64 threadId,  String msgId,  String text)  messageReceived,required TResult Function( String transferId,  BigInt received,  BigInt total)  transferProgress,required TResult Function( String transferId,  bool success)  transferCompleted,required TResult Function( String deviceId,  SasDto sas)  pairingSas,required TResult Function( String deviceId)  pairingPeerConfirmed,required TResult Function( String deviceId,  PlatformInt64 threadId,  String name,  int colour)  pairingCompleted,required TResult Function( String deviceId,  String reason)  pairingFailed,required TResult Function( bool available,  String? reason)  radioChanged,}) {final _that = this;
switch (_that) {
case CoreEvent_DiscoveryUpdated():
return discoveryUpdated(_that.devices);case CoreEvent_Pinged():
return pinged(_that.deviceId,_that.name);case CoreEvent_PingAcked():
return pingAcked(_that.deviceId);case CoreEvent_PingFailed():
return pingFailed(_that.deviceId,_that.reason);case CoreEvent_MessageReceived():
return messageReceived(_that.threadId,_that.msgId,_that.text);case CoreEvent_TransferProgress():
return transferProgress(_that.transferId,_that.received,_that.total);case CoreEvent_TransferCompleted():
return transferCompleted(_that.transferId,_that.success);case CoreEvent_PairingSas():
return pairingSas(_that.deviceId,_that.sas);case CoreEvent_PairingPeerConfirmed():
return pairingPeerConfirmed(_that.deviceId);case CoreEvent_PairingCompleted():
return pairingCompleted(_that.deviceId,_that.threadId,_that.name,_that.colour);case CoreEvent_PairingFailed():
return pairingFailed(_that.deviceId,_that.reason);case CoreEvent_RadioChanged():
return radioChanged(_that.available,_that.reason);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( List<NearbyDevice> devices)?  discoveryUpdated,TResult? Function( String deviceId,  String name)?  pinged,TResult? Function( String deviceId)?  pingAcked,TResult? Function( String deviceId,  String reason)?  pingFailed,TResult? Function( PlatformInt64 threadId,  String msgId,  String text)?  messageReceived,TResult? Function( String transferId,  BigInt received,  BigInt total)?  transferProgress,TResult? Function( String transferId,  bool success)?  transferCompleted,TResult? Function( String deviceId,  SasDto sas)?  pairingSas,TResult? Function( String deviceId)?  pairingPeerConfirmed,TResult? Function( String deviceId,  PlatformInt64 threadId,  String name,  int colour)?  pairingCompleted,TResult? Function( String deviceId,  String reason)?  pairingFailed,TResult? Function( bool available,  String? reason)?  radioChanged,}) {final _that = this;
switch (_that) {
case CoreEvent_DiscoveryUpdated() when discoveryUpdated != null:
return discoveryUpdated(_that.devices);case CoreEvent_Pinged() when pinged != null:
return pinged(_that.deviceId,_that.name);case CoreEvent_PingAcked() when pingAcked != null:
return pingAcked(_that.deviceId);case CoreEvent_PingFailed() when pingFailed != null:
return pingFailed(_that.deviceId,_that.reason);case CoreEvent_MessageReceived() when messageReceived != null:
return messageReceived(_that.threadId,_that.msgId,_that.text);case CoreEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that.transferId,_that.received,_that.total);case CoreEvent_TransferCompleted() when transferCompleted != null:
return transferCompleted(_that.transferId,_that.success);case CoreEvent_PairingSas() when pairingSas != null:
return pairingSas(_that.deviceId,_that.sas);case CoreEvent_PairingPeerConfirmed() when pairingPeerConfirmed != null:
return pairingPeerConfirmed(_that.deviceId);case CoreEvent_PairingCompleted() when pairingCompleted != null:
return pairingCompleted(_that.deviceId,_that.threadId,_that.name,_that.colour);case CoreEvent_PairingFailed() when pairingFailed != null:
return pairingFailed(_that.deviceId,_that.reason);case CoreEvent_RadioChanged() when radioChanged != null:
return radioChanged(_that.available,_that.reason);case _:
  return null;

}
}

}

/// @nodoc


class CoreEvent_DiscoveryUpdated extends CoreEvent {
  const CoreEvent_DiscoveryUpdated({required final  List<NearbyDevice> devices}): _devices = devices,super._();
  

 final  List<NearbyDevice> _devices;
 List<NearbyDevice> get devices {
  if (_devices is EqualUnmodifiableListView) return _devices;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_devices);
}


/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CoreEvent_DiscoveryUpdatedCopyWith<CoreEvent_DiscoveryUpdated> get copyWith => _$CoreEvent_DiscoveryUpdatedCopyWithImpl<CoreEvent_DiscoveryUpdated>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent_DiscoveryUpdated&&const DeepCollectionEquality().equals(other._devices, _devices));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(_devices));

@override
String toString() {
  return 'CoreEvent.discoveryUpdated(devices: $devices)';
}


}

/// @nodoc
abstract mixin class $CoreEvent_DiscoveryUpdatedCopyWith<$Res> implements $CoreEventCopyWith<$Res> {
  factory $CoreEvent_DiscoveryUpdatedCopyWith(CoreEvent_DiscoveryUpdated value, $Res Function(CoreEvent_DiscoveryUpdated) _then) = _$CoreEvent_DiscoveryUpdatedCopyWithImpl;
@useResult
$Res call({
 List<NearbyDevice> devices
});




}
/// @nodoc
class _$CoreEvent_DiscoveryUpdatedCopyWithImpl<$Res>
    implements $CoreEvent_DiscoveryUpdatedCopyWith<$Res> {
  _$CoreEvent_DiscoveryUpdatedCopyWithImpl(this._self, this._then);

  final CoreEvent_DiscoveryUpdated _self;
  final $Res Function(CoreEvent_DiscoveryUpdated) _then;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? devices = null,}) {
  return _then(CoreEvent_DiscoveryUpdated(
devices: null == devices ? _self._devices : devices // ignore: cast_nullable_to_non_nullable
as List<NearbyDevice>,
  ));
}


}

/// @nodoc


class CoreEvent_Pinged extends CoreEvent {
  const CoreEvent_Pinged({required this.deviceId, required this.name}): super._();
  

 final  String deviceId;
 final  String name;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CoreEvent_PingedCopyWith<CoreEvent_Pinged> get copyWith => _$CoreEvent_PingedCopyWithImpl<CoreEvent_Pinged>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent_Pinged&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId)&&(identical(other.name, name) || other.name == name));
}


@override
int get hashCode => Object.hash(runtimeType,deviceId,name);

@override
String toString() {
  return 'CoreEvent.pinged(deviceId: $deviceId, name: $name)';
}


}

/// @nodoc
abstract mixin class $CoreEvent_PingedCopyWith<$Res> implements $CoreEventCopyWith<$Res> {
  factory $CoreEvent_PingedCopyWith(CoreEvent_Pinged value, $Res Function(CoreEvent_Pinged) _then) = _$CoreEvent_PingedCopyWithImpl;
@useResult
$Res call({
 String deviceId, String name
});




}
/// @nodoc
class _$CoreEvent_PingedCopyWithImpl<$Res>
    implements $CoreEvent_PingedCopyWith<$Res> {
  _$CoreEvent_PingedCopyWithImpl(this._self, this._then);

  final CoreEvent_Pinged _self;
  final $Res Function(CoreEvent_Pinged) _then;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? deviceId = null,Object? name = null,}) {
  return _then(CoreEvent_Pinged(
deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,name: null == name ? _self.name : name // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class CoreEvent_PingAcked extends CoreEvent {
  const CoreEvent_PingAcked({required this.deviceId}): super._();
  

 final  String deviceId;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CoreEvent_PingAckedCopyWith<CoreEvent_PingAcked> get copyWith => _$CoreEvent_PingAckedCopyWithImpl<CoreEvent_PingAcked>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent_PingAcked&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId));
}


@override
int get hashCode => Object.hash(runtimeType,deviceId);

@override
String toString() {
  return 'CoreEvent.pingAcked(deviceId: $deviceId)';
}


}

/// @nodoc
abstract mixin class $CoreEvent_PingAckedCopyWith<$Res> implements $CoreEventCopyWith<$Res> {
  factory $CoreEvent_PingAckedCopyWith(CoreEvent_PingAcked value, $Res Function(CoreEvent_PingAcked) _then) = _$CoreEvent_PingAckedCopyWithImpl;
@useResult
$Res call({
 String deviceId
});




}
/// @nodoc
class _$CoreEvent_PingAckedCopyWithImpl<$Res>
    implements $CoreEvent_PingAckedCopyWith<$Res> {
  _$CoreEvent_PingAckedCopyWithImpl(this._self, this._then);

  final CoreEvent_PingAcked _self;
  final $Res Function(CoreEvent_PingAcked) _then;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? deviceId = null,}) {
  return _then(CoreEvent_PingAcked(
deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class CoreEvent_PingFailed extends CoreEvent {
  const CoreEvent_PingFailed({required this.deviceId, required this.reason}): super._();
  

 final  String deviceId;
 final  String reason;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CoreEvent_PingFailedCopyWith<CoreEvent_PingFailed> get copyWith => _$CoreEvent_PingFailedCopyWithImpl<CoreEvent_PingFailed>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent_PingFailed&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId)&&(identical(other.reason, reason) || other.reason == reason));
}


@override
int get hashCode => Object.hash(runtimeType,deviceId,reason);

@override
String toString() {
  return 'CoreEvent.pingFailed(deviceId: $deviceId, reason: $reason)';
}


}

/// @nodoc
abstract mixin class $CoreEvent_PingFailedCopyWith<$Res> implements $CoreEventCopyWith<$Res> {
  factory $CoreEvent_PingFailedCopyWith(CoreEvent_PingFailed value, $Res Function(CoreEvent_PingFailed) _then) = _$CoreEvent_PingFailedCopyWithImpl;
@useResult
$Res call({
 String deviceId, String reason
});




}
/// @nodoc
class _$CoreEvent_PingFailedCopyWithImpl<$Res>
    implements $CoreEvent_PingFailedCopyWith<$Res> {
  _$CoreEvent_PingFailedCopyWithImpl(this._self, this._then);

  final CoreEvent_PingFailed _self;
  final $Res Function(CoreEvent_PingFailed) _then;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? deviceId = null,Object? reason = null,}) {
  return _then(CoreEvent_PingFailed(
deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,reason: null == reason ? _self.reason : reason // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class CoreEvent_MessageReceived extends CoreEvent {
  const CoreEvent_MessageReceived({required this.threadId, required this.msgId, required this.text}): super._();
  

 final  PlatformInt64 threadId;
 final  String msgId;
 final  String text;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CoreEvent_MessageReceivedCopyWith<CoreEvent_MessageReceived> get copyWith => _$CoreEvent_MessageReceivedCopyWithImpl<CoreEvent_MessageReceived>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent_MessageReceived&&(identical(other.threadId, threadId) || other.threadId == threadId)&&(identical(other.msgId, msgId) || other.msgId == msgId)&&(identical(other.text, text) || other.text == text));
}


@override
int get hashCode => Object.hash(runtimeType,threadId,msgId,text);

@override
String toString() {
  return 'CoreEvent.messageReceived(threadId: $threadId, msgId: $msgId, text: $text)';
}


}

/// @nodoc
abstract mixin class $CoreEvent_MessageReceivedCopyWith<$Res> implements $CoreEventCopyWith<$Res> {
  factory $CoreEvent_MessageReceivedCopyWith(CoreEvent_MessageReceived value, $Res Function(CoreEvent_MessageReceived) _then) = _$CoreEvent_MessageReceivedCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 threadId, String msgId, String text
});




}
/// @nodoc
class _$CoreEvent_MessageReceivedCopyWithImpl<$Res>
    implements $CoreEvent_MessageReceivedCopyWith<$Res> {
  _$CoreEvent_MessageReceivedCopyWithImpl(this._self, this._then);

  final CoreEvent_MessageReceived _self;
  final $Res Function(CoreEvent_MessageReceived) _then;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? threadId = null,Object? msgId = null,Object? text = null,}) {
  return _then(CoreEvent_MessageReceived(
threadId: null == threadId ? _self.threadId : threadId // ignore: cast_nullable_to_non_nullable
as PlatformInt64,msgId: null == msgId ? _self.msgId : msgId // ignore: cast_nullable_to_non_nullable
as String,text: null == text ? _self.text : text // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class CoreEvent_TransferProgress extends CoreEvent {
  const CoreEvent_TransferProgress({required this.transferId, required this.received, required this.total}): super._();
  

 final  String transferId;
 final  BigInt received;
 final  BigInt total;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CoreEvent_TransferProgressCopyWith<CoreEvent_TransferProgress> get copyWith => _$CoreEvent_TransferProgressCopyWithImpl<CoreEvent_TransferProgress>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent_TransferProgress&&(identical(other.transferId, transferId) || other.transferId == transferId)&&(identical(other.received, received) || other.received == received)&&(identical(other.total, total) || other.total == total));
}


@override
int get hashCode => Object.hash(runtimeType,transferId,received,total);

@override
String toString() {
  return 'CoreEvent.transferProgress(transferId: $transferId, received: $received, total: $total)';
}


}

/// @nodoc
abstract mixin class $CoreEvent_TransferProgressCopyWith<$Res> implements $CoreEventCopyWith<$Res> {
  factory $CoreEvent_TransferProgressCopyWith(CoreEvent_TransferProgress value, $Res Function(CoreEvent_TransferProgress) _then) = _$CoreEvent_TransferProgressCopyWithImpl;
@useResult
$Res call({
 String transferId, BigInt received, BigInt total
});




}
/// @nodoc
class _$CoreEvent_TransferProgressCopyWithImpl<$Res>
    implements $CoreEvent_TransferProgressCopyWith<$Res> {
  _$CoreEvent_TransferProgressCopyWithImpl(this._self, this._then);

  final CoreEvent_TransferProgress _self;
  final $Res Function(CoreEvent_TransferProgress) _then;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? transferId = null,Object? received = null,Object? total = null,}) {
  return _then(CoreEvent_TransferProgress(
transferId: null == transferId ? _self.transferId : transferId // ignore: cast_nullable_to_non_nullable
as String,received: null == received ? _self.received : received // ignore: cast_nullable_to_non_nullable
as BigInt,total: null == total ? _self.total : total // ignore: cast_nullable_to_non_nullable
as BigInt,
  ));
}


}

/// @nodoc


class CoreEvent_TransferCompleted extends CoreEvent {
  const CoreEvent_TransferCompleted({required this.transferId, required this.success}): super._();
  

 final  String transferId;
 final  bool success;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CoreEvent_TransferCompletedCopyWith<CoreEvent_TransferCompleted> get copyWith => _$CoreEvent_TransferCompletedCopyWithImpl<CoreEvent_TransferCompleted>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent_TransferCompleted&&(identical(other.transferId, transferId) || other.transferId == transferId)&&(identical(other.success, success) || other.success == success));
}


@override
int get hashCode => Object.hash(runtimeType,transferId,success);

@override
String toString() {
  return 'CoreEvent.transferCompleted(transferId: $transferId, success: $success)';
}


}

/// @nodoc
abstract mixin class $CoreEvent_TransferCompletedCopyWith<$Res> implements $CoreEventCopyWith<$Res> {
  factory $CoreEvent_TransferCompletedCopyWith(CoreEvent_TransferCompleted value, $Res Function(CoreEvent_TransferCompleted) _then) = _$CoreEvent_TransferCompletedCopyWithImpl;
@useResult
$Res call({
 String transferId, bool success
});




}
/// @nodoc
class _$CoreEvent_TransferCompletedCopyWithImpl<$Res>
    implements $CoreEvent_TransferCompletedCopyWith<$Res> {
  _$CoreEvent_TransferCompletedCopyWithImpl(this._self, this._then);

  final CoreEvent_TransferCompleted _self;
  final $Res Function(CoreEvent_TransferCompleted) _then;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? transferId = null,Object? success = null,}) {
  return _then(CoreEvent_TransferCompleted(
transferId: null == transferId ? _self.transferId : transferId // ignore: cast_nullable_to_non_nullable
as String,success: null == success ? _self.success : success // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class CoreEvent_PairingSas extends CoreEvent {
  const CoreEvent_PairingSas({required this.deviceId, required this.sas}): super._();
  

 final  String deviceId;
 final  SasDto sas;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CoreEvent_PairingSasCopyWith<CoreEvent_PairingSas> get copyWith => _$CoreEvent_PairingSasCopyWithImpl<CoreEvent_PairingSas>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent_PairingSas&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId)&&(identical(other.sas, sas) || other.sas == sas));
}


@override
int get hashCode => Object.hash(runtimeType,deviceId,sas);

@override
String toString() {
  return 'CoreEvent.pairingSas(deviceId: $deviceId, sas: $sas)';
}


}

/// @nodoc
abstract mixin class $CoreEvent_PairingSasCopyWith<$Res> implements $CoreEventCopyWith<$Res> {
  factory $CoreEvent_PairingSasCopyWith(CoreEvent_PairingSas value, $Res Function(CoreEvent_PairingSas) _then) = _$CoreEvent_PairingSasCopyWithImpl;
@useResult
$Res call({
 String deviceId, SasDto sas
});




}
/// @nodoc
class _$CoreEvent_PairingSasCopyWithImpl<$Res>
    implements $CoreEvent_PairingSasCopyWith<$Res> {
  _$CoreEvent_PairingSasCopyWithImpl(this._self, this._then);

  final CoreEvent_PairingSas _self;
  final $Res Function(CoreEvent_PairingSas) _then;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? deviceId = null,Object? sas = null,}) {
  return _then(CoreEvent_PairingSas(
deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,sas: null == sas ? _self.sas : sas // ignore: cast_nullable_to_non_nullable
as SasDto,
  ));
}


}

/// @nodoc


class CoreEvent_PairingPeerConfirmed extends CoreEvent {
  const CoreEvent_PairingPeerConfirmed({required this.deviceId}): super._();
  

 final  String deviceId;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CoreEvent_PairingPeerConfirmedCopyWith<CoreEvent_PairingPeerConfirmed> get copyWith => _$CoreEvent_PairingPeerConfirmedCopyWithImpl<CoreEvent_PairingPeerConfirmed>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent_PairingPeerConfirmed&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId));
}


@override
int get hashCode => Object.hash(runtimeType,deviceId);

@override
String toString() {
  return 'CoreEvent.pairingPeerConfirmed(deviceId: $deviceId)';
}


}

/// @nodoc
abstract mixin class $CoreEvent_PairingPeerConfirmedCopyWith<$Res> implements $CoreEventCopyWith<$Res> {
  factory $CoreEvent_PairingPeerConfirmedCopyWith(CoreEvent_PairingPeerConfirmed value, $Res Function(CoreEvent_PairingPeerConfirmed) _then) = _$CoreEvent_PairingPeerConfirmedCopyWithImpl;
@useResult
$Res call({
 String deviceId
});




}
/// @nodoc
class _$CoreEvent_PairingPeerConfirmedCopyWithImpl<$Res>
    implements $CoreEvent_PairingPeerConfirmedCopyWith<$Res> {
  _$CoreEvent_PairingPeerConfirmedCopyWithImpl(this._self, this._then);

  final CoreEvent_PairingPeerConfirmed _self;
  final $Res Function(CoreEvent_PairingPeerConfirmed) _then;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? deviceId = null,}) {
  return _then(CoreEvent_PairingPeerConfirmed(
deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class CoreEvent_PairingCompleted extends CoreEvent {
  const CoreEvent_PairingCompleted({required this.deviceId, required this.threadId, required this.name, required this.colour}): super._();
  

 final  String deviceId;
 final  PlatformInt64 threadId;
 final  String name;
 final  int colour;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CoreEvent_PairingCompletedCopyWith<CoreEvent_PairingCompleted> get copyWith => _$CoreEvent_PairingCompletedCopyWithImpl<CoreEvent_PairingCompleted>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent_PairingCompleted&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId)&&(identical(other.threadId, threadId) || other.threadId == threadId)&&(identical(other.name, name) || other.name == name)&&(identical(other.colour, colour) || other.colour == colour));
}


@override
int get hashCode => Object.hash(runtimeType,deviceId,threadId,name,colour);

@override
String toString() {
  return 'CoreEvent.pairingCompleted(deviceId: $deviceId, threadId: $threadId, name: $name, colour: $colour)';
}


}

/// @nodoc
abstract mixin class $CoreEvent_PairingCompletedCopyWith<$Res> implements $CoreEventCopyWith<$Res> {
  factory $CoreEvent_PairingCompletedCopyWith(CoreEvent_PairingCompleted value, $Res Function(CoreEvent_PairingCompleted) _then) = _$CoreEvent_PairingCompletedCopyWithImpl;
@useResult
$Res call({
 String deviceId, PlatformInt64 threadId, String name, int colour
});




}
/// @nodoc
class _$CoreEvent_PairingCompletedCopyWithImpl<$Res>
    implements $CoreEvent_PairingCompletedCopyWith<$Res> {
  _$CoreEvent_PairingCompletedCopyWithImpl(this._self, this._then);

  final CoreEvent_PairingCompleted _self;
  final $Res Function(CoreEvent_PairingCompleted) _then;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? deviceId = null,Object? threadId = null,Object? name = null,Object? colour = null,}) {
  return _then(CoreEvent_PairingCompleted(
deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,threadId: null == threadId ? _self.threadId : threadId // ignore: cast_nullable_to_non_nullable
as PlatformInt64,name: null == name ? _self.name : name // ignore: cast_nullable_to_non_nullable
as String,colour: null == colour ? _self.colour : colour // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class CoreEvent_PairingFailed extends CoreEvent {
  const CoreEvent_PairingFailed({required this.deviceId, required this.reason}): super._();
  

 final  String deviceId;
 final  String reason;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CoreEvent_PairingFailedCopyWith<CoreEvent_PairingFailed> get copyWith => _$CoreEvent_PairingFailedCopyWithImpl<CoreEvent_PairingFailed>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent_PairingFailed&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId)&&(identical(other.reason, reason) || other.reason == reason));
}


@override
int get hashCode => Object.hash(runtimeType,deviceId,reason);

@override
String toString() {
  return 'CoreEvent.pairingFailed(deviceId: $deviceId, reason: $reason)';
}


}

/// @nodoc
abstract mixin class $CoreEvent_PairingFailedCopyWith<$Res> implements $CoreEventCopyWith<$Res> {
  factory $CoreEvent_PairingFailedCopyWith(CoreEvent_PairingFailed value, $Res Function(CoreEvent_PairingFailed) _then) = _$CoreEvent_PairingFailedCopyWithImpl;
@useResult
$Res call({
 String deviceId, String reason
});




}
/// @nodoc
class _$CoreEvent_PairingFailedCopyWithImpl<$Res>
    implements $CoreEvent_PairingFailedCopyWith<$Res> {
  _$CoreEvent_PairingFailedCopyWithImpl(this._self, this._then);

  final CoreEvent_PairingFailed _self;
  final $Res Function(CoreEvent_PairingFailed) _then;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? deviceId = null,Object? reason = null,}) {
  return _then(CoreEvent_PairingFailed(
deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,reason: null == reason ? _self.reason : reason // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class CoreEvent_RadioChanged extends CoreEvent {
  const CoreEvent_RadioChanged({required this.available, this.reason}): super._();
  

 final  bool available;
 final  String? reason;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CoreEvent_RadioChangedCopyWith<CoreEvent_RadioChanged> get copyWith => _$CoreEvent_RadioChangedCopyWithImpl<CoreEvent_RadioChanged>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CoreEvent_RadioChanged&&(identical(other.available, available) || other.available == available)&&(identical(other.reason, reason) || other.reason == reason));
}


@override
int get hashCode => Object.hash(runtimeType,available,reason);

@override
String toString() {
  return 'CoreEvent.radioChanged(available: $available, reason: $reason)';
}


}

/// @nodoc
abstract mixin class $CoreEvent_RadioChangedCopyWith<$Res> implements $CoreEventCopyWith<$Res> {
  factory $CoreEvent_RadioChangedCopyWith(CoreEvent_RadioChanged value, $Res Function(CoreEvent_RadioChanged) _then) = _$CoreEvent_RadioChangedCopyWithImpl;
@useResult
$Res call({
 bool available, String? reason
});




}
/// @nodoc
class _$CoreEvent_RadioChangedCopyWithImpl<$Res>
    implements $CoreEvent_RadioChangedCopyWith<$Res> {
  _$CoreEvent_RadioChangedCopyWithImpl(this._self, this._then);

  final CoreEvent_RadioChanged _self;
  final $Res Function(CoreEvent_RadioChanged) _then;

/// Create a copy of CoreEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? available = null,Object? reason = freezed,}) {
  return _then(CoreEvent_RadioChanged(
available: null == available ? _self.available : available // ignore: cast_nullable_to_non_nullable
as bool,reason: freezed == reason ? _self.reason : reason // ignore: cast_nullable_to_non_nullable
as String?,
  ));
}


}

// dart format on
