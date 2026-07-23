import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/main.dart';
import 'package:hoppler/src/rust/frb_generated.dart';
import 'package:integration_test/integration_test.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async => await RustLib.init());
  testWidgets('Rust core is reachable and reports its version', (WidgetTester tester) async {
    await tester.pumpWidget(const HopplerApp());
    expect(find.textContaining('core: libhoppler'), findsOneWidget);
  });
}
