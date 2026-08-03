// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'platform.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$HostCommand {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostCommand);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'HostCommand()';
}


}

/// @nodoc
class $HostCommandCopyWith<$Res>  {
$HostCommandCopyWith(HostCommand _, $Res Function(HostCommand) __);
}


/// Adds pattern-matching-related methods to [HostCommand].
extension HostCommandPatterns on HostCommand {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( HostCommand_BleSetLocalId value)?  bleSetLocalId,TResult Function( HostCommand_BleStartAdvertising value)?  bleStartAdvertising,TResult Function( HostCommand_BleStopAdvertising value)?  bleStopAdvertising,TResult Function( HostCommand_BleStartScanning value)?  bleStartScanning,TResult Function( HostCommand_BleStopScanning value)?  bleStopScanning,TResult Function( HostCommand_BleConnect value)?  bleConnect,TResult Function( HostCommand_BleSend value)?  bleSend,TResult Function( HostCommand_BleDisconnect value)?  bleDisconnect,TResult Function( HostCommand_BleShutdown value)?  bleShutdown,required TResult orElse(),}){
final _that = this;
switch (_that) {
case HostCommand_BleSetLocalId() when bleSetLocalId != null:
return bleSetLocalId(_that);case HostCommand_BleStartAdvertising() when bleStartAdvertising != null:
return bleStartAdvertising(_that);case HostCommand_BleStopAdvertising() when bleStopAdvertising != null:
return bleStopAdvertising(_that);case HostCommand_BleStartScanning() when bleStartScanning != null:
return bleStartScanning(_that);case HostCommand_BleStopScanning() when bleStopScanning != null:
return bleStopScanning(_that);case HostCommand_BleConnect() when bleConnect != null:
return bleConnect(_that);case HostCommand_BleSend() when bleSend != null:
return bleSend(_that);case HostCommand_BleDisconnect() when bleDisconnect != null:
return bleDisconnect(_that);case HostCommand_BleShutdown() when bleShutdown != null:
return bleShutdown(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( HostCommand_BleSetLocalId value)  bleSetLocalId,required TResult Function( HostCommand_BleStartAdvertising value)  bleStartAdvertising,required TResult Function( HostCommand_BleStopAdvertising value)  bleStopAdvertising,required TResult Function( HostCommand_BleStartScanning value)  bleStartScanning,required TResult Function( HostCommand_BleStopScanning value)  bleStopScanning,required TResult Function( HostCommand_BleConnect value)  bleConnect,required TResult Function( HostCommand_BleSend value)  bleSend,required TResult Function( HostCommand_BleDisconnect value)  bleDisconnect,required TResult Function( HostCommand_BleShutdown value)  bleShutdown,}){
final _that = this;
switch (_that) {
case HostCommand_BleSetLocalId():
return bleSetLocalId(_that);case HostCommand_BleStartAdvertising():
return bleStartAdvertising(_that);case HostCommand_BleStopAdvertising():
return bleStopAdvertising(_that);case HostCommand_BleStartScanning():
return bleStartScanning(_that);case HostCommand_BleStopScanning():
return bleStopScanning(_that);case HostCommand_BleConnect():
return bleConnect(_that);case HostCommand_BleSend():
return bleSend(_that);case HostCommand_BleDisconnect():
return bleDisconnect(_that);case HostCommand_BleShutdown():
return bleShutdown(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( HostCommand_BleSetLocalId value)?  bleSetLocalId,TResult? Function( HostCommand_BleStartAdvertising value)?  bleStartAdvertising,TResult? Function( HostCommand_BleStopAdvertising value)?  bleStopAdvertising,TResult? Function( HostCommand_BleStartScanning value)?  bleStartScanning,TResult? Function( HostCommand_BleStopScanning value)?  bleStopScanning,TResult? Function( HostCommand_BleConnect value)?  bleConnect,TResult? Function( HostCommand_BleSend value)?  bleSend,TResult? Function( HostCommand_BleDisconnect value)?  bleDisconnect,TResult? Function( HostCommand_BleShutdown value)?  bleShutdown,}){
final _that = this;
switch (_that) {
case HostCommand_BleSetLocalId() when bleSetLocalId != null:
return bleSetLocalId(_that);case HostCommand_BleStartAdvertising() when bleStartAdvertising != null:
return bleStartAdvertising(_that);case HostCommand_BleStopAdvertising() when bleStopAdvertising != null:
return bleStopAdvertising(_that);case HostCommand_BleStartScanning() when bleStartScanning != null:
return bleStartScanning(_that);case HostCommand_BleStopScanning() when bleStopScanning != null:
return bleStopScanning(_that);case HostCommand_BleConnect() when bleConnect != null:
return bleConnect(_that);case HostCommand_BleSend() when bleSend != null:
return bleSend(_that);case HostCommand_BleDisconnect() when bleDisconnect != null:
return bleDisconnect(_that);case HostCommand_BleShutdown() when bleShutdown != null:
return bleShutdown(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String localId)?  bleSetLocalId,TResult Function( Uint8List payload)?  bleStartAdvertising,TResult Function()?  bleStopAdvertising,TResult Function()?  bleStartScanning,TResult Function()?  bleStopScanning,TResult Function( String peer)?  bleConnect,TResult Function( String peer,  Uint8List bytes)?  bleSend,TResult Function( String peer)?  bleDisconnect,TResult Function()?  bleShutdown,required TResult orElse(),}) {final _that = this;
switch (_that) {
case HostCommand_BleSetLocalId() when bleSetLocalId != null:
return bleSetLocalId(_that.localId);case HostCommand_BleStartAdvertising() when bleStartAdvertising != null:
return bleStartAdvertising(_that.payload);case HostCommand_BleStopAdvertising() when bleStopAdvertising != null:
return bleStopAdvertising();case HostCommand_BleStartScanning() when bleStartScanning != null:
return bleStartScanning();case HostCommand_BleStopScanning() when bleStopScanning != null:
return bleStopScanning();case HostCommand_BleConnect() when bleConnect != null:
return bleConnect(_that.peer);case HostCommand_BleSend() when bleSend != null:
return bleSend(_that.peer,_that.bytes);case HostCommand_BleDisconnect() when bleDisconnect != null:
return bleDisconnect(_that.peer);case HostCommand_BleShutdown() when bleShutdown != null:
return bleShutdown();case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String localId)  bleSetLocalId,required TResult Function( Uint8List payload)  bleStartAdvertising,required TResult Function()  bleStopAdvertising,required TResult Function()  bleStartScanning,required TResult Function()  bleStopScanning,required TResult Function( String peer)  bleConnect,required TResult Function( String peer,  Uint8List bytes)  bleSend,required TResult Function( String peer)  bleDisconnect,required TResult Function()  bleShutdown,}) {final _that = this;
switch (_that) {
case HostCommand_BleSetLocalId():
return bleSetLocalId(_that.localId);case HostCommand_BleStartAdvertising():
return bleStartAdvertising(_that.payload);case HostCommand_BleStopAdvertising():
return bleStopAdvertising();case HostCommand_BleStartScanning():
return bleStartScanning();case HostCommand_BleStopScanning():
return bleStopScanning();case HostCommand_BleConnect():
return bleConnect(_that.peer);case HostCommand_BleSend():
return bleSend(_that.peer,_that.bytes);case HostCommand_BleDisconnect():
return bleDisconnect(_that.peer);case HostCommand_BleShutdown():
return bleShutdown();}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String localId)?  bleSetLocalId,TResult? Function( Uint8List payload)?  bleStartAdvertising,TResult? Function()?  bleStopAdvertising,TResult? Function()?  bleStartScanning,TResult? Function()?  bleStopScanning,TResult? Function( String peer)?  bleConnect,TResult? Function( String peer,  Uint8List bytes)?  bleSend,TResult? Function( String peer)?  bleDisconnect,TResult? Function()?  bleShutdown,}) {final _that = this;
switch (_that) {
case HostCommand_BleSetLocalId() when bleSetLocalId != null:
return bleSetLocalId(_that.localId);case HostCommand_BleStartAdvertising() when bleStartAdvertising != null:
return bleStartAdvertising(_that.payload);case HostCommand_BleStopAdvertising() when bleStopAdvertising != null:
return bleStopAdvertising();case HostCommand_BleStartScanning() when bleStartScanning != null:
return bleStartScanning();case HostCommand_BleStopScanning() when bleStopScanning != null:
return bleStopScanning();case HostCommand_BleConnect() when bleConnect != null:
return bleConnect(_that.peer);case HostCommand_BleSend() when bleSend != null:
return bleSend(_that.peer,_that.bytes);case HostCommand_BleDisconnect() when bleDisconnect != null:
return bleDisconnect(_that.peer);case HostCommand_BleShutdown() when bleShutdown != null:
return bleShutdown();case _:
  return null;

}
}

}

/// @nodoc


class HostCommand_BleSetLocalId extends HostCommand {
  const HostCommand_BleSetLocalId({required this.localId}): super._();
  

 final  String localId;

/// Create a copy of HostCommand
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostCommand_BleSetLocalIdCopyWith<HostCommand_BleSetLocalId> get copyWith => _$HostCommand_BleSetLocalIdCopyWithImpl<HostCommand_BleSetLocalId>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostCommand_BleSetLocalId&&(identical(other.localId, localId) || other.localId == localId));
}


@override
int get hashCode => Object.hash(runtimeType,localId);

@override
String toString() {
  return 'HostCommand.bleSetLocalId(localId: $localId)';
}


}

/// @nodoc
abstract mixin class $HostCommand_BleSetLocalIdCopyWith<$Res> implements $HostCommandCopyWith<$Res> {
  factory $HostCommand_BleSetLocalIdCopyWith(HostCommand_BleSetLocalId value, $Res Function(HostCommand_BleSetLocalId) _then) = _$HostCommand_BleSetLocalIdCopyWithImpl;
@useResult
$Res call({
 String localId
});




}
/// @nodoc
class _$HostCommand_BleSetLocalIdCopyWithImpl<$Res>
    implements $HostCommand_BleSetLocalIdCopyWith<$Res> {
  _$HostCommand_BleSetLocalIdCopyWithImpl(this._self, this._then);

  final HostCommand_BleSetLocalId _self;
  final $Res Function(HostCommand_BleSetLocalId) _then;

/// Create a copy of HostCommand
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? localId = null,}) {
  return _then(HostCommand_BleSetLocalId(
localId: null == localId ? _self.localId : localId // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class HostCommand_BleStartAdvertising extends HostCommand {
  const HostCommand_BleStartAdvertising({required this.payload}): super._();
  

 final  Uint8List payload;

/// Create a copy of HostCommand
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostCommand_BleStartAdvertisingCopyWith<HostCommand_BleStartAdvertising> get copyWith => _$HostCommand_BleStartAdvertisingCopyWithImpl<HostCommand_BleStartAdvertising>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostCommand_BleStartAdvertising&&const DeepCollectionEquality().equals(other.payload, payload));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(payload));

@override
String toString() {
  return 'HostCommand.bleStartAdvertising(payload: $payload)';
}


}

/// @nodoc
abstract mixin class $HostCommand_BleStartAdvertisingCopyWith<$Res> implements $HostCommandCopyWith<$Res> {
  factory $HostCommand_BleStartAdvertisingCopyWith(HostCommand_BleStartAdvertising value, $Res Function(HostCommand_BleStartAdvertising) _then) = _$HostCommand_BleStartAdvertisingCopyWithImpl;
@useResult
$Res call({
 Uint8List payload
});




}
/// @nodoc
class _$HostCommand_BleStartAdvertisingCopyWithImpl<$Res>
    implements $HostCommand_BleStartAdvertisingCopyWith<$Res> {
  _$HostCommand_BleStartAdvertisingCopyWithImpl(this._self, this._then);

  final HostCommand_BleStartAdvertising _self;
  final $Res Function(HostCommand_BleStartAdvertising) _then;

/// Create a copy of HostCommand
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? payload = null,}) {
  return _then(HostCommand_BleStartAdvertising(
payload: null == payload ? _self.payload : payload // ignore: cast_nullable_to_non_nullable
as Uint8List,
  ));
}


}

/// @nodoc


class HostCommand_BleStopAdvertising extends HostCommand {
  const HostCommand_BleStopAdvertising(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostCommand_BleStopAdvertising);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'HostCommand.bleStopAdvertising()';
}


}




/// @nodoc


class HostCommand_BleStartScanning extends HostCommand {
  const HostCommand_BleStartScanning(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostCommand_BleStartScanning);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'HostCommand.bleStartScanning()';
}


}




/// @nodoc


class HostCommand_BleStopScanning extends HostCommand {
  const HostCommand_BleStopScanning(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostCommand_BleStopScanning);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'HostCommand.bleStopScanning()';
}


}




/// @nodoc


class HostCommand_BleConnect extends HostCommand {
  const HostCommand_BleConnect({required this.peer}): super._();
  

 final  String peer;

/// Create a copy of HostCommand
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostCommand_BleConnectCopyWith<HostCommand_BleConnect> get copyWith => _$HostCommand_BleConnectCopyWithImpl<HostCommand_BleConnect>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostCommand_BleConnect&&(identical(other.peer, peer) || other.peer == peer));
}


@override
int get hashCode => Object.hash(runtimeType,peer);

@override
String toString() {
  return 'HostCommand.bleConnect(peer: $peer)';
}


}

/// @nodoc
abstract mixin class $HostCommand_BleConnectCopyWith<$Res> implements $HostCommandCopyWith<$Res> {
  factory $HostCommand_BleConnectCopyWith(HostCommand_BleConnect value, $Res Function(HostCommand_BleConnect) _then) = _$HostCommand_BleConnectCopyWithImpl;
@useResult
$Res call({
 String peer
});




}
/// @nodoc
class _$HostCommand_BleConnectCopyWithImpl<$Res>
    implements $HostCommand_BleConnectCopyWith<$Res> {
  _$HostCommand_BleConnectCopyWithImpl(this._self, this._then);

  final HostCommand_BleConnect _self;
  final $Res Function(HostCommand_BleConnect) _then;

/// Create a copy of HostCommand
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? peer = null,}) {
  return _then(HostCommand_BleConnect(
peer: null == peer ? _self.peer : peer // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class HostCommand_BleSend extends HostCommand {
  const HostCommand_BleSend({required this.peer, required this.bytes}): super._();
  

 final  String peer;
 final  Uint8List bytes;

/// Create a copy of HostCommand
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostCommand_BleSendCopyWith<HostCommand_BleSend> get copyWith => _$HostCommand_BleSendCopyWithImpl<HostCommand_BleSend>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostCommand_BleSend&&(identical(other.peer, peer) || other.peer == peer)&&const DeepCollectionEquality().equals(other.bytes, bytes));
}


@override
int get hashCode => Object.hash(runtimeType,peer,const DeepCollectionEquality().hash(bytes));

@override
String toString() {
  return 'HostCommand.bleSend(peer: $peer, bytes: $bytes)';
}


}

/// @nodoc
abstract mixin class $HostCommand_BleSendCopyWith<$Res> implements $HostCommandCopyWith<$Res> {
  factory $HostCommand_BleSendCopyWith(HostCommand_BleSend value, $Res Function(HostCommand_BleSend) _then) = _$HostCommand_BleSendCopyWithImpl;
@useResult
$Res call({
 String peer, Uint8List bytes
});




}
/// @nodoc
class _$HostCommand_BleSendCopyWithImpl<$Res>
    implements $HostCommand_BleSendCopyWith<$Res> {
  _$HostCommand_BleSendCopyWithImpl(this._self, this._then);

  final HostCommand_BleSend _self;
  final $Res Function(HostCommand_BleSend) _then;

/// Create a copy of HostCommand
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? peer = null,Object? bytes = null,}) {
  return _then(HostCommand_BleSend(
peer: null == peer ? _self.peer : peer // ignore: cast_nullable_to_non_nullable
as String,bytes: null == bytes ? _self.bytes : bytes // ignore: cast_nullable_to_non_nullable
as Uint8List,
  ));
}


}

/// @nodoc


class HostCommand_BleDisconnect extends HostCommand {
  const HostCommand_BleDisconnect({required this.peer}): super._();
  

 final  String peer;

/// Create a copy of HostCommand
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostCommand_BleDisconnectCopyWith<HostCommand_BleDisconnect> get copyWith => _$HostCommand_BleDisconnectCopyWithImpl<HostCommand_BleDisconnect>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostCommand_BleDisconnect&&(identical(other.peer, peer) || other.peer == peer));
}


@override
int get hashCode => Object.hash(runtimeType,peer);

@override
String toString() {
  return 'HostCommand.bleDisconnect(peer: $peer)';
}


}

/// @nodoc
abstract mixin class $HostCommand_BleDisconnectCopyWith<$Res> implements $HostCommandCopyWith<$Res> {
  factory $HostCommand_BleDisconnectCopyWith(HostCommand_BleDisconnect value, $Res Function(HostCommand_BleDisconnect) _then) = _$HostCommand_BleDisconnectCopyWithImpl;
@useResult
$Res call({
 String peer
});




}
/// @nodoc
class _$HostCommand_BleDisconnectCopyWithImpl<$Res>
    implements $HostCommand_BleDisconnectCopyWith<$Res> {
  _$HostCommand_BleDisconnectCopyWithImpl(this._self, this._then);

  final HostCommand_BleDisconnect _self;
  final $Res Function(HostCommand_BleDisconnect) _then;

/// Create a copy of HostCommand
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? peer = null,}) {
  return _then(HostCommand_BleDisconnect(
peer: null == peer ? _self.peer : peer // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class HostCommand_BleShutdown extends HostCommand {
  const HostCommand_BleShutdown(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostCommand_BleShutdown);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'HostCommand.bleShutdown()';
}


}




/// @nodoc
mixin _$HostFact {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostFact);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'HostFact()';
}


}

/// @nodoc
class $HostFactCopyWith<$Res>  {
$HostFactCopyWith(HostFact _, $Res Function(HostFact) __);
}


/// Adds pattern-matching-related methods to [HostFact].
extension HostFactPatterns on HostFact {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( HostFact_BlePeerFound value)?  blePeerFound,TResult Function( HostFact_BlePeerLost value)?  blePeerLost,TResult Function( HostFact_BlePipeOpened value)?  blePipeOpened,TResult Function( HostFact_BlePipeFailed value)?  blePipeFailed,TResult Function( HostFact_BlePipeClosed value)?  blePipeClosed,TResult Function( HostFact_BleReceived value)?  bleReceived,TResult Function( HostFact_BleAvailability value)?  bleAvailability,TResult Function( HostFact_BleWriteComplete value)?  bleWriteComplete,required TResult orElse(),}){
final _that = this;
switch (_that) {
case HostFact_BlePeerFound() when blePeerFound != null:
return blePeerFound(_that);case HostFact_BlePeerLost() when blePeerLost != null:
return blePeerLost(_that);case HostFact_BlePipeOpened() when blePipeOpened != null:
return blePipeOpened(_that);case HostFact_BlePipeFailed() when blePipeFailed != null:
return blePipeFailed(_that);case HostFact_BlePipeClosed() when blePipeClosed != null:
return blePipeClosed(_that);case HostFact_BleReceived() when bleReceived != null:
return bleReceived(_that);case HostFact_BleAvailability() when bleAvailability != null:
return bleAvailability(_that);case HostFact_BleWriteComplete() when bleWriteComplete != null:
return bleWriteComplete(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( HostFact_BlePeerFound value)  blePeerFound,required TResult Function( HostFact_BlePeerLost value)  blePeerLost,required TResult Function( HostFact_BlePipeOpened value)  blePipeOpened,required TResult Function( HostFact_BlePipeFailed value)  blePipeFailed,required TResult Function( HostFact_BlePipeClosed value)  blePipeClosed,required TResult Function( HostFact_BleReceived value)  bleReceived,required TResult Function( HostFact_BleAvailability value)  bleAvailability,required TResult Function( HostFact_BleWriteComplete value)  bleWriteComplete,}){
final _that = this;
switch (_that) {
case HostFact_BlePeerFound():
return blePeerFound(_that);case HostFact_BlePeerLost():
return blePeerLost(_that);case HostFact_BlePipeOpened():
return blePipeOpened(_that);case HostFact_BlePipeFailed():
return blePipeFailed(_that);case HostFact_BlePipeClosed():
return blePipeClosed(_that);case HostFact_BleReceived():
return bleReceived(_that);case HostFact_BleAvailability():
return bleAvailability(_that);case HostFact_BleWriteComplete():
return bleWriteComplete(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( HostFact_BlePeerFound value)?  blePeerFound,TResult? Function( HostFact_BlePeerLost value)?  blePeerLost,TResult? Function( HostFact_BlePipeOpened value)?  blePipeOpened,TResult? Function( HostFact_BlePipeFailed value)?  blePipeFailed,TResult? Function( HostFact_BlePipeClosed value)?  blePipeClosed,TResult? Function( HostFact_BleReceived value)?  bleReceived,TResult? Function( HostFact_BleAvailability value)?  bleAvailability,TResult? Function( HostFact_BleWriteComplete value)?  bleWriteComplete,}){
final _that = this;
switch (_that) {
case HostFact_BlePeerFound() when blePeerFound != null:
return blePeerFound(_that);case HostFact_BlePeerLost() when blePeerLost != null:
return blePeerLost(_that);case HostFact_BlePipeOpened() when blePipeOpened != null:
return blePipeOpened(_that);case HostFact_BlePipeFailed() when blePipeFailed != null:
return blePipeFailed(_that);case HostFact_BlePipeClosed() when blePipeClosed != null:
return blePipeClosed(_that);case HostFact_BleReceived() when bleReceived != null:
return bleReceived(_that);case HostFact_BleAvailability() when bleAvailability != null:
return bleAvailability(_that);case HostFact_BleWriteComplete() when bleWriteComplete != null:
return bleWriteComplete(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String peer,  Uint8List payload)?  blePeerFound,TResult Function( String peer)?  blePeerLost,TResult Function( String peer)?  blePipeOpened,TResult Function( String peer,  String why)?  blePipeFailed,TResult Function( String peer)?  blePipeClosed,TResult Function( String peer,  Uint8List bytes)?  bleReceived,TResult Function( bool available,  String? reason)?  bleAvailability,TResult Function( String peer,  BigInt bytes)?  bleWriteComplete,required TResult orElse(),}) {final _that = this;
switch (_that) {
case HostFact_BlePeerFound() when blePeerFound != null:
return blePeerFound(_that.peer,_that.payload);case HostFact_BlePeerLost() when blePeerLost != null:
return blePeerLost(_that.peer);case HostFact_BlePipeOpened() when blePipeOpened != null:
return blePipeOpened(_that.peer);case HostFact_BlePipeFailed() when blePipeFailed != null:
return blePipeFailed(_that.peer,_that.why);case HostFact_BlePipeClosed() when blePipeClosed != null:
return blePipeClosed(_that.peer);case HostFact_BleReceived() when bleReceived != null:
return bleReceived(_that.peer,_that.bytes);case HostFact_BleAvailability() when bleAvailability != null:
return bleAvailability(_that.available,_that.reason);case HostFact_BleWriteComplete() when bleWriteComplete != null:
return bleWriteComplete(_that.peer,_that.bytes);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String peer,  Uint8List payload)  blePeerFound,required TResult Function( String peer)  blePeerLost,required TResult Function( String peer)  blePipeOpened,required TResult Function( String peer,  String why)  blePipeFailed,required TResult Function( String peer)  blePipeClosed,required TResult Function( String peer,  Uint8List bytes)  bleReceived,required TResult Function( bool available,  String? reason)  bleAvailability,required TResult Function( String peer,  BigInt bytes)  bleWriteComplete,}) {final _that = this;
switch (_that) {
case HostFact_BlePeerFound():
return blePeerFound(_that.peer,_that.payload);case HostFact_BlePeerLost():
return blePeerLost(_that.peer);case HostFact_BlePipeOpened():
return blePipeOpened(_that.peer);case HostFact_BlePipeFailed():
return blePipeFailed(_that.peer,_that.why);case HostFact_BlePipeClosed():
return blePipeClosed(_that.peer);case HostFact_BleReceived():
return bleReceived(_that.peer,_that.bytes);case HostFact_BleAvailability():
return bleAvailability(_that.available,_that.reason);case HostFact_BleWriteComplete():
return bleWriteComplete(_that.peer,_that.bytes);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String peer,  Uint8List payload)?  blePeerFound,TResult? Function( String peer)?  blePeerLost,TResult? Function( String peer)?  blePipeOpened,TResult? Function( String peer,  String why)?  blePipeFailed,TResult? Function( String peer)?  blePipeClosed,TResult? Function( String peer,  Uint8List bytes)?  bleReceived,TResult? Function( bool available,  String? reason)?  bleAvailability,TResult? Function( String peer,  BigInt bytes)?  bleWriteComplete,}) {final _that = this;
switch (_that) {
case HostFact_BlePeerFound() when blePeerFound != null:
return blePeerFound(_that.peer,_that.payload);case HostFact_BlePeerLost() when blePeerLost != null:
return blePeerLost(_that.peer);case HostFact_BlePipeOpened() when blePipeOpened != null:
return blePipeOpened(_that.peer);case HostFact_BlePipeFailed() when blePipeFailed != null:
return blePipeFailed(_that.peer,_that.why);case HostFact_BlePipeClosed() when blePipeClosed != null:
return blePipeClosed(_that.peer);case HostFact_BleReceived() when bleReceived != null:
return bleReceived(_that.peer,_that.bytes);case HostFact_BleAvailability() when bleAvailability != null:
return bleAvailability(_that.available,_that.reason);case HostFact_BleWriteComplete() when bleWriteComplete != null:
return bleWriteComplete(_that.peer,_that.bytes);case _:
  return null;

}
}

}

/// @nodoc


class HostFact_BlePeerFound extends HostFact {
  const HostFact_BlePeerFound({required this.peer, required this.payload}): super._();
  

 final  String peer;
 final  Uint8List payload;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostFact_BlePeerFoundCopyWith<HostFact_BlePeerFound> get copyWith => _$HostFact_BlePeerFoundCopyWithImpl<HostFact_BlePeerFound>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostFact_BlePeerFound&&(identical(other.peer, peer) || other.peer == peer)&&const DeepCollectionEquality().equals(other.payload, payload));
}


@override
int get hashCode => Object.hash(runtimeType,peer,const DeepCollectionEquality().hash(payload));

@override
String toString() {
  return 'HostFact.blePeerFound(peer: $peer, payload: $payload)';
}


}

/// @nodoc
abstract mixin class $HostFact_BlePeerFoundCopyWith<$Res> implements $HostFactCopyWith<$Res> {
  factory $HostFact_BlePeerFoundCopyWith(HostFact_BlePeerFound value, $Res Function(HostFact_BlePeerFound) _then) = _$HostFact_BlePeerFoundCopyWithImpl;
@useResult
$Res call({
 String peer, Uint8List payload
});




}
/// @nodoc
class _$HostFact_BlePeerFoundCopyWithImpl<$Res>
    implements $HostFact_BlePeerFoundCopyWith<$Res> {
  _$HostFact_BlePeerFoundCopyWithImpl(this._self, this._then);

  final HostFact_BlePeerFound _self;
  final $Res Function(HostFact_BlePeerFound) _then;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? peer = null,Object? payload = null,}) {
  return _then(HostFact_BlePeerFound(
peer: null == peer ? _self.peer : peer // ignore: cast_nullable_to_non_nullable
as String,payload: null == payload ? _self.payload : payload // ignore: cast_nullable_to_non_nullable
as Uint8List,
  ));
}


}

/// @nodoc


class HostFact_BlePeerLost extends HostFact {
  const HostFact_BlePeerLost({required this.peer}): super._();
  

 final  String peer;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostFact_BlePeerLostCopyWith<HostFact_BlePeerLost> get copyWith => _$HostFact_BlePeerLostCopyWithImpl<HostFact_BlePeerLost>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostFact_BlePeerLost&&(identical(other.peer, peer) || other.peer == peer));
}


@override
int get hashCode => Object.hash(runtimeType,peer);

@override
String toString() {
  return 'HostFact.blePeerLost(peer: $peer)';
}


}

/// @nodoc
abstract mixin class $HostFact_BlePeerLostCopyWith<$Res> implements $HostFactCopyWith<$Res> {
  factory $HostFact_BlePeerLostCopyWith(HostFact_BlePeerLost value, $Res Function(HostFact_BlePeerLost) _then) = _$HostFact_BlePeerLostCopyWithImpl;
@useResult
$Res call({
 String peer
});




}
/// @nodoc
class _$HostFact_BlePeerLostCopyWithImpl<$Res>
    implements $HostFact_BlePeerLostCopyWith<$Res> {
  _$HostFact_BlePeerLostCopyWithImpl(this._self, this._then);

  final HostFact_BlePeerLost _self;
  final $Res Function(HostFact_BlePeerLost) _then;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? peer = null,}) {
  return _then(HostFact_BlePeerLost(
peer: null == peer ? _self.peer : peer // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class HostFact_BlePipeOpened extends HostFact {
  const HostFact_BlePipeOpened({required this.peer}): super._();
  

 final  String peer;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostFact_BlePipeOpenedCopyWith<HostFact_BlePipeOpened> get copyWith => _$HostFact_BlePipeOpenedCopyWithImpl<HostFact_BlePipeOpened>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostFact_BlePipeOpened&&(identical(other.peer, peer) || other.peer == peer));
}


@override
int get hashCode => Object.hash(runtimeType,peer);

@override
String toString() {
  return 'HostFact.blePipeOpened(peer: $peer)';
}


}

/// @nodoc
abstract mixin class $HostFact_BlePipeOpenedCopyWith<$Res> implements $HostFactCopyWith<$Res> {
  factory $HostFact_BlePipeOpenedCopyWith(HostFact_BlePipeOpened value, $Res Function(HostFact_BlePipeOpened) _then) = _$HostFact_BlePipeOpenedCopyWithImpl;
@useResult
$Res call({
 String peer
});




}
/// @nodoc
class _$HostFact_BlePipeOpenedCopyWithImpl<$Res>
    implements $HostFact_BlePipeOpenedCopyWith<$Res> {
  _$HostFact_BlePipeOpenedCopyWithImpl(this._self, this._then);

  final HostFact_BlePipeOpened _self;
  final $Res Function(HostFact_BlePipeOpened) _then;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? peer = null,}) {
  return _then(HostFact_BlePipeOpened(
peer: null == peer ? _self.peer : peer // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class HostFact_BlePipeFailed extends HostFact {
  const HostFact_BlePipeFailed({required this.peer, required this.why}): super._();
  

 final  String peer;
 final  String why;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostFact_BlePipeFailedCopyWith<HostFact_BlePipeFailed> get copyWith => _$HostFact_BlePipeFailedCopyWithImpl<HostFact_BlePipeFailed>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostFact_BlePipeFailed&&(identical(other.peer, peer) || other.peer == peer)&&(identical(other.why, why) || other.why == why));
}


@override
int get hashCode => Object.hash(runtimeType,peer,why);

@override
String toString() {
  return 'HostFact.blePipeFailed(peer: $peer, why: $why)';
}


}

/// @nodoc
abstract mixin class $HostFact_BlePipeFailedCopyWith<$Res> implements $HostFactCopyWith<$Res> {
  factory $HostFact_BlePipeFailedCopyWith(HostFact_BlePipeFailed value, $Res Function(HostFact_BlePipeFailed) _then) = _$HostFact_BlePipeFailedCopyWithImpl;
@useResult
$Res call({
 String peer, String why
});




}
/// @nodoc
class _$HostFact_BlePipeFailedCopyWithImpl<$Res>
    implements $HostFact_BlePipeFailedCopyWith<$Res> {
  _$HostFact_BlePipeFailedCopyWithImpl(this._self, this._then);

  final HostFact_BlePipeFailed _self;
  final $Res Function(HostFact_BlePipeFailed) _then;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? peer = null,Object? why = null,}) {
  return _then(HostFact_BlePipeFailed(
peer: null == peer ? _self.peer : peer // ignore: cast_nullable_to_non_nullable
as String,why: null == why ? _self.why : why // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class HostFact_BlePipeClosed extends HostFact {
  const HostFact_BlePipeClosed({required this.peer}): super._();
  

 final  String peer;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostFact_BlePipeClosedCopyWith<HostFact_BlePipeClosed> get copyWith => _$HostFact_BlePipeClosedCopyWithImpl<HostFact_BlePipeClosed>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostFact_BlePipeClosed&&(identical(other.peer, peer) || other.peer == peer));
}


@override
int get hashCode => Object.hash(runtimeType,peer);

@override
String toString() {
  return 'HostFact.blePipeClosed(peer: $peer)';
}


}

/// @nodoc
abstract mixin class $HostFact_BlePipeClosedCopyWith<$Res> implements $HostFactCopyWith<$Res> {
  factory $HostFact_BlePipeClosedCopyWith(HostFact_BlePipeClosed value, $Res Function(HostFact_BlePipeClosed) _then) = _$HostFact_BlePipeClosedCopyWithImpl;
@useResult
$Res call({
 String peer
});




}
/// @nodoc
class _$HostFact_BlePipeClosedCopyWithImpl<$Res>
    implements $HostFact_BlePipeClosedCopyWith<$Res> {
  _$HostFact_BlePipeClosedCopyWithImpl(this._self, this._then);

  final HostFact_BlePipeClosed _self;
  final $Res Function(HostFact_BlePipeClosed) _then;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? peer = null,}) {
  return _then(HostFact_BlePipeClosed(
peer: null == peer ? _self.peer : peer // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class HostFact_BleReceived extends HostFact {
  const HostFact_BleReceived({required this.peer, required this.bytes}): super._();
  

 final  String peer;
 final  Uint8List bytes;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostFact_BleReceivedCopyWith<HostFact_BleReceived> get copyWith => _$HostFact_BleReceivedCopyWithImpl<HostFact_BleReceived>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostFact_BleReceived&&(identical(other.peer, peer) || other.peer == peer)&&const DeepCollectionEquality().equals(other.bytes, bytes));
}


@override
int get hashCode => Object.hash(runtimeType,peer,const DeepCollectionEquality().hash(bytes));

@override
String toString() {
  return 'HostFact.bleReceived(peer: $peer, bytes: $bytes)';
}


}

/// @nodoc
abstract mixin class $HostFact_BleReceivedCopyWith<$Res> implements $HostFactCopyWith<$Res> {
  factory $HostFact_BleReceivedCopyWith(HostFact_BleReceived value, $Res Function(HostFact_BleReceived) _then) = _$HostFact_BleReceivedCopyWithImpl;
@useResult
$Res call({
 String peer, Uint8List bytes
});




}
/// @nodoc
class _$HostFact_BleReceivedCopyWithImpl<$Res>
    implements $HostFact_BleReceivedCopyWith<$Res> {
  _$HostFact_BleReceivedCopyWithImpl(this._self, this._then);

  final HostFact_BleReceived _self;
  final $Res Function(HostFact_BleReceived) _then;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? peer = null,Object? bytes = null,}) {
  return _then(HostFact_BleReceived(
peer: null == peer ? _self.peer : peer // ignore: cast_nullable_to_non_nullable
as String,bytes: null == bytes ? _self.bytes : bytes // ignore: cast_nullable_to_non_nullable
as Uint8List,
  ));
}


}

/// @nodoc


class HostFact_BleAvailability extends HostFact {
  const HostFact_BleAvailability({required this.available, this.reason}): super._();
  

 final  bool available;
 final  String? reason;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostFact_BleAvailabilityCopyWith<HostFact_BleAvailability> get copyWith => _$HostFact_BleAvailabilityCopyWithImpl<HostFact_BleAvailability>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostFact_BleAvailability&&(identical(other.available, available) || other.available == available)&&(identical(other.reason, reason) || other.reason == reason));
}


@override
int get hashCode => Object.hash(runtimeType,available,reason);

@override
String toString() {
  return 'HostFact.bleAvailability(available: $available, reason: $reason)';
}


}

/// @nodoc
abstract mixin class $HostFact_BleAvailabilityCopyWith<$Res> implements $HostFactCopyWith<$Res> {
  factory $HostFact_BleAvailabilityCopyWith(HostFact_BleAvailability value, $Res Function(HostFact_BleAvailability) _then) = _$HostFact_BleAvailabilityCopyWithImpl;
@useResult
$Res call({
 bool available, String? reason
});




}
/// @nodoc
class _$HostFact_BleAvailabilityCopyWithImpl<$Res>
    implements $HostFact_BleAvailabilityCopyWith<$Res> {
  _$HostFact_BleAvailabilityCopyWithImpl(this._self, this._then);

  final HostFact_BleAvailability _self;
  final $Res Function(HostFact_BleAvailability) _then;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? available = null,Object? reason = freezed,}) {
  return _then(HostFact_BleAvailability(
available: null == available ? _self.available : available // ignore: cast_nullable_to_non_nullable
as bool,reason: freezed == reason ? _self.reason : reason // ignore: cast_nullable_to_non_nullable
as String?,
  ));
}


}

/// @nodoc


class HostFact_BleWriteComplete extends HostFact {
  const HostFact_BleWriteComplete({required this.peer, required this.bytes}): super._();
  

 final  String peer;
 final  BigInt bytes;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HostFact_BleWriteCompleteCopyWith<HostFact_BleWriteComplete> get copyWith => _$HostFact_BleWriteCompleteCopyWithImpl<HostFact_BleWriteComplete>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is HostFact_BleWriteComplete&&(identical(other.peer, peer) || other.peer == peer)&&(identical(other.bytes, bytes) || other.bytes == bytes));
}


@override
int get hashCode => Object.hash(runtimeType,peer,bytes);

@override
String toString() {
  return 'HostFact.bleWriteComplete(peer: $peer, bytes: $bytes)';
}


}

/// @nodoc
abstract mixin class $HostFact_BleWriteCompleteCopyWith<$Res> implements $HostFactCopyWith<$Res> {
  factory $HostFact_BleWriteCompleteCopyWith(HostFact_BleWriteComplete value, $Res Function(HostFact_BleWriteComplete) _then) = _$HostFact_BleWriteCompleteCopyWithImpl;
@useResult
$Res call({
 String peer, BigInt bytes
});




}
/// @nodoc
class _$HostFact_BleWriteCompleteCopyWithImpl<$Res>
    implements $HostFact_BleWriteCompleteCopyWith<$Res> {
  _$HostFact_BleWriteCompleteCopyWithImpl(this._self, this._then);

  final HostFact_BleWriteComplete _self;
  final $Res Function(HostFact_BleWriteComplete) _then;

/// Create a copy of HostFact
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? peer = null,Object? bytes = null,}) {
  return _then(HostFact_BleWriteComplete(
peer: null == peer ? _self.peer : peer // ignore: cast_nullable_to_non_nullable
as String,bytes: null == bytes ? _self.bytes : bytes // ignore: cast_nullable_to_non_nullable
as BigInt,
  ));
}


}

// dart format on
