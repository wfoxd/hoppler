import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/src/platform/permissions.dart';

/// The Dart half of the permission channel.
///
/// The case worth guarding is the one with no plugin at all: Linux desktop
/// registers no handler, and a `MissingPluginException` read as "denied" would
/// turn a working rung into a refusal — Discovery would refuse to switch on, on
/// the platform where nothing was ever in the way.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const channel = MethodChannel('org.hoppler/permissions');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  void answer(Future<Object?>? Function(MethodCall) handler) {
    messenger.setMockMethodCallHandler(channel, handler);
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
  }

  test('a granted permission lets the radio run', () async {
    answer((call) async => call.method == 'ensureRadio' ? true : null);
    expect(await ensureRadioPermission(), isTrue);
  });

  test('a refusal is reported rather than thrown', () async {
    answer((_) async => false);
    expect(
      await ensureRadioPermission(),
      isFalse,
      reason: 'being denied is something a person chose, not a fault',
    );
  });

  test('a platform with no such permission is not a refusal', () async {
    // No handler registered at all, which is Linux desktop.
    expect(
      await ensureRadioPermission(),
      isTrue,
      reason: 'reporting denied here would refuse to switch Discovery on for a '
          'rung with nothing standing in its way',
    );
  });

  test('a null answer is a refusal, not a crash', () async {
    // An older or partial host returning nothing must not take the app down on
    // the switch — the honest reading is "no permission".
    answer((_) async => null);
    expect(await ensureRadioPermission(), isFalse);
  });
}
