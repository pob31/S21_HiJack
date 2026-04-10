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

  static String _formatPan(double pan) {
    if (pan.abs() < 0.02) return 'C';
    if (pan < 0) return 'L${(-pan * 100).round()}';
    return 'R${(pan * 100).round()}';
  }

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 80,
      child: Column(
        children: [
          Expanded(
            child: VerticalFader(
              value: send.level,
              dbMin: -60.0,
              dbMax: 10.0,
              label: send.inputLabel,
              onChanged: onLevelChanged,
            ),
          ),
          const SizedBox(height: 6),
          // Pan slider (-1.0 L .. 0.0 C .. +1.0 R)
          Text(
            _formatPan(send.pan),
            style: const TextStyle(color: Colors.white54, fontSize: 9),
          ),
          SizedBox(
            width: 76,
            height: 36,
            child: SliderTheme(
              data: SliderThemeData(
                trackHeight: 4,
                thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 9),
                activeTrackColor: Colors.orangeAccent,
                inactiveTrackColor: Colors.white12,
                thumbColor: Colors.orange,
                overlayColor: Colors.orangeAccent.withAlpha(40),
              ),
              child: Slider(
                value: send.pan.clamp(-1.0, 1.0),
                min: -1.0,
                max: 1.0,
                onChanged: onPanChanged,
              ),
            ),
          ),
          const SizedBox(height: 6),
          // On/Off toggle
          GestureDetector(
            onTap: onToggle,
            child: Container(
              width: 48,
              height: 28,
              decoration: BoxDecoration(
                color: send.on ? Colors.green : Colors.grey[800],
                borderRadius: BorderRadius.circular(6),
              ),
              child: Center(
                child: Text(
                  send.on ? 'ON' : 'OFF',
                  style: const TextStyle(
                    color: Colors.white,
                    fontSize: 11,
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
  static String _formatPan(double pan) {
    if (pan.abs() < 0.02) return 'C';
    if (pan < 0) return 'L${(-pan * 100).round()}';
    return 'R${(pan * 100).round()}';
  }

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
              Text(
                'Pan ${_formatPan(send.pan)}',
                style: const TextStyle(color: Colors.white38, fontSize: 11),
              ),
              Expanded(
                child: SizedBox(
                  height: 32,
                  child: SliderTheme(
                    data: SliderThemeData(
                      trackHeight: 3,
                      thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 7),
                      activeTrackColor: Colors.orangeAccent,
                      inactiveTrackColor: Colors.white12,
                      thumbColor: Colors.orange,
                      overlayColor: Colors.orangeAccent.withAlpha(30),
                    ),
                    child: Slider(
                      value: send.pan.clamp(-1.0, 1.0),
                      min: -1.0,
                      max: 1.0,
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
