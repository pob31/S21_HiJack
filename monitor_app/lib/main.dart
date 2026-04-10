import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'models/monitor_client.dart';
import 'services/osc_service.dart';
import 'screens/connection_screen.dart';

void main() async {
  WidgetsFlutterBinding.ensureInitialized();

  final osc = OscService();
  await osc.bind();

  runApp(
    ChangeNotifierProvider(
      create: (_) => MonitorClientModel(),
      child: S21MonitorApp(osc: osc),
    ),
  );
}

class S21MonitorApp extends StatelessWidget {
  final OscService osc;
  const S21MonitorApp({super.key, required this.osc});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'S21 Monitor',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        brightness: Brightness.dark,
        colorSchemeSeed: Colors.blue,
        useMaterial3: true,
      ),
      home: ConnectionScreen(osc: osc),
    );
  }
}
