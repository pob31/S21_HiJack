/// State for one input channel's send to one aux.
class SendState {
  final int inputCh;
  final int auxCh;
  double level;
  double pan;
  bool on;
  String name; // input channel name from console

  SendState({
    required this.inputCh,
    required this.auxCh,
    this.level = -150.0,
    this.pan = 0.0,
    this.on = false,
    this.name = '',
  });

  String get inputLabel => name.isNotEmpty ? name : 'In $inputCh';
}

/// State for an aux output channel.
class AuxState {
  final int auxCh;
  double fader;
  bool mute;
  String name; // aux channel name from console

  AuxState({
    required this.auxCh,
    this.fader = -150.0,
    this.mute = false,
    this.name = '',
  });

  String get label => name.isNotEmpty ? name : 'Aux $auxCh';
}
