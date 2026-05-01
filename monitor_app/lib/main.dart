import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import 'models/monitor_client.dart';
import 'screens/connection_screen.dart';
import 'screens/monitor_screen.dart';
import 'services/background_osc_service.dart';
import 'services/credentials_store.dart';
import 'services/osc_bridge.dart';

void main() async {
  WidgetsFlutterBinding.ensureInitialized();

  await configureBackgroundService();
  final bridge = OscBridge()..attach();

  final creds = await CredentialsStore.load();
  final running = await bridge.isRunning();

  // If we have credentials AND the background service is still alive
  // (typical when the user just backgrounded and re-foregrounded the app),
  // skip the connection screen and go straight to the mixer. Otherwise
  // show the wizard with the controllers prefilled from stored creds.
  final Widget home = (creds != null && running)
      ? MonitorScreen(bridge: bridge)
      : ConnectionScreen(bridge: bridge);

  runApp(
    ChangeNotifierProvider(
      create: (_) => MonitorClientModel(
        clientName: creds?.name ?? '',
      ),
      child: S21MonitorApp(bridge: bridge, home: home),
    ),
  );
}

class S21MonitorApp extends StatefulWidget {
  final OscBridge bridge;
  final Widget home;
  const S21MonitorApp({super.key, required this.bridge, required this.home});

  @override
  State<S21MonitorApp> createState() => _S21MonitorAppState();
}

class _S21MonitorAppState extends State<S21MonitorApp>
    with WidgetsBindingObserver {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    widget.bridge.dispose();
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // On resume, ask the background service for its latest snapshot so the
    // UI repaints immediately with current state — Android keeps the
    // service running, iOS suspends it but the snapshot will arrive once
    // the isolate wakes up and re-pulls from the daemon.
    if (state == AppLifecycleState.resumed) {
      widget.bridge.requestSnapshot();
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
      home: widget.home,
    );
  }
}
