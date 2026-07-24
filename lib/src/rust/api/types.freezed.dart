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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( CoreEvent_DiscoveryUpdated value)?  discoveryUpdated,TResult Function( CoreEvent_Pinged value)?  pinged,TResult Function( CoreEvent_MessageReceived value)?  messageReceived,TResult Function( CoreEvent_TransferProgress value)?  transferProgress,TResult Function( CoreEvent_TransferCompleted value)?  transferCompleted,required TResult orElse(),}){
final _that = this;
switch (_that) {
case CoreEvent_DiscoveryUpdated() when discoveryUpdated != null:
return discoveryUpdated(_that);case CoreEvent_Pinged() when pinged != null:
return pinged(_that);case CoreEvent_MessageReceived() when messageReceived != null:
return messageReceived(_that);case CoreEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that);case CoreEvent_TransferCompleted() when transferCompleted != null:
return transferCompleted(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( CoreEvent_DiscoveryUpdated value)  discoveryUpdated,required TResult Function( CoreEvent_Pinged value)  pinged,required TResult Function( CoreEvent_MessageReceived value)  messageReceived,required TResult Function( CoreEvent_TransferProgress value)  transferProgress,required TResult Function( CoreEvent_TransferCompleted value)  transferCompleted,}){
final _that = this;
switch (_that) {
case CoreEvent_DiscoveryUpdated():
return discoveryUpdated(_that);case CoreEvent_Pinged():
return pinged(_that);case CoreEvent_MessageReceived():
return messageReceived(_that);case CoreEvent_TransferProgress():
return transferProgress(_that);case CoreEvent_TransferCompleted():
return transferCompleted(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( CoreEvent_DiscoveryUpdated value)?  discoveryUpdated,TResult? Function( CoreEvent_Pinged value)?  pinged,TResult? Function( CoreEvent_MessageReceived value)?  messageReceived,TResult? Function( CoreEvent_TransferProgress value)?  transferProgress,TResult? Function( CoreEvent_TransferCompleted value)?  transferCompleted,}){
final _that = this;
switch (_that) {
case CoreEvent_DiscoveryUpdated() when discoveryUpdated != null:
return discoveryUpdated(_that);case CoreEvent_Pinged() when pinged != null:
return pinged(_that);case CoreEvent_MessageReceived() when messageReceived != null:
return messageReceived(_that);case CoreEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that);case CoreEvent_TransferCompleted() when transferCompleted != null:
return transferCompleted(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( List<NearbyDevice> devices)?  discoveryUpdated,TResult Function( String deviceId,  String name)?  pinged,TResult Function( PlatformInt64 threadId,  String msgId,  String text)?  messageReceived,TResult Function( String transferId,  BigInt received,  BigInt total)?  transferProgress,TResult Function( String transferId,  bool success)?  transferCompleted,required TResult orElse(),}) {final _that = this;
switch (_that) {
case CoreEvent_DiscoveryUpdated() when discoveryUpdated != null:
return discoveryUpdated(_that.devices);case CoreEvent_Pinged() when pinged != null:
return pinged(_that.deviceId,_that.name);case CoreEvent_MessageReceived() when messageReceived != null:
return messageReceived(_that.threadId,_that.msgId,_that.text);case CoreEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that.transferId,_that.received,_that.total);case CoreEvent_TransferCompleted() when transferCompleted != null:
return transferCompleted(_that.transferId,_that.success);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( List<NearbyDevice> devices)  discoveryUpdated,required TResult Function( String deviceId,  String name)  pinged,required TResult Function( PlatformInt64 threadId,  String msgId,  String text)  messageReceived,required TResult Function( String transferId,  BigInt received,  BigInt total)  transferProgress,required TResult Function( String transferId,  bool success)  transferCompleted,}) {final _that = this;
switch (_that) {
case CoreEvent_DiscoveryUpdated():
return discoveryUpdated(_that.devices);case CoreEvent_Pinged():
return pinged(_that.deviceId,_that.name);case CoreEvent_MessageReceived():
return messageReceived(_that.threadId,_that.msgId,_that.text);case CoreEvent_TransferProgress():
return transferProgress(_that.transferId,_that.received,_that.total);case CoreEvent_TransferCompleted():
return transferCompleted(_that.transferId,_that.success);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( List<NearbyDevice> devices)?  discoveryUpdated,TResult? Function( String deviceId,  String name)?  pinged,TResult? Function( PlatformInt64 threadId,  String msgId,  String text)?  messageReceived,TResult? Function( String transferId,  BigInt received,  BigInt total)?  transferProgress,TResult? Function( String transferId,  bool success)?  transferCompleted,}) {final _that = this;
switch (_that) {
case CoreEvent_DiscoveryUpdated() when discoveryUpdated != null:
return discoveryUpdated(_that.devices);case CoreEvent_Pinged() when pinged != null:
return pinged(_that.deviceId,_that.name);case CoreEvent_MessageReceived() when messageReceived != null:
return messageReceived(_that.threadId,_that.msgId,_that.text);case CoreEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that.transferId,_that.received,_that.total);case CoreEvent_TransferCompleted() when transferCompleted != null:
return transferCompleted(_that.transferId,_that.success);case _:
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

// dart format on
