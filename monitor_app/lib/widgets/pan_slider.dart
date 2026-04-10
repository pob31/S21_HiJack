import 'package:flutter/material.dart';

/// A bi-directional pan slider where the active track grows from the center.
/// Value range: -1.0 (full left) to +1.0 (full right), 0.0 = center.
class CenterPanSlider extends StatefulWidget {
  final double value;
  final ValueChanged<double> onChanged;
  final double width;
  final double height;

  const CenterPanSlider({
    super.key,
    required this.value,
    required this.onChanged,
    this.width = 86,
    this.height = 40,
  });

  @override
  State<CenterPanSlider> createState() => _CenterPanSliderState();
}

class _CenterPanSliderState extends State<CenterPanSlider> {
  bool _dragging = false;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onHorizontalDragStart: (_) => setState(() => _dragging = true),
      onHorizontalDragEnd: (_) => setState(() => _dragging = false),
      onHorizontalDragUpdate: (details) {
        final trackWidth = widget.width - 18; // thumb diameter
        final delta = details.delta.dx / trackWidth * 2.0;
        final newVal = (widget.value + delta).clamp(-1.0, 1.0);
        widget.onChanged(newVal);
      },
      onTapDown: (details) {
        final trackWidth = widget.width - 18;
        final trackStart = 9.0; // half thumb
        final pos = (details.localPosition.dx - trackStart) / trackWidth;
        final newVal = (pos * 2.0 - 1.0).clamp(-1.0, 1.0);
        widget.onChanged(newVal);
      },
      onDoubleTap: () {
        // Double-tap to center
        widget.onChanged(0.0);
      },
      child: CustomPaint(
        size: Size(widget.width, widget.height),
        painter: _PanPainter(
          value: widget.value.clamp(-1.0, 1.0),
          dragging: _dragging,
        ),
      ),
    );
  }
}

class _PanPainter extends CustomPainter {
  final double value;
  final bool dragging;

  _PanPainter({required this.value, required this.dragging});

  @override
  void paint(Canvas canvas, Size size) {
    final thumbRadius = 9.0;
    final trackHeight = 4.0;
    final trackY = size.height / 2;
    final trackLeft = thumbRadius;
    final trackRight = size.width - thumbRadius;
    final trackWidth = trackRight - trackLeft;
    final centerX = trackLeft + trackWidth / 2;

    // Inactive track (full width)
    final inactivePaint = Paint()
      ..color = const Color(0xFF333333)
      ..strokeWidth = trackHeight
      ..strokeCap = StrokeCap.round;
    canvas.drawLine(
      Offset(trackLeft, trackY),
      Offset(trackRight, trackY),
      inactivePaint,
    );

    // Center tick mark
    final tickPaint = Paint()
      ..color = const Color(0xFF555555)
      ..strokeWidth = 1.5;
    canvas.drawLine(
      Offset(centerX, trackY - 6),
      Offset(centerX, trackY + 6),
      tickPaint,
    );

    // Active track (from center to thumb)
    final thumbX = trackLeft + (value + 1.0) / 2.0 * trackWidth;
    final activePaint = Paint()
      ..color = Colors.grey
      ..strokeWidth = trackHeight
      ..strokeCap = StrokeCap.round;
    canvas.drawLine(
      Offset(centerX, trackY),
      Offset(thumbX, trackY),
      activePaint,
    );

    // Thumb
    final thumbPaint = Paint()
      ..color = dragging ? Colors.grey : Colors.white70;
    canvas.drawCircle(Offset(thumbX, trackY), thumbRadius, thumbPaint);

    // Thumb border
    final borderPaint = Paint()
      ..color = Colors.white24
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.0;
    canvas.drawCircle(Offset(thumbX, trackY), thumbRadius, borderPaint);
  }

  @override
  bool shouldRepaint(_PanPainter oldDelegate) =>
      value != oldDelegate.value || dragging != oldDelegate.dragging;
}
