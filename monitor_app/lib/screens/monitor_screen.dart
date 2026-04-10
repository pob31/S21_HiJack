import 'dart:async';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../models/monitor_client.dart';
import '../services/osc_service.dart';
import '../widgets/channel_strip.dart';
import '../widgets/fader_widget.dart';

class MonitorScreen extends StatefulWidget {
  final OscService osc;
  const MonitorScreen({super.key, required this.osc});

  @override
  State<MonitorScreen> createState() => _MonitorScreenState();
}

class _MonitorScreenState extends State<MonitorScreen> {
  late StreamSubscription<OscMessage> _sub;
  int _tabIndex = 0; // 0 = My Mix, 1 = My Aux

  @override
  void initState() {
    super.initState();
    _sub = widget.osc.incoming.listen(_handleIncoming);
  }

  @override
  void dispose() {
    _sub.cancel();
    super.dispose();
  }

  void _handleIncoming(OscMessage msg) {
    final model = context.read<MonitorClientModel>();

    // Parse send state pushes: /monitor/send/{input}/{aux}/{level|pan|on}
    if (msg.address.startsWith('/monitor/send/')) {
      final parts = msg.address.split('/');
      // /monitor/send/{input}/{aux}/{param}
      if (parts.length >= 6) {
        final input = int.tryParse(parts[3]);
        final aux = int.tryParse(parts[4]);
        final param = parts[5];
        if (input != null && aux != null && msg.args.isNotEmpty) {
          final key = (input, aux);
          final current = model.sends[key];
          double level = current?.level ?? -150.0;
          double pan = current?.pan ?? 0.0;
          bool on = current?.on ?? false;

          final val = msg.args[0];
          switch (param) {
            case 'level':
              if (val is OscFloat) level = val.value;
            case 'pan':
              if (val is OscFloat) pan = val.value;
            case 'on':
              if (val is OscInt) on = val.value != 0;
              if (val is OscFloat) on = val.value != 0.0;
          }
          model.updateSend(input, aux, level, pan, on);
        }
      }
    }

    // Parse aux state pushes: /monitor/aux/{aux}/{fader|mute}
    if (msg.address.startsWith('/monitor/aux/')) {
      final parts = msg.address.split('/');
      if (parts.length >= 5) {
        final aux = int.tryParse(parts[3]);
        final param = parts[4];
        if (aux != null && msg.args.isNotEmpty) {
          final current = model.auxStates[aux];
          double fader = current?.fader ?? -150.0;
          bool mute = current?.mute ?? false;

          final val = msg.args[0];
          switch (param) {
            case 'fader':
              if (val is OscFloat) fader = val.value;
            case 'mute':
              if (val is OscInt) mute = val.value != 0;
              if (val is OscFloat) mute = val.value != 0.0;
          }
          model.updateAux(aux, fader, mute);
        }
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Consumer<MonitorClientModel>(
      builder: (context, model, _) {
        return Scaffold(
          backgroundColor: const Color(0xFF1A1A2E),
          appBar: AppBar(
            backgroundColor: const Color(0xFF16213E),
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
          ),
          body: Column(
            children: [
              // Tab bar
              Container(
                color: const Color(0xFF16213E),
                child: Row(
                  children: [
                    _tabButton('My Mix', 0),
                    _tabButton('My Aux', 1),
                    const Spacer(),
                    // Aux selector
                    if (_tabIndex == 0 && model.availableAuxes.isNotEmpty)
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 8),
                        child: DropdownButton<int>(
                          value: model.selectedAux,
                          hint: const Text('Select Aux',
                              style: TextStyle(color: Colors.white54)),
                          dropdownColor: const Color(0xFF16213E),
                          style: const TextStyle(color: Colors.white),
                          items: model.availableAuxes
                              .map((a) => DropdownMenuItem(
                                    value: a,
                                    child: Text('Aux $a'),
                                  ))
                              .toList(),
                          onChanged: (v) {
                            setState(() => model.selectedAux = v);
                          },
                        ),
                      ),
                  ],
                ),
              ),
              // Content
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
      // Tablet: horizontal scroll of vertical channel strips
      return SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.all(8),
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
      // Phone: vertical list of horizontal channel strips
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
        final state = model.auxStates[auxCh];
        final fader = state?.fader ?? -150.0;
        final mute = state?.mute ?? false;

        return Container(
          margin: const EdgeInsets.only(bottom: 12),
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
            color: const Color(0xFF16213E),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'Aux $auxCh',
                style: const TextStyle(
                  color: Colors.white,
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
                      onChanged: (v) {
                        model.updateAux(auxCh, v, mute);
                        widget.osc.send(
                          model.daemonHost,
                          model.daemonPort,
                          '/monitor/${model.clientName}/aux/$auxCh/fader',
                          [OscFloat(v)],
                        );
                      },
                    ),
                  ),
                  const SizedBox(width: 8),
                  Text(
                    _formatDb(fader),
                    style: const TextStyle(color: Colors.white54),
                  ),
                  const SizedBox(width: 12),
                  GestureDetector(
                    onTap: () {
                      final newMute = !mute;
                      model.updateAux(auxCh, fader, newMute);
                      widget.osc.send(
                        model.daemonHost,
                        model.daemonPort,
                        '/monitor/${model.clientName}/aux/$auxCh/mute',
                        [OscInt(newMute ? 1 : 0)],
                      );
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
                          mute ? 'MUTE' : 'MUTE',
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
    widget.osc.send(
      model.daemonHost,
      model.daemonPort,
      '/monitor/${model.clientName}/send/${send.inputCh}/${send.auxCh}/level',
      [OscFloat(value)],
    );
  }

  void _sendPan(MonitorClientModel model, SendState send, double value) {
    send.pan = value;
    model.notifyListeners();
    widget.osc.send(
      model.daemonHost,
      model.daemonPort,
      '/monitor/${model.clientName}/send/${send.inputCh}/${send.auxCh}/pan',
      [OscFloat(value)],
    );
  }

  void _sendToggle(MonitorClientModel model, SendState send) {
    send.on = !send.on;
    model.notifyListeners();
    widget.osc.send(
      model.daemonHost,
      model.daemonPort,
      '/monitor/${model.clientName}/send/${send.inputCh}/${send.auxCh}/on',
      [OscInt(send.on ? 1 : 0)],
    );
  }

  String _formatDb(double db) {
    if (db <= -140) return '-inf';
    return '${db.toStringAsFixed(1)} dB';
  }
}
