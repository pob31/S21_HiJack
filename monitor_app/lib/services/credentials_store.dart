import 'package:shared_preferences/shared_preferences.dart';

/// Connection details that survive app kill so the user doesn't have to
/// re-enter them on every launch. Cleared explicitly via the in-app
/// shutdown button.
class Credentials {
  final String host;
  final int port;
  final String name;

  const Credentials({
    required this.host,
    required this.port,
    required this.name,
  });

  bool get isComplete => host.isNotEmpty && port > 0 && name.isNotEmpty;
}

class CredentialsStore {
  static const _kHost = 's21.host';
  static const _kPort = 's21.port';
  static const _kName = 's21.name';

  /// Returns null if any field is missing — i.e. user has never completed
  /// a Connect flow.
  static Future<Credentials?> load() async {
    final prefs = await SharedPreferences.getInstance();
    final host = prefs.getString(_kHost) ?? '';
    final port = prefs.getInt(_kPort) ?? 0;
    final name = prefs.getString(_kName) ?? '';
    final c = Credentials(host: host, port: port, name: name);
    return c.isComplete ? c : null;
  }

  static Future<void> save(Credentials c) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kHost, c.host);
    await prefs.setInt(_kPort, c.port);
    await prefs.setString(_kName, c.name);
  }

  static Future<void> clear() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.remove(_kHost);
    await prefs.remove(_kPort);
    await prefs.remove(_kName);
  }
}
