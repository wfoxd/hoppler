import 'package:flutter/material.dart';
import 'package:hoppler/src/rust/api/core.dart';
import 'package:hoppler/src/rust/frb_generated.dart';

Future<void> main() async {
  await RustLib.init();
  runApp(const HopplerApp());
}

class HopplerApp extends StatelessWidget {
  const HopplerApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Hoppler',
      theme: ThemeData(colorScheme: ColorScheme.fromSeed(seedColor: Colors.orange)),
      home: Scaffold(
        appBar: AppBar(title: const Text('Hoppler')),
        body: Center(child: Text('core: ${coreVersion()}')),
      ),
    );
  }
}
