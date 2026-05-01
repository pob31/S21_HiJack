import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models/monitor_client.dart';
import '../models/send_state.dart';
import '../services/credentials_store.dart';
import '../services/osc_bridge.dart';
import '../widgets/channel_strip.dart';
import '../widgets/fader_widget.dart';
import 'connection_screen.dart';

class MonitorScreen extends StatefulWidget {
  final OscBridge bridge;
  const MonitorScreen({super.key, required this.bridge});

  @override
  State<MonitorScreen> createState() => _MonitorScreenState();
}

class _MonitorScreenState extends State<MonitorScreen> {
  late StreamSubscription<BridgeEvent> _sub;
  int _tabIndex = 0; // 0 = My Mix, 1 = My Aux

  @override
  void initState() {
    super.initState();
    _sub = widget.bridge.events.listen(_onEvent);
    // Pull whatever the background service has cached so the UI repaints
    // immediately on this screen's first build (and on every resume).
    widget.bridge.requestSnapshot();
  }

  @override
  void dispose() {
    _sub.cancel();
    super.dispose();
  }

  void _onEvent(BridgeEvent event) {
    if (!mounted) return;
    final model = context.read<MonitorClientModel>();

    switch (event) {
      case StatusChanged(:final connected, :final console):
        if (console.isNotEmpty) {
          model.consoleName = console;
        }
        model.status = connected
            ? ConnectionStatus.connected
            : ConnectionStatus.connecting;
        model.notifyListeners();

      case Snapshot s:
        if (s.console.isNotEmpty) model.consoleName = s.console;
        model.status = s.connected
            ? ConnectionStatus.connected
            : ConnectionStatus.connecting;
        // Replay names first so subsequent send entries pick them up.
        for (final n in s.names) {
          _applyName(model, n);
        }
        for (final send in s.sends) {
          model.updateSend(send.input, send.aux, send.level, send.pan, send.on);
          if (!model.permittedAuxes.contains(send.aux)) {
            model.permittedAuxes.add(send.aux);
          }
        }
        for (final aux in s.auxes) {
          model.updateAux(aux.aux, aux.fader, aux.mute);
        }
        model.permittedAuxes.sort();
        model.selectedAux ??=
            model.permittedAuxes.isNotEmpty ? model.permittedAuxes.first : null;
        model.notifyListeners();

      case SendUpdated(
          :final input,
          :final aux,
          :final level,
          :final pan,
          :final on,
        ):
        model.updateSend(input, aux, level, pan, on);
        if (!model.permittedAuxes.contains(aux)) {
          model.permittedAuxes.add(aux);
          model.permittedAuxes.sort();
          model.selectedAux ??= aux;
          model.notifyListeners();
        }

      case AuxUpdated(:final aux, :final fader, :final mute):
        model.updateAux(aux, fader, mute);

      case ChannelNamed n:
        _applyName(model, n);
        model.notifyListeners();
    }
  }

  void _applyName(MonitorClientModel model, ChannelNamed n) {
    if (n.kind == 'input') {
      for (final entry in model.sends.entries) {
        if (entry.key.$1 == n.ch) {
          entry.value.name = n.name;
        }
      }
    } else if (n.kind == 'aux') {
      final state = model.auxStates.putIfAbsent(
        n.ch,
        () => AuxState(auxCh: n.ch),
      );
      state.name = n.name;
    }
  }

  @override
  Widget build(BuildContext context) {
    return Consumer<MonitorClientModel>(
      builder: (context, model, _) {
        return Scaffold(
          backgroundColor: const Color(0xFF0D0D0D),
          appBar: AppBar(
            backgroundColor: const Color(0xFF1A1A1A),
            title: Row(
              children: [
                Text(
                  model.consoleName.isNotEmpty
                      ? model.consoleName
                      : 'S21 Monitor',
                  style: const TextStyle(fontSize: 16),
                ),
                const SizedBox(width: 12),
                Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
                  decoration: BoxDecoration(
                    color: model.status == ConnectionStatus.connected
                        ? Colors.green
                        : Colors.red,
                    borderRadius: BorderRadius.circular(4),
                  ),
                  child: Text(
                    model.clientName,
                    style: const TextStyle(fontSize: 12, color: Colors.white),
                  ),
                ),
              ],
            ),
            actions: [
              IconButton(
                icon: const Icon(Icons.power_settings_new, size: 20),
                tooltip: 'Shut down service',
                onPressed: () => _shutdown(model),
              ),
            ],
          ),
          body: Column(
            children: [
              Container(
                color: const Color(0xFF1A1A1A),
                child: Row(
                  children: [
                    _tabButton('My Mix', 0),
                    _tabButton('My Aux', 1),
                    const Spacer(),
                    if (_tabIndex == 0 && model.availableAuxes.isNotEmpty)
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 8),
                        child: DropdownButton<int>(
                          value: model.selectedAux,
                          hint: const Text('Select Aux',
                              style: TextStyle(color: Colors.white54)),
                          dropdownColor: const Color(0xFF1A1A1A),
                          style: const TextStyle(color: Colors.white),
                          items: model.availableAuxes.map((a) {
                            final auxState = model.auxStates[a];
                            final auxName = auxState?.name ?? '';
                            final label = auxName.isNotEmpty
                                ? '$auxName (Aux $a)'
                                : 'Aux $a';
                            return DropdownMenuItem(
                              value: a,
                              child: Text(label),
                            );
                          }).toList(),
                          onChanged: (v) {
                            setState(() => model.selectedAux = v);
                          },
                        ),
                      ),
                  ],
                ),
              ),
              Expanded(
                child: _tabIndex == 0
                    ? _buildMyMix(context, model)
                    : _buildMyAux(context, model),
              ),
            ],
          ),
        );
      },
    );
  }

  Widget _tabButton(String label, int index) {
    final selected = _tabIndex == index;
    return GestureDetector(
      onTap: () => setState(() => _tabIndex = index),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 12),
        decoration: BoxDecoration(
          border: Border(
            bottom: BorderSide(
              color: selected ? Colors.blueAccent : Colors.transparent,
              width: 2,
            ),
          ),
        ),
        child: Text(
          label,
          style: TextStyle(
            color: selected ? Colors.white : Colors.white54,
            fontWeight: selected ? FontWeight.bold : FontWeight.normal,
          ),
        ),
      ),
    );
  }

  // ── My Mix tab ──

  Widget _buildMyMix(BuildContext context, MonitorClientModel model) {
    if (model.selectedAux == null) {
      return const Center(
        child: Text(
          'Select an aux to start mixing',
          style: TextStyle(color: Colors.white54),
        ),
      );
    }

    final sends = model.sendsForSelectedAux();
    if (sends.isEmpty) {
      return const Center(
        child: Text(
          'Waiting for state from daemon...',
          style: TextStyle(color: Colors.white54),
        ),
      );
    }

    final isWide = MediaQuery.of(context).size.width >= 600;

    if (isWide) {
      return SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        padding:
            const EdgeInsets.only(left: 8, right: 8, top: 8, bottom: 40),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: sends.map((send) {
            return Padding(
              padding: const EdgeInsets.symmetric(horizontal: 2),
              child: SizedBox(
                height: double.infinity,
                child: VerticalChannelStrip(
                  send: send,
                  onLevelChanged: (v) => _sendLevel(model, send, v),
                  onPanChanged: (v) => _sendPan(model, send, v),
                  onToggle: () => _sendToggle(model, send),
                ),
              ),
            );
          }).toList(),
        ),
      );
    } else {
      return ListView.separated(
        padding: const EdgeInsets.all(8),
        itemCount: sends.length,
        separatorBuilder: (_, __) => const SizedBox(height: 4),
        itemBuilder: (_, i) {
          final send = sends[i];
          return HorizontalChannelStrip(
            send: send,
            onLevelChanged: (v) => _sendLevel(model, send, v),
            onPanChanged: (v) => _sendPan(model, send, v),
            onToggle: () => _sendToggle(model, send),
          );
        },
      );
    }
  }

  // ── My Aux tab ──

  Widget _buildMyAux(BuildContext context, MonitorClientModel model) {
    final auxes = model.availableAuxes;
    if (auxes.isEmpty) {
      return const Center(
        child: Text(
          'No aux channels assigned',
          style: TextStyle(color: Colors.white54),
        ),
      );
    }

    return ListView(
      padding: const EdgeInsets.all(16),
      children: auxes.map((auxCh) {
        final auxState = model.auxStates[auxCh];
        final fader = auxState?.fader ?? -150.0;
        final mute = auxState?.mute ?? false;
        final auxName = auxState?.name ?? '';
        final displayName =
            auxName.isNotEmpty ? '$auxName (Aux $auxCh)' : 'Aux $auxCh';

        return Container(
          margin: const EdgeInsets.only(bottom: 12),
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
            color: const Color(0xFF1A1A1A),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                displayName,
                style: TextStyle(
                  color: mute ? Colors.white30 : Colors.white,
                  fontSize: 16,
                  fontWeight: FontWeight.bold,
                ),
              ),
              const SizedBox(height: 8),
              Row(
                children: [
                  Expanded(
                    child: HorizontalFader(
                      value: fader,
                      dbMin: -80.0,
                      dbMax: 10.0,
                      active: !mute,
                      onChanged: (v) {
                        model.updateAux(auxCh, v, mute);
                        widget.bridge.setAuxFader(auxCh, v);
                      },
                    ),
                  ),
                  const SizedBox(width: 8),
                  Text(
                    _formatDb(fader),
                    style: TextStyle(
                        color: mute ? Colors.white24 : Colors.white54),
                  ),
                  const SizedBox(width: 12),
                  GestureDetector(
                    onTap: () {
                      final newMute = !mute;
                      model.updateAux(auxCh, fader, newMute);
                      widget.bridge.setAuxMute(auxCh, newMute);
                    },
                    child: Container(
                      width: 56,
                      height: 32,
                      decoration: BoxDecoration(
                        color: mute ? Colors.red : Colors.grey[800],
                        borderRadius: BorderRadius.circular(4),
                      ),
                      child: Center(
                        child: Text(
                          'MUTE',
                          style: TextStyle(
                            color: mute ? Colors.white : Colors.white54,
                            fontWeight: FontWeight.bold,
                            fontSize: 11,
                          ),
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ],
          ),
        );
      }).toList(),
    );
  }

  // ── Send helpers ──

  void _sendLevel(MonitorClientModel model, SendState send, double value) {
    send.level = value;
    model.notifyListeners();
    widget.bridge.setSendLevel(send.inputCh, send.auxCh, value);
  }

  void _sendPan(MonitorClientModel model, SendState send, double value) {
    send.pan = value;
    model.notifyListeners();
    widget.bridge.setSendPan(send.inputCh, send.auxCh, value);
  }

  void _sendToggle(MonitorClientModel model, SendState send) {
    send.on = !send.on;
    model.notifyListeners();
    widget.bridge.setSendOn(send.inputCh, send.auxCh, send.on);
  }

  /// Tear down the background service AND clear stored credentials.
  /// Matches the WFS DIY pattern: an explicit "I'm done" action that
  /// stops the OSC daemon connection and returns to the connection
  /// wizard. Subsequent app launches will require entering creds again.
  Future<void> _shutdown(MonitorClientModel model) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Shut down service?'),
        content: const Text(
          'This stops the background OSC connection. The app will return '
          'to the connection screen.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text('Shut down'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;

    widget.bridge.shutdown();
    await CredentialsStore.clear();

    if (!mounted) return;
    model.setDisconnected();
    model.sends.clear();
    model.auxStates.clear();
    model.selectedAux = null;
    model.permittedAuxes.clear();

    Navigator.of(context).pushReplacement(
      MaterialPageRoute(
        builder: (_) => ConnectionScreen(bridge: widget.bridge),
      ),
    );
    debugPrint('S21 Monitor: background service shut down');
  }

  String _formatDb(double db) {
    if (db <= -59) return '-inf';
    return '${db.toStringAsFixed(1)} dB';
  }
}
