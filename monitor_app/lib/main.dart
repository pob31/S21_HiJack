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

class S21MonitorApp extends StatefulWidget {
  final OscService osc;
  const S21MonitorApp({super.key, required this.osc});

  @override
  State<S21MonitorApp> createState() => _S21MonitorAppState();
}

class _S21MonitorAppState extends State<S21MonitorApp> with WidgetsBindingObserver {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    widget.osc.dispose();
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.detached) {
      widget.osc.dispose();
    } else if (state == AppLifecycleState.resumed) {
      // Rebind socket if it was closed
      if (!widget.osc.isBound) {
        widget.osc.bind();
      }
    }
  }

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
      home: ConnectionScreen(osc: widget.osc),
    );
  }
}
