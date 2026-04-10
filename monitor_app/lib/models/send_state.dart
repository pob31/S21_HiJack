/// State for one input channel's send to one aux.
class SendState {
  final int inputCh;
  final int auxCh;
  double level;
  double pan;
  bool on;

  SendState({
    required this.inputCh,
    required this.auxCh,
    this.level = -150.0,
    this.pan = 0.0,
    this.on = false,
  });

  String get inputLabel => 'Input $inputCh';
}

/// State for an aux output channel.
class AuxState {
  final int auxCh;
  double fader;
  bool mute;

  AuxState({
    required this.auxCh,
    this.fader = -150.0,
    this.mute = false,
  });

  String get label => 'Aux $auxCh';
}
