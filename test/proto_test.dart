import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hoppler/src/gen/hello.pb.dart';

void main() {
  // Golden bytes shared with the Rust test (rust/src/proto.rs):
  // field 1, varint 42 -> [0x08, 0x2A]
  test('PingFrame round-trip matches golden bytes', () {
    final bytes = (PingFrame()..nonce = Int64(42)).writeToBuffer();
    expect(bytes, [0x08, 0x2A]);

    final decoded = PingFrame.fromBuffer(bytes);
    expect(decoded.nonce.toInt(), 42);
  });
}
