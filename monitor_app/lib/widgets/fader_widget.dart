import 'dart:math';
import 'package:flutter/material.dart';

// ── dB ↔ slider position conversion ──
// Audio faders use a pseudo-logarithmic curve:
// - Slider position 0.0..1.0 maps to dB range -inf..+10
// - The curve gives more resolution near 0 dB and less at the extremes.
// We use: position = (dB + 60) / 70 for dB >= -60, with a compressed
// region below -60 dB.

// ── dB ↔ slider position with configurable range ──

// Console fader taper: log-like curve that gives most resolution
// in the -20..+10 dB range where mixing actually happens.
// Uses a piecewise approach:
//   - Bottom 30% of travel → dbMin..-20 dB (compressed, rarely used)
//   - Top 70% of travel → -20..+10 dB (expanded, main working range)

double dbToPosition(double db, double dbMin, double dbMax) {
  if (db <= dbMin) return 0.0;
  if (db >= dbMax) return 1.0;

  const double knee = -20.0;    // where the curve transitions
  const double kneePos = 0.30;  // 30% of travel for everything below -20

  if (db <= knee) {
    // Bottom region: dbMin..-20 → 0..0.30
    final range = knee - dbMin;
    if (range <= 0) return 0.0;
    return kneePos * (db - dbMin) / range;
  } else {
    // Top region: -20..dbMax → 0.30..1.0
    final range = dbMax - knee;
    if (range <= 0) return 1.0;
    return kneePos + (1.0 - kneePos) * (db - knee) / range;
  }
}

double positionToDb(double pos, double dbMin, double dbMax) {
  if (pos <= 0.0) return dbMin;
  if (pos >= 1.0) return dbMax;

  const double knee = -20.0;
  const double kneePos = 0.30;

  if (pos <= kneePos) {
    // Bottom region: 0..0.30 → dbMin..-20
    final range = knee - dbMin;
    return dbMin + range * (pos / kneePos);
  } else {
    // Top region: 0.30..1.0 → -20..dbMax
    final range = dbMax - knee;
    return knee + range * ((pos - kneePos) / (1.0 - kneePos));
  }
}

String formatDb(double db, double dbMin) {
  if (db <= dbMin + 1) return '-inf';
  return '${db.toStringAsFixed(1)}';
}

/// A vertical fader slider for mixing levels (dB scale with log curve).
class VerticalFader extends StatelessWidget {
  final double value; // dB
  final double dbMin;
  final double dbMax;
  final String label;
  final ValueChanged<double> onChanged;

  const VerticalFader({
    super.key,
    required this.value,
    this.dbMin = -60.0,
    this.dbMax = 10.0,
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
        const SizedBox(height: 2),
        Text(
          formatDb(value, dbMin),
          style: const TextStyle(color: Colors.white54, fontSize: 12),
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
                value: dbToPosition(value, dbMin, dbMax),
                min: 0.0,
                max: 1.0,
                onChanged: (pos) => onChanged(positionToDb(pos, dbMin, dbMax)),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// A horizontal fader for phone layout (dB scale with log curve).
class HorizontalFader extends StatelessWidget {
  final double value; // dB
  final double dbMin;
  final double dbMax;
  final ValueChanged<double> onChanged;

  const HorizontalFader({
    super.key,
    required this.value,
    this.dbMin = -60.0,
    this.dbMax = 10.0,
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
        value: dbToPosition(value, dbMin, dbMax),
        min: 0.0,
        max: 1.0,
        onChanged: (pos) => onChanged(positionToDb(pos, dbMin, dbMax)),
      ),
    );
  }
}
