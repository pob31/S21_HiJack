import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models/monitor_client.dart';
import '../services/credentials_store.dart';
import '../services/osc_bridge.dart';
import '../services/osc_service.dart';
import 'monitor_screen.dart';

class ConnectionScreen extends StatefulWidget {
  final OscBridge bridge;
  const ConnectionScreen({super.key, required this.bridge});

  @override
  State<ConnectionScreen> createState() => _ConnectionScreenState();
}

class _ConnectionScreenState extends State<ConnectionScreen> {
  late final TextEditingController _hostController;
  late final TextEditingController _portController;
  late final TextEditingController _nameController;
  bool _starting = false;
  bool _discovering = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _hostController = TextEditingController();
    _portController = TextEditingController(text: '8025');
    _nameController = TextEditingController();
    _hydrate();
  }

  Future<void> _hydrate() async {
    final stored = await CredentialsStore.load();
    if (!mounted || stored == null) return;
    setState(() {
      _hostController.text = stored.host;
      _portController.text = stored.port.toString();
      _nameController.text = stored.name;
    });
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: const Color(0xFF0D0D0D),
      body: Center(
        child: Container(
          constraints: const BoxConstraints(maxWidth: 400),
          padding: const EdgeInsets.all(32),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                'S21 Monitor',
                style: Theme.of(context).textTheme.headlineMedium?.copyWith(
                      color: Colors.white,
                      fontWeight: FontWeight.bold,
                    ),
                textAlign: TextAlign.center,
              ),
              const SizedBox(height: 8),
              Text(
                'Personal monitor mixer',
                style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                      color: Colors.white54,
                    ),
                textAlign: TextAlign.center,
              ),
              const SizedBox(height: 32),

              // Client name
              TextField(
                controller: _nameController,
                decoration: const InputDecoration(
                  labelText: 'Your name',
                  hintText: 'e.g. Drums, Keys, Vocals',
                  border: OutlineInputBorder(),
                  filled: true,
                  fillColor: Color(0xFF1A1A1A),
                ),
                style: const TextStyle(color: Colors.white),
              ),
              const SizedBox(height: 16),

              // Host
              TextField(
                controller: _hostController,
                decoration: const InputDecoration(
                  labelText: 'Daemon IP address',
                  hintText: 'e.g. 192.168.1.100',
                  border: OutlineInputBorder(),
                  filled: true,
                  fillColor: Color(0xFF1A1A1A),
                ),
                style: const TextStyle(color: Colors.white),
                keyboardType: TextInputType.url,
              ),
              const SizedBox(height: 16),

              // Port
              TextField(
                controller: _portController,
                decoration: const InputDecoration(
                  labelText: 'Port',
                  border: OutlineInputBorder(),
                  filled: true,
                  fillColor: Color(0xFF1A1A1A),
                ),
                style: const TextStyle(color: Colors.white),
                keyboardType: TextInputType.number,
              ),
              const SizedBox(height: 24),

              if (_error != null)
                Padding(
                  padding: const EdgeInsets.only(bottom: 16),
                  child: Text(
                    _error!,
                    style: const TextStyle(color: Colors.redAccent),
                    textAlign: TextAlign.center,
                  ),
                ),

              // Connect button
              FilledButton.icon(
                onPressed: _starting ? null : _connect,
                icon: _starting
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.wifi),
                label: Text(_starting ? 'Starting…' : 'Connect'),
                style: FilledButton.styleFrom(
                  minimumSize: const Size(double.infinity, 48),
                ),
              ),
              const SizedBox(height: 12),

              OutlinedButton.icon(
                onPressed: _discovering ? null : _discover,
                icon: _discovering
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.search),
                label: Text(_discovering ? 'Searching…' : 'Auto-discover'),
                style: OutlinedButton.styleFrom(
                  minimumSize: const Size(double.infinity, 48),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  /// Broadcast `/monitor/discover` on the LAN and auto-fill the host /
  /// port fields from the daemon's reply. Uses a throwaway OscService
  /// so it doesn't compete with the (not-yet-started) background
  /// isolate's socket.
  Future<void> _discover() async {
    setState(() {
      _discovering = true;
      _error = null;
    });

    final probe = OscService();
    await probe.bind();
    try {
      probe.broadcast(8025, '/monitor/discover', []);
      final msg = await probe.incoming
          .where((m) => m.address == '/monitor/discovered')
          .first
          .timeout(const Duration(seconds: 3));

      if (msg.args.isNotEmpty && msg.args[0] is OscString) {
        final consoleName = (msg.args[0] as OscString).value;
        int? payloadPort;
        if (msg.args.length >= 2 && msg.args[1] is OscInt) {
          payloadPort = (msg.args[1] as OscInt).value;
        }
        final host = msg.sourceHost;
        final port = payloadPort ?? msg.sourcePort;

        if (host != null && port != null) {
          setState(() {
            _hostController.text = host;
            _portController.text = port.toString();
            _error = 'Found "$consoleName" at $host:$port';
            _discovering = false;
          });
        } else {
          setState(() {
            _error = 'Found "$consoleName" — please enter IP manually';
            _discovering = false;
          });
        }
      }
    } on TimeoutException {
      setState(() {
        _error = 'No daemon found on the network';
        _discovering = false;
      });
    } catch (_) {
      setState(() {
        _error = 'Discovery failed';
        _discovering = false;
      });
    } finally {
      probe.dispose();
    }
  }

  Future<void> _connect() async {
    final name = _nameController.text.trim();
    final host = _hostController.text.trim();
    final port = int.tryParse(_portController.text.trim()) ?? 8025;

    if (name.isEmpty) {
      setState(() => _error = 'Please enter your name');
      return;
    }
    if (host.isEmpty) {
      setState(() => _error = 'Please enter the daemon IP address');
      return;
    }

    setState(() {
      _error = null;
      _starting = true;
    });

    final creds = Credentials(host: host, port: port, name: name);
    final ok = await widget.bridge.startWith(creds);

    if (!mounted) return;
    if (!ok) {
      setState(() {
        _starting = false;
        _error = 'Failed to start the background service';
      });
      return;
    }

    final model = context.read<MonitorClientModel>();
    model.clientName = name;
    model.setConnected(host, port, '');

    // Background service is up; ask it for the current snapshot. The
    // MonitorScreen will subscribe to bridge.events on mount and pick up
    // whatever arrives.
    widget.bridge.requestSnapshot();

    if (!mounted) return;
    Navigator.of(context).pushReplacement(
      MaterialPageRoute(
        builder: (_) => MonitorScreen(bridge: widget.bridge),
      ),
    );
  }

  @override
  void dispose() {
    _hostController.dispose();
    _portController.dispose();
    _nameController.dispose();
    super.dispose();
  }
}
