import 'dart:async';
import 'osc_service.dart';

/// Result of a daemon discovery.
class DiscoveredDaemon {
  final String host;
  final int port;
  final String consoleName;

  DiscoveredDaemon({
    required this.host,
    required this.port,
    required this.consoleName,
  });

  @override
  String toString() => '$consoleName ($host:$port)';
}

/// Discovers S21_HiJack daemons on the local network via UDP broadcast.
class DiscoveryService {
  final OscService _osc;
  static const int defaultPort = 8025;

  DiscoveryService(this._osc);

  /// Broadcast a discovery probe and collect replies for [timeout] duration.
  Future<List<DiscoveredDaemon>> discover({
    Duration timeout = const Duration(seconds: 3),
  }) async {
    final results = <DiscoveredDaemon>[];
    final completer = Completer<List<DiscoveredDaemon>>();

    final sub = _osc.incoming.listen((msg) {
      if (msg.address == '/monitor/discovered' && msg.args.isNotEmpty) {
        final name = msg.args[0];
        if (name is OscString) {
          // The reply comes from the daemon's IP; we can extract it
          // from the datagram source, but since OscService doesn't
          // expose the source address, we'll need the user to confirm.
          // For now, store what we know.
          results.add(DiscoveredDaemon(
            host: '', // Will be populated from datagram source
            port: defaultPort,
            consoleName: name.value,
          ));
        }
      }
    });

    // Send broadcast
    _osc.broadcast(defaultPort, '/monitor/discover', []);

    // Wait for replies
    await Future.delayed(timeout);
    await sub.cancel();

    if (!completer.isCompleted) {
      completer.complete(results);
    }
    return completer.future;
  }
}
