import 'dart:math';
import 'package:flutter/material.dart';

// ── dB ↔ slider position conversion ──
// Audio faders use a pseudo-logarithmic curve:
// - Slider position 0.0..1.0 maps to dB range -inf..+10
// - The curve gives more resolution near 0 dB and less at the extremes.
// We use: position = (dB + 60) / 70 for dB >= -60, with a compressed
// region below -60 dB.

const double _dbMin = -90.0;
const double _dbMax = 10.0;

double _dbToPosition(double db) {
  if (db <= _dbMin) return 0.0;
  if (db >= _dbMax) return 1.0;
  // Attempt a log-like curve: more space near 0dB
  // Map -90..+10 using a power curve (exponent < 1 = more space at top)
  final normalized = (db - _dbMin) / (_dbMax - _dbMin); // 0..1 linear
  return pow(normalized, 0.5).toDouble(); // sqrt gives more top resolution
}

double _positionToDb(double pos) {
  if (pos <= 0.0) return _dbMin;
  if (pos >= 1.0) return _dbMax;
  // Inverse of sqrt: square
  final normalized = pos * pos;
  return _dbMin + normalized * (_dbMax - _dbMin);
}

String _formatDb(double db) {
  if (db <= -89) return '-inf';
  return '${db.toStringAsFixed(1)}';
}

/// A vertical fader slider for mixing levels (dB scale with log curve).
class VerticalFader extends StatelessWidget {
  final double value; // dB
  final String label;
  final ValueChanged<double> onChanged;

  const VerticalFader({
    super.key,
    required this.value,
    required this.label,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(
          label,
          style: const TextStyle(
            color: Colors.white70,
            fontSize: 10,
            fontWeight: FontWeight.w500,
          ),
          overflow: TextOverflow.ellipsis,
        ),
        const SizedBox(height: 4),
        Expanded(
          child: RotatedBox(
            quarterTurns: -1,
            child: SliderTheme(
              data: SliderThemeData(
                trackHeight: 4,
                thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 8),
                activeTrackColor: Colors.blueAccent,
                inactiveTrackColor: Colors.white12,
                thumbColor: Colors.white,
                overlayColor: Colors.blueAccent.withAlpha(40),
              ),
              child: Slider(
                value: _dbToPosition(value),
                min: 0.0,
                max: 1.0,
                onChanged: (pos) => onChanged(_positionToDb(pos)),
              ),
            ),
          ),
        ),
        Text(
          _formatDb(value),
          style: const TextStyle(color: Colors.white54, fontSize: 9),
        ),
      ],
    );
  }
}

/// A horizontal fader for phone layout (dB scale with log curve).
class HorizontalFader extends StatelessWidget {
  final double value; // dB
  final ValueChanged<double> onChanged;

  const HorizontalFader({
    super.key,
    required this.value,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return SliderTheme(
      data: SliderThemeData(
        trackHeight: 4,
        thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 8),
        activeTrackColor: Colors.blueAccent,
        inactiveTrackColor: Colors.white12,
        thumbColor: Colors.white,
        overlayColor: Colors.blueAccent.withAlpha(40),
      ),
      child: Slider(
        value: _dbToPosition(value),
        min: 0.0,
        max: 1.0,
        onChanged: (pos) => onChanged(_positionToDb(pos)),
      ),
    );
  }
}
