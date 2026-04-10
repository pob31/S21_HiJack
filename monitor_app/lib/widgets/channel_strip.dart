import 'package:flutter/material.dart';
import '../models/send_state.dart';
import 'fader_widget.dart';
import 'pan_slider.dart';

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
      width: 130,
      child: Column(
        children: [
          Expanded(
            child: VerticalFader(
              value: send.level,
              dbMin: -60.0,
              dbMax: 10.0,
              label: send.inputLabel,
              active: send.on,
              onChanged: onLevelChanged,
            ),
          ),
          const SizedBox(height: 12),
          // Pan slider (-1.0 L .. 0.0 C .. +1.0 R)
          Text(
            _formatPan(send.pan),
            style: TextStyle(
              color: send.on ? Colors.white54 : Colors.white24,
              fontSize: 9,
            ),
          ),
          const SizedBox(height: 4),
          CenterPanSlider(
            value: send.pan,
            onChanged: onPanChanged,
            width: 120,
            height: 40,
            active: send.on,
          ),
          const SizedBox(height: 12),
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
          const SizedBox(height: 24),
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
        color: const Color(0xFF1A1A1A),
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
              const SizedBox(width: 8),
              Expanded(
                child: CenterPanSlider(
                  value: send.pan,
                  onChanged: onPanChanged,
                  width: 200,
                  height: 36,
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
