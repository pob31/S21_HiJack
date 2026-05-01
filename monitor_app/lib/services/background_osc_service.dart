import 'dart:async';
import 'dart:ui';

import 'package:flutter_background_service/flutter_background_service.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';

import 'credentials_store.dart';
import 'osc_service.dart';

// IPC event names exchanged between the UI isolate and this background
// isolate. Stable strings — change them only if you also bump the bridge.
const kEvtStateUpdate = 'stateUpdate';
const kEvtSnapshot = 'snapshot';
const kEvtStatus = 'status';
const kCmdRequestSnapshot = 'requestSnapshot';
const kCmdSetSendLevel = 'setSendLevel';
const kCmdSetSendPan = 'setSendPan';
const kCmdSetSendOn = 'setSendOn';
const kCmdSetAuxFader = 'setAuxFader';
const kCmdSetAuxMute = 'setAuxMute';
const kCmdShutdown = 'shutdown';

const _kAndroidNotificationChannelId = 's21_monitor_service';
const _kAndroidNotificationChannelName = 'S21 Monitor connection';
const _kAndroidNotificationId = 8521;

/// Configure the FlutterBackgroundService once on app startup.
/// Idempotent — safe to call from main() before runApp.
Future<void> configureBackgroundService() async {
  final svc = FlutterBackgroundService();

  // Android needs a notification channel registered before any
  // foreground notification can be posted.
  const androidChannel = AndroidNotificationChannel(
    _kAndroidNotificationChannelId,
    _kAndroidNotificationChannelName,
    description: 'Keeps the OSC connection to the daemon alive',
    importance: Importance.low,
  );
  final notifications = FlutterLocalNotificationsPlugin();
  await notifications
      .resolvePlatformSpecificImplementation<
          AndroidFlutterLocalNotificationsPlugin>()
      ?.createNotificationChannel(androidChannel);

  await svc.configure(
    androidConfiguration: AndroidConfiguration(
      onStart: backgroundServiceMain,
      autoStart: false,
      isForegroundMode: true,
      notificationChannelId: _kAndroidNotificationChannelId,
      initialNotificationTitle: 'S21 Monitor',
      initialNotificationContent: 'Connecting…',
      foregroundServiceNotificationId: _kAndroidNotificationId,
    ),
    iosConfiguration: IosConfiguration(
      onForeground: backgroundServiceMain,
      // No background work on iOS — App Store policy. Service runs only
      // while app is foregrounded; resume goes through fast-resume path.
      autoStart: false,
    ),
  );
}

/// Background isolate entry point. Runs in a dedicated Dart isolate; has
/// no access to the UI's Provider tree. Communicates with the UI via
/// `service.invoke(...)` and `service.on(...)` streams.
@pragma('vm:entry-point')
Future<void> backgroundServiceMain(ServiceInstance service) async {
  // Required when the entry runs in a fresh isolate.
  DartPluginRegistrant.ensureInitialized();

  final creds = await CredentialsStore.load();
  if (creds == null) {
    service.stopSelf();
    return;
  }

  final osc = OscService();
  await osc.bind();

  // ─── State snapshot held in this isolate ───
  final sends = <_SendKey, _SendSnap>{};
  final auxes = <int, _AuxSnap>{};
  final inputNames = <int, String>{};
  String consoleName = '';
  bool connected = false;
  DateTime lastReceived = DateTime.now();

  void pushStatus() {
    service.invoke(kEvtStatus, {
      'connected': connected,
      'console': consoleName,
    });
  }

  Map<String, dynamic> buildSnapshot() => {
        'console': consoleName,
        'connected': connected,
        'sends': sends.entries
            .map((e) => {
                  'input': e.key.input,
                  'aux': e.key.aux,
                  'level': e.value.level,
                  'pan': e.value.pan,
                  'on': e.value.on,
                  'name': inputNames[e.key.input] ?? '',
                })
            .toList(),
        'auxes': auxes.entries
            .map((e) => {
                  'aux': e.key,
                  'fader': e.value.fader,
                  'mute': e.value.mute,
                  'name': e.value.name,
                })
            .toList(),
      };

  // ─── Incoming OSC ───
  osc.incoming.listen((msg) {
    lastReceived = DateTime.now();
    if (!connected) {
      connected = true;
      pushStatus();
    }

    if (msg.address.startsWith('/monitor/state/send/')) {
      final parts = msg.address.split('/');
      if (parts.length >= 6 && msg.args.length >= 3) {
        final input = int.tryParse(parts[4]);
        final aux = int.tryParse(parts[5]);
        if (input == null || aux == null) return;
        final level = (msg.args[0] is OscFloat)
            ? (msg.args[0] as OscFloat).value
            : -150.0;
        final pan = (msg.args[1] is OscFloat)
            ? (msg.args[1] as OscFloat).value
            : 0.0;
        bool on = false;
        final onArg = msg.args[2];
        if (onArg is OscBool) {
          on = onArg.value;
        } else if (onArg is OscInt) {
          on = onArg.value != 0;
        } else if (onArg is OscFloat) {
          on = onArg.value != 0.0;
        }
        sends[_SendKey(input, aux)] = _SendSnap(level, pan, on);
        service.invoke(kEvtStateUpdate, {
          'type': 'send',
          'input': input,
          'aux': aux,
          'level': level,
          'pan': pan,
          'on': on,
        });
      }
    } else if (msg.address.startsWith('/monitor/state/aux/')) {
      final parts = msg.address.split('/');
      if (parts.length >= 5 && msg.args.length >= 2) {
        final aux = int.tryParse(parts[4]);
        if (aux == null) return;
        final fader = (msg.args[0] is OscFloat)
            ? (msg.args[0] as OscFloat).value
            : -150.0;
        bool mute = false;
        final muteArg = msg.args[1];
        if (muteArg is OscBool) {
          mute = muteArg.value;
        } else if (muteArg is OscInt) {
          mute = muteArg.value != 0;
        } else if (muteArg is OscFloat) {
          mute = muteArg.value != 0.0;
        }
        final existing = auxes[aux];
        auxes[aux] = _AuxSnap(fader, mute, existing?.name ?? '');
        service.invoke(kEvtStateUpdate, {
          'type': 'aux',
          'aux': aux,
          'fader': fader,
          'mute': mute,
        });
      }
    } else if (msg.address.startsWith('/monitor/state/name/')) {
      final parts = msg.address.split('/');
      if (parts.length >= 6 && msg.args.isNotEmpty) {
        final kind = parts[4];
        final ch = int.tryParse(parts[5]);
        final nameArg = msg.args[0];
        if (ch == null || nameArg is! OscString) return;
        final name = nameArg.value;
        if (kind == 'input') {
          inputNames[ch] = name;
          service.invoke(kEvtStateUpdate, {
            'type': 'name',
            'kind': 'input',
            'ch': ch,
            'name': name,
          });
        } else if (kind == 'aux') {
          final existing = auxes[ch];
          auxes[ch] = _AuxSnap(
            existing?.fader ?? -150.0,
            existing?.mute ?? false,
            name,
          );
          service.invoke(kEvtStateUpdate, {
            'type': 'name',
            'kind': 'aux',
            'ch': ch,
            'name': name,
          });
        }
      }
    } else if (msg.address == '/monitor/discovered' && msg.args.isNotEmpty) {
      final nameArg = msg.args[0];
      if (nameArg is OscString) {
        consoleName = nameArg.value;
        pushStatus();
      }
    }
  });

  // ─── Heartbeat + reconnection watchdog ───
  // The OscService heartbeat sends /monitor/{name}/connect every 10 s.
  // We additionally re-request the full state if no incoming message has
  // been seen for >15 s, mirroring the watchdog the UI used to run.
  osc.startHeartbeat(creds.host, creds.port, creds.name);
  osc.send(creds.host, creds.port, '/monitor/${creds.name}/state', []);

  final watchdog = Timer.periodic(const Duration(seconds: 5), (_) {
    final elapsed = DateTime.now().difference(lastReceived).inSeconds;
    if (elapsed > 15) {
      if (connected) {
        connected = false;
        pushStatus();
      }
      osc.send(creds.host, creds.port,
          '/monitor/${creds.name}/connect', []);
      osc.send(creds.host, creds.port,
          '/monitor/${creds.name}/state', []);
    }
  });

  // ─── UI commands ───
  service.on(kCmdRequestSnapshot).listen((_) {
    service.invoke(kEvtSnapshot, buildSnapshot());
  });

  service.on(kCmdSetSendLevel).listen((data) {
    if (data == null) return;
    final input = data['input'] as int;
    final aux = data['aux'] as int;
    final value = (data['value'] as num).toDouble();
    osc.send(creds.host, creds.port,
        '/monitor/${creds.name}/send/$input/$aux/level', [OscFloat(value)]);
  });
  service.on(kCmdSetSendPan).listen((data) {
    if (data == null) return;
    final input = data['input'] as int;
    final aux = data['aux'] as int;
    final value = (data['value'] as num).toDouble();
    osc.send(creds.host, creds.port,
        '/monitor/${creds.name}/send/$input/$aux/pan', [OscFloat(value)]);
  });
  service.on(kCmdSetSendOn).listen((data) {
    if (data == null) return;
    final input = data['input'] as int;
    final aux = data['aux'] as int;
    final value = data['value'] as bool;
    osc.send(creds.host, creds.port,
        '/monitor/${creds.name}/send/$input/$aux/on', [OscBool(value)]);
  });
  service.on(kCmdSetAuxFader).listen((data) {
    if (data == null) return;
    final aux = data['aux'] as int;
    final value = (data['value'] as num).toDouble();
    osc.send(creds.host, creds.port,
        '/monitor/${creds.name}/aux/$aux/fader', [OscFloat(value)]);
  });
  service.on(kCmdSetAuxMute).listen((data) {
    if (data == null) return;
    final aux = data['aux'] as int;
    final value = data['value'] as bool;
    osc.send(creds.host, creds.port,
        '/monitor/${creds.name}/aux/$aux/mute', [OscBool(value)]);
  });

  service.on(kCmdShutdown).listen((_) async {
    watchdog.cancel();
    osc.dispose();
    await service.stopSelf();
  });

  // Update the foreground notification once we've identified the console.
  if (service is AndroidServiceInstance) {
    Timer.periodic(const Duration(seconds: 5), (t) async {
      if (!await service.isForegroundService()) return;
      final label = consoleName.isNotEmpty ? consoleName : 'console';
      service.setForegroundNotificationInfo(
        title: 'S21 Monitor',
        content: connected
            ? 'Connected to $label as ${creds.name}'
            : 'Reconnecting to $label…',
      );
    });
  }
}

class _SendKey {
  final int input;
  final int aux;
  const _SendKey(this.input, this.aux);

  @override
  bool operator ==(Object other) =>
      other is _SendKey && other.input == input && other.aux == aux;

  @override
  int get hashCode => Object.hash(input, aux);
}

class _SendSnap {
  final double level;
  final double pan;
  final bool on;
  const _SendSnap(this.level, this.pan, this.on);
}

class _AuxSnap {
  final double fader;
  final bool mute;
  final String name;
  const _AuxSnap(this.fader, this.mute, this.name);
}
