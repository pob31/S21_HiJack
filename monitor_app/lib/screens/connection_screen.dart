import 'dart:async';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../models/monitor_client.dart';
import '../services/osc_service.dart';
import 'monitor_screen.dart';

class ConnectionScreen extends StatefulWidget {
  final OscService osc;
  const ConnectionScreen({super.key, required this.osc});

  @override
  State<ConnectionScreen> createState() => _ConnectionScreenState();
}

  // Persist last connection across navigation (survives disconnect/reconnect)
  static String _lastHost = '';
  static String _lastPort = '8025';
  static String _lastName = '';

class _ConnectionScreenState extends State<ConnectionScreen> {
  late final TextEditingController _hostController;
  late final TextEditingController _portController;
  late final TextEditingController _nameController;
  bool _discovering = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _hostController = TextEditingController(text: _lastHost);
    _portController = TextEditingController(text: _lastPort);
    _nameController = TextEditingController(text: _lastName);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: const Color(0xFF1A1A2E),
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
                  fillColor: Color(0xFF16213E),
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
                  fillColor: Color(0xFF16213E),
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
                  fillColor: Color(0xFF16213E),
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
                onPressed: _connect,
                icon: const Icon(Icons.wifi),
                label: const Text('Connect'),
                style: FilledButton.styleFrom(
                  minimumSize: const Size(double.infinity, 48),
                ),
              ),
              const SizedBox(height: 12),

              // Discover button
              OutlinedButton.icon(
                onPressed: _discovering ? null : _discover,
                icon: _discovering
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.search),
                label: Text(_discovering ? 'Searching...' : 'Auto-discover'),
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

  void _connect() {
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

    // Save for next time
    _lastHost = host;
    _lastPort = port.toString();
    _lastName = name;

    final model = context.read<MonitorClientModel>();
    model.clientName = name;
    model.setConnected(host, port, '');

    debugPrint('S21 Monitor: connecting to $host:$port as "$name"');

    // Start heartbeat
    widget.osc.startHeartbeat(host, port, name);

    // Send connect (heartbeat will maintain it)
    widget.osc.send(host, port, '/monitor/$name/connect', []);
    debugPrint('S21 Monitor: sent connect to $host:$port as "$name"');

    // Navigate to monitor screen FIRST, then request state
    // so the listener is ready when the state messages arrive.
    if (mounted) {
      Navigator.of(context).pushReplacement(
        MaterialPageRoute(
          builder: (_) => MonitorScreen(osc: widget.osc, requestStateOnMount: true),
        ),
      );
    }
  }

  Future<void> _discover() async {
    setState(() {
      _discovering = true;
      _error = null;
    });

    // Send broadcast
    widget.osc.broadcast(8025, '/monitor/discover', []);

    // Wait for a reply
    try {
      final msg = await widget.osc.incoming
          .where((m) => m.address == '/monitor/discovered')
          .first
          .timeout(const Duration(seconds: 3));

      if (msg.args.isNotEmpty && msg.args[0] is OscString) {
        final consoleName = (msg.args[0] as OscString).value;
        // We got a reply — but we need the source IP.
        // For now, user still needs to enter the IP manually.
        setState(() {
          _error = 'Found "$consoleName" — enter its IP above';
          _discovering = false;
        });
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
    }
  }

  @override
  void dispose() {
    _hostController.dispose();
    _portController.dispose();
    _nameController.dispose();
    super.dispose();
  }
}
