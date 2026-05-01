import 'package:flutter/gestures.dart';
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

const double _kFaderTrackHeight = 16.0;
const double _kFaderThumbThickness = 4.0;
const double _kFaderThumbWidthFraction = 0.8;
const double _kLabelColumnWidth = 56.0;
const double _kSliderColumnWidth = 32.0;

/// A vertical fader slider for mixing levels (dB scale with log curve).
class VerticalFader extends StatelessWidget {
  final double value; // dB
  final double dbMin;
  final double dbMax;
  final String label;
  final bool active;
  final ValueChanged<double> onChanged;

  const VerticalFader({
    super.key,
    required this.value,
    this.dbMin = -60.0,
    this.dbMax = 10.0,
    required this.label,
    this.active = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          children: [
            const SizedBox(width: _kLabelColumnWidth),
            SizedBox(
              width: _kSliderColumnWidth,
              child: FittedBox(
                fit: BoxFit.scaleDown,
                child: Text(
                  formatDb(value, dbMin),
                  style: TextStyle(
                    color: active ? Colors.white70 : Colors.white30,
                    fontSize: 12,
                  ),
                ),
              ),
            ),
            const Spacer(),
          ],
        ),
        const SizedBox(height: 4),
        Expanded(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SizedBox(
                width: _kLabelColumnWidth,
                child: RotatedBox(
                  quarterTurns: -1,
                  child: FittedBox(
                    fit: BoxFit.contain,
                    alignment: Alignment.centerRight,
                    child: Text(
                      label,
                      maxLines: 1,
                      style: TextStyle(
                        color: active ? Colors.white : Colors.white38,
                        fontSize: 48,
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                  ),
                ),
              ),
              SizedBox(
                width: _kSliderColumnWidth,
                child: LayoutBuilder(
                  builder: (context, constraints) {
                    return RawGestureDetector(
                      behavior: HitTestBehavior.opaque,
                      gestures: <Type, GestureRecognizerFactory>{
                        _InstantVerticalDragRecognizer:
                            GestureRecognizerFactoryWithHandlers<
                                _InstantVerticalDragRecognizer>(
                          () => _InstantVerticalDragRecognizer(),
                          (instance) {
                            instance.onUpdate = (details) {
                              final faderHeight = constraints.maxHeight;
                              if (faderHeight <= 0) return;
                              final positionDelta =
                                  -details.delta.dy / faderHeight;
                              if (positionDelta == 0) return;
                              final currentPos =
                                  dbToPosition(value, dbMin, dbMax);
                              final newPos =
                                  (currentPos + positionDelta).clamp(0.0, 1.0);
                              onChanged(positionToDb(newPos, dbMin, dbMax));
                            };
                          },
                        ),
                      },
                      child: IgnorePointer(
                        child: RotatedBox(
                          quarterTurns: -1,
                          child: SliderTheme(
                            data: SliderThemeData(
                              trackHeight: _kFaderTrackHeight,
                              thumbShape: const _LineThumbShape(
                                widthFraction: _kFaderThumbWidthFraction,
                                thickness: _kFaderThumbThickness,
                              ),
                              overlayShape: SliderComponentShape.noOverlay,
                              activeTrackColor:
                                  active ? Colors.blueAccent : Colors.grey[700]!,
                              inactiveTrackColor: Colors.white12,
                              thumbColor: active ? Colors.white : Colors.grey[600]!,
                              overlayColor: (active ? Colors.blueAccent : Colors.grey)
                                  .withAlpha(40),
                            ),
                            child: Theme(
                              data: Theme.of(context).copyWith(
                                materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                              ),
                              child: Slider(
                                value: dbToPosition(value, dbMin, dbMax),
                                min: 0.0,
                                max: 1.0,
                                onChanged: (_) {},
                              ),
                            ),
                          ),
                        ),
                      ),
                    );
                  },
                ),
              ),
              const Spacer(),
            ],
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
  final bool active;
  final ValueChanged<double> onChanged;

  const HorizontalFader({
    super.key,
    required this.value,
    this.dbMin = -60.0,
    this.dbMax = 10.0,
    this.active = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return SliderTheme(
      data: SliderThemeData(
        trackHeight: _kFaderTrackHeight,
        thumbShape: const _LineThumbShape(
          widthFraction: _kFaderThumbWidthFraction,
          thickness: _kFaderThumbThickness,
        ),
        overlayShape: SliderComponentShape.noOverlay,
        activeTrackColor: active ? Colors.blueAccent : Colors.grey[700]!,
        inactiveTrackColor: Colors.white12,
        thumbColor: active ? Colors.white : Colors.grey[600]!,
        overlayColor: (active ? Colors.blueAccent : Colors.grey).withAlpha(40),
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

/// Vertical-drag recognizer that wins the gesture arena on the very first
/// pointer move, with no touch-slop. Lets the fader respond from the first
/// pixel of finger movement and prevents a parent horizontal Scrollable
/// from stealing the gesture mid-drag.
class _InstantVerticalDragRecognizer extends VerticalDragGestureRecognizer {
  @override
  bool hasSufficientGlobalDistanceToAccept(
    PointerDeviceKind pointerDeviceKind,
    double? deviceTouchSlop,
  ) =>
      true;
}

/// Thumb that draws a thick line spanning [widthFraction] of the track's
/// perpendicular extent, replacing the default round thumb.
class _LineThumbShape extends SliderComponentShape {
  final double widthFraction;
  final double thickness;
  const _LineThumbShape({required this.widthFraction, required this.thickness});

  @override
  Size getPreferredSize(bool isEnabled, bool isDiscrete) =>
      Size(thickness * 4, thickness * 4);

  @override
  void paint(
    PaintingContext context,
    Offset center, {
    required Animation<double> activationAnimation,
    required Animation<double> enableAnimation,
    required bool isDiscrete,
    required TextPainter labelPainter,
    required RenderBox parentBox,
    required SliderThemeData sliderTheme,
    required TextDirection textDirection,
    required double value,
    required double textScaleFactor,
    required Size sizeWithOverflow,
  }) {
    final trackHeight = sliderTheme.trackHeight ?? _kFaderTrackHeight;
    final lineLen = trackHeight * widthFraction;
    final rect = Rect.fromCenter(
      center: center,
      width: thickness,
      height: lineLen,
    );
    final paint = Paint()
      ..color = sliderTheme.thumbColor ?? Colors.white
      ..style = PaintingStyle.fill;
    context.canvas.drawRRect(
      RRect.fromRectAndRadius(rect, const Radius.circular(1.5)),
      paint,
    );
  }
}
