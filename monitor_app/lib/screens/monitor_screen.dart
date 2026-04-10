import 'dart:async';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../models/monitor_client.dart';
import '../models/send_state.dart';
import '../services/osc_service.dart';
import '../widgets/channel_strip.dart';
import '../widgets/fader_widget.dart';
import 'connection_screen.dart';

class MonitorScreen extends StatefulWidget {
  final OscService osc;
  final bool requestStateOnMount;
  const MonitorScreen({super.key, required this.osc, this.requestStateOnMount = false});

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

    if (widget.requestStateOnMount) {
      // Delay slightly to ensure listener is active
      Future.delayed(const Duration(milliseconds: 100), () {
        if (!mounted) return;
        final model = context.read<MonitorClientModel>();
        final name = model.clientName;
        final host = model.daemonHost;
        final port = model.daemonPort;
        widget.osc.send(host, port, '/monitor/$name/state', []);
        debugPrint('S21 Monitor: state request sent after mount');
      });
    }
  }

  @override
  void dispose() {
    _sub.cancel();
    super.dispose();
  }

  void _handleIncoming(OscMessage msg) {
    final model = context.read<MonitorClientModel>();
    debugPrint('S21 Monitor: received ${msg.address} (${msg.args.length} args)');

    // Parse send state pushes: /monitor/state/send/{input}/{aux} [level, pan, on]
    // Single message with 3 args: Float(level), Float(pan), Int(on)
    if (msg.address.startsWith('/monitor/state/send/')) {
      final parts = msg.address.split('/');
      // parts: ['', 'monitor', 'state', 'send', '{input}', '{aux}']
      if (parts.length >= 6 && msg.args.length >= 3) {
        final input = int.tryParse(parts[4]);
        final aux = int.tryParse(parts[5]);
        if (input != null && aux != null) {
          final level = (msg.args[0] is OscFloat) ? (msg.args[0] as OscFloat).value : -150.0;
          final pan = (msg.args[1] is OscFloat) ? (msg.args[1] as OscFloat).value : 0.0;
          final on = (msg.args[2] is OscInt) ? (msg.args[2] as OscInt).value != 0 : false;
          model.updateSend(input, aux, level, pan, on);
          debugPrint('S21 Monitor: parsed send in=$input aux=$aux level=$level pan=$pan on=$on');

          // Auto-discover permitted auxes from incoming data
          if (!model.permittedAuxes.contains(aux)) {
            model.permittedAuxes.add(aux);
            model.permittedAuxes.sort();
            // Auto-select first aux if none selected
            model.selectedAux ??= aux;
            debugPrint('S21 Monitor: discovered aux $aux, selected=${model.selectedAux}');
          }
        }
      }
    }

    // Parse channel name pushes: /monitor/state/name/{input|aux}/{ch} [name]
    if (msg.address.startsWith('/monitor/state/name/')) {
      final parts = msg.address.split('/');
      // parts: ['', 'monitor', 'state', 'name', 'input'|'aux', '{ch}']
      if (parts.length >= 6 && msg.args.isNotEmpty) {
        final type = parts[4];
        final ch = int.tryParse(parts[5]);
        final nameArg = msg.args[0];
        if (ch != null && nameArg is OscString) {
          final name = nameArg.value;
          if (type == 'input') {
            // Update name on all sends for this input
            for (final entry in model.sends.entries) {
              if (entry.key.$1 == ch) {
                entry.value.name = name;
              }
            }
            model.notifyListeners();
          } else if (type == 'aux') {
            final state = model.auxStates.putIfAbsent(
              ch, () => AuxState(auxCh: ch),
            );
            state.name = name;
            model.notifyListeners();
          }
        }
      }
    }

    // Parse aux state pushes: /monitor/state/aux/{aux} [fader, mute]
    if (msg.address.startsWith('/monitor/state/aux/')) {
      final parts = msg.address.split('/');
      if (parts.length >= 5 && msg.args.length >= 2) {
        final aux = int.tryParse(parts[4]);
        if (aux != null) {
          final fader = (msg.args[0] is OscFloat) ? (msg.args[0] as OscFloat).value : -150.0;
          final mute = (msg.args[1] is OscInt) ? (msg.args[1] as OscInt).value != 0 : false;
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
            actions: [
              IconButton(
                icon: const Icon(Icons.logout, size: 20),
                tooltip: 'Disconnect',
                onPressed: () => _disconnect(model),
              ),
            ],
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
                              .map((a) {
                                final auxState = model.auxStates[a];
                                final auxName = auxState?.name ?? '';
                                final label = auxName.isNotEmpty ? '$auxName (Aux $a)' : 'Aux $a';
                                return DropdownMenuItem(
                                  value: a,
                                  child: Text(label),
                                );
                              })
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
        final auxState = model.auxStates[auxCh];
        final fader = auxState?.fader ?? -150.0;
        final mute = auxState?.mute ?? false;
        final auxName = auxState?.name ?? '';
        final displayName = auxName.isNotEmpty ? '$auxName (Aux $auxCh)' : 'Aux $auxCh';

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
                displayName,
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
                      dbMin: -80.0,
                      dbMax: 10.0,
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

  Future<void> _disconnect(MonitorClientModel model) async {
    await _sub.cancel();
    await widget.osc.reset();
    model.setDisconnected();
    model.sends.clear();
    model.auxStates.clear();
    model.selectedAux = null;
    model.permittedAuxes.clear();
    if (mounted) {
      Navigator.of(context).pushReplacement(
        MaterialPageRoute(
          builder: (_) => ConnectionScreen(osc: widget.osc),
        ),
      );
    }
  }

  String _formatDb(double db) {
    if (db <= -59) return '-inf';
    return '${db.toStringAsFixed(1)} dB';
  }
}
