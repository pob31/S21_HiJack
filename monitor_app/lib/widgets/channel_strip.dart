import 'package:flutter/material.dart';
import '../models/send_state.dart';
import 'fader_widget.dart';

/// A vertical channel strip (tablet layout): fader + pan + on/off.
class VerticalChannelStrip extends StatelessWidget {
  final SendState send;
  final ValueChanged<double> onLevelChanged;
  final ValueChanged<double> onPanChanged;
  final VoidCallback onToggle;

  const VerticalChannelStrip({
    super.key,
    required this.send,
    required this.onLevelChanged,
    required this.onPanChanged,
    required this.onToggle,
  });

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 56,
      child: Column(
        children: [
          Expanded(
            child: VerticalFader(
              value: send.level,
              label: send.inputLabel,
              onChanged: onLevelChanged,
            ),
          ),
          const SizedBox(height: 4),
          // Pan knob (simplified as small slider)
          SizedBox(
            width: 48,
            height: 24,
            child: SliderTheme(
              data: SliderThemeData(
                trackHeight: 2,
                thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 5),
                activeTrackColor: Colors.orangeAccent,
                inactiveTrackColor: Colors.white12,
                thumbColor: Colors.orange,
                overlayColor: Colors.orangeAccent.withAlpha(30),
              ),
              child: Slider(
                value: send.pan.clamp(-100.0, 100.0),
                min: -100.0,
                max: 100.0,
                onChanged: onPanChanged,
              ),
            ),
          ),
          const SizedBox(height: 4),
          // On/Off toggle
          GestureDetector(
            onTap: onToggle,
            child: Container(
              width: 36,
              height: 20,
              decoration: BoxDecoration(
                color: send.on ? Colors.green : Colors.grey[800],
                borderRadius: BorderRadius.circular(4),
              ),
              child: Center(
                child: Text(
                  send.on ? 'ON' : 'OFF',
                  style: const TextStyle(
                    color: Colors.white,
                    fontSize: 9,
                    fontWeight: FontWeight.bold,
                  ),
                ),
              ),
            ),
          ),
          const SizedBox(height: 4),
        ],
      ),
    );
  }
}

/// A horizontal channel strip (phone layout): label + fader + pan + on/off.
class HorizontalChannelStrip extends StatelessWidget {
  final SendState send;
  final ValueChanged<double> onLevelChanged;
  final ValueChanged<double> onPanChanged;
  final VoidCallback onToggle;

  const HorizontalChannelStrip({
    super.key,
    required this.send,
    required this.onLevelChanged,
    required this.onPanChanged,
    required this.onToggle,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      decoration: BoxDecoration(
        color: const Color(0xFF16213E),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  send.inputLabel,
                  style: const TextStyle(
                    color: Colors.white,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
              Text(
                _formatDb(send.level),
                style: const TextStyle(color: Colors.white54, fontSize: 12),
              ),
            ],
          ),
          const SizedBox(height: 4),
          HorizontalFader(
            value: send.level,
            onChanged: onLevelChanged,
          ),
          Row(
            children: [
              const Text('Pan', style: TextStyle(color: Colors.white38, fontSize: 11)),
              Expanded(
                child: SizedBox(
                  height: 24,
                  child: SliderTheme(
                    data: SliderThemeData(
                      trackHeight: 2,
                      thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 5),
                      activeTrackColor: Colors.orangeAccent,
                      inactiveTrackColor: Colors.white12,
                      thumbColor: Colors.orange,
                      overlayColor: Colors.orangeAccent.withAlpha(30),
                    ),
                    child: Slider(
                      value: send.pan.clamp(-100.0, 100.0),
                      min: -100.0,
                      max: 100.0,
                      onChanged: onPanChanged,
                    ),
                  ),
                ),
              ),
              GestureDetector(
                onTap: onToggle,
                child: Container(
                  width: 40,
                  height: 24,
                  decoration: BoxDecoration(
                    color: send.on ? Colors.green : Colors.grey[800],
                    borderRadius: BorderRadius.circular(4),
                  ),
                  child: Center(
                    child: Text(
                      send.on ? 'ON' : 'OFF',
                      style: const TextStyle(
                        color: Colors.white,
                        fontSize: 10,
                        fontWeight: FontWeight.bold,
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
  }

  String _formatDb(double db) {
    if (db <= -140) return '-inf';
    return '${db.toStringAsFixed(1)} dB';
  }
}
