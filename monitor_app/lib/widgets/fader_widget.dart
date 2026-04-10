import 'package:flutter/material.dart';

/// A vertical fader slider for mixing levels.
class VerticalFader extends StatelessWidget {
  final double value; // dB, typically -150 to +10
  final double min;
  final double max;
  final String label;
  final ValueChanged<double> onChanged;

  const VerticalFader({
    super.key,
    required this.value,
    this.min = -150.0,
    this.max = 10.0,
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
                value: value.clamp(min, max),
                min: min,
                max: max,
                onChanged: onChanged,
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

  String _formatDb(double db) {
    if (db <= -140) return '-inf';
    return '${db.toStringAsFixed(1)} dB';
  }
}

/// A horizontal fader for phone layout.
class HorizontalFader extends StatelessWidget {
  final double value;
  final double min;
  final double max;
  final ValueChanged<double> onChanged;

  const HorizontalFader({
    super.key,
    required this.value,
    this.min = -150.0,
    this.max = 10.0,
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
        value: value.clamp(min, max),
        min: min,
        max: max,
        onChanged: onChanged,
      ),
    );
  }
}
