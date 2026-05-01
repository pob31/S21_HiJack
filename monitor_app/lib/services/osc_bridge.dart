import 'dart:async';

import 'package:flutter_background_service/flutter_background_service.dart';

import 'background_osc_service.dart';
import 'credentials_store.dart';

/// One state-change event delivered from the background service to the
/// UI. Decoded into a small set of typed records so the screens don't
/// have to re-parse the IPC `Map`.
sealed class BridgeEvent {
  const BridgeEvent();
}

class SendUpdated extends BridgeEvent {
  final int input;
  final int aux;
  final double level;
  final double pan;
  final bool on;
  const SendUpdated(this.input, this.aux, this.level, this.pan, this.on);
}

class AuxUpdated extends BridgeEvent {
  final int aux;
  final double fader;
  final bool mute;
  const AuxUpdated(this.aux, this.fader, this.mute);
}

class ChannelNamed extends BridgeEvent {
  final String kind; // 'input' | 'aux'
  final int ch;
  final String name;
  const ChannelNamed(this.kind, this.ch, this.name);
}

class StatusChanged extends BridgeEvent {
  final bool connected;
  final String console;
  const StatusChanged(this.connected, this.console);
}

class Snapshot extends BridgeEvent {
  final bool connected;
  final String console;
  final List<SendUpdated> sends;
  final List<AuxUpdated> auxes;
  final List<ChannelNamed> names;
  const Snapshot({
    required this.connected,
    required this.console,
    required this.sends,
    required this.auxes,
    required this.names,
  });
}

/// UI-side facade for the background OSC service. Hides the IPC plumbing
/// behind a typed event stream and command methods.
class OscBridge {
  final FlutterBackgroundService _svc = FlutterBackgroundService();
  final StreamController<BridgeEvent> _events =
      StreamController<BridgeEvent>.broadcast();
  StreamSubscription? _stateSub;
  StreamSubscription? _snapSub;
  StreamSubscription? _statusSub;

  Stream<BridgeEvent> get events => _events.stream;

  /// Wire IPC streams from the background service into the typed event
  /// stream. Call once at startup (after [configureBackgroundService]).
  void attach() {
    _stateSub ??= _svc.on(kEvtStateUpdate).listen(_onStateUpdate);
    _snapSub ??= _svc.on(kEvtSnapshot).listen(_onSnapshot);
    _statusSub ??= _svc.on(kEvtStatus).listen(_onStatus);
  }

  Future<bool> isRunning() async => _svc.isRunning();

  /// Persist credentials and start the background isolate. The service's
  /// onStart will read the same credentials, bind the socket, and begin
  /// the heartbeat.
  Future<bool> startWith(Credentials c) async {
    await CredentialsStore.save(c);
    return _svc.startService();
  }

  /// Tear down the background service. The bridge stays alive — call
  /// [dispose] on app shutdown to release stream subscriptions too.
  void shutdown() => _svc.invoke(kCmdShutdown);

  void requestSnapshot() => _svc.invoke(kCmdRequestSnapshot);

  void setSendLevel(int input, int aux, double value) {
    _svc.invoke(kCmdSetSendLevel, {
      'input': input,
      'aux': aux,
      'value': value,
    });
  }

  void setSendPan(int input, int aux, double value) {
    _svc.invoke(kCmdSetSendPan, {
      'input': input,
      'aux': aux,
      'value': value,
    });
  }

  void setSendOn(int input, int aux, bool value) {
    _svc.invoke(kCmdSetSendOn, {
      'input': input,
      'aux': aux,
      'value': value,
    });
  }

  void setAuxFader(int aux, double value) {
    _svc.invoke(kCmdSetAuxFader, {'aux': aux, 'value': value});
  }

  void setAuxMute(int aux, bool value) {
    _svc.invoke(kCmdSetAuxMute, {'aux': aux, 'value': value});
  }

  Future<void> dispose() async {
    await _stateSub?.cancel();
    await _snapSub?.cancel();
    await _statusSub?.cancel();
    await _events.close();
  }

  // ── decoders ──

  void _onStateUpdate(Map<String, dynamic>? data) {
    if (data == null) return;
    final type = data['type'];
    switch (type) {
      case 'send':
        _events.add(SendUpdated(
          data['input'] as int,
          data['aux'] as int,
          (data['level'] as num).toDouble(),
          (data['pan'] as num).toDouble(),
          data['on'] as bool,
        ));
      case 'aux':
        _events.add(AuxUpdated(
          data['aux'] as int,
          (data['fader'] as num).toDouble(),
          data['mute'] as bool,
        ));
      case 'name':
        _events.add(ChannelNamed(
          data['kind'] as String,
          data['ch'] as int,
          data['name'] as String,
        ));
    }
  }

  void _onSnapshot(Map<String, dynamic>? data) {
    if (data == null) return;
    final sends = <SendUpdated>[];
    final auxes = <AuxUpdated>[];
    final names = <ChannelNamed>[];
    for (final raw in (data['sends'] as List?) ?? const []) {
      final m = raw as Map;
      sends.add(SendUpdated(
        m['input'] as int,
        m['aux'] as int,
        (m['level'] as num).toDouble(),
        (m['pan'] as num).toDouble(),
        m['on'] as bool,
      ));
      final inputName = m['name'] as String? ?? '';
      if (inputName.isNotEmpty) {
        names.add(ChannelNamed('input', m['input'] as int, inputName));
      }
    }
    for (final raw in (data['auxes'] as List?) ?? const []) {
      final m = raw as Map;
      auxes.add(AuxUpdated(
        m['aux'] as int,
        (m['fader'] as num).toDouble(),
        m['mute'] as bool,
      ));
      final auxName = m['name'] as String? ?? '';
      if (auxName.isNotEmpty) {
        names.add(ChannelNamed('aux', m['aux'] as int, auxName));
      }
    }
    _events.add(Snapshot(
      connected: data['connected'] as bool? ?? false,
      console: data['console'] as String? ?? '',
      sends: sends,
      auxes: auxes,
      names: names,
    ));
  }

  void _onStatus(Map<String, dynamic>? data) {
    if (data == null) return;
    _events.add(StatusChanged(
      data['connected'] as bool? ?? false,
      data['console'] as String? ?? '',
    ));
  }
}
