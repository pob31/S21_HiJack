import 'package:flutter/foundation.dart';
import 'send_state.dart';

/// Connection state of the monitor app.
enum ConnectionStatus { disconnected, connecting, connected }

/// The app's state model, provided via ChangeNotifier to the widget tree.
class MonitorClientModel extends ChangeNotifier {
  String clientName;
  String daemonHost = '';
  int daemonPort = 8025;
  String consoleName = '';
  ConnectionStatus status = ConnectionStatus.disconnected;

  /// Permitted aux channels (populated from daemon config).
  List<int> permittedAuxes = [];

  /// Currently selected aux for the "My Mix" tab.
  int? selectedAux;

  /// Send states keyed by (inputCh, auxCh).
  final Map<(int, int), SendState> sends = {};

  /// Aux output states keyed by auxCh.
  final Map<int, AuxState> auxStates = {};

  /// Tech mode: unlocks all auxes.
  bool techMode = false;

  MonitorClientModel({this.clientName = ''});

  void setConnected(String host, int port, String console) {
    daemonHost = host;
    daemonPort = port;
    consoleName = console;
    status = ConnectionStatus.connected;
    notifyListeners();
  }

  void setDisconnected() {
    status = ConnectionStatus.disconnected;
    notifyListeners();
  }

  void updateSend(int inputCh, int auxCh, double level, double pan, bool on) {
    final key = (inputCh, auxCh);
    final state = sends.putIfAbsent(
      key,
      () => SendState(inputCh: inputCh, auxCh: auxCh),
    );
    state.level = level;
    state.pan = pan;
    state.on = on;
    notifyListeners();
  }

  void updateAux(int auxCh, double fader, bool mute) {
    final state = auxStates.putIfAbsent(
      auxCh,
      () => AuxState(auxCh: auxCh),
    );
    state.fader = fader;
    state.mute = mute;
    notifyListeners();
  }

  /// Get all sends for the currently selected aux, sorted by input channel.
  List<SendState> sendsForSelectedAux() {
    if (selectedAux == null) return [];
    final aux = selectedAux!;
    return sends.entries
        .where((e) => e.key.$2 == aux)
        .map((e) => e.value)
        .toList()
      ..sort((a, b) => a.inputCh.compareTo(b.inputCh));
  }

  /// Available auxes (permitted or all in tech mode).
  List<int> get availableAuxes => techMode
      ? List.generate(24, (i) => i + 1) // show all in tech mode
      : permittedAuxes;
}
