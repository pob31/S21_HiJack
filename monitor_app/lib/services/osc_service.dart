import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

/// Minimal OSC encoder/decoder for the monitor protocol.
/// OSC messages: /address\0 (padded to 4) + ,types\0 (padded to 4) + args
class OscService {
  RawDatagramSocket? _socket;
  final StreamController<OscMessage> _incoming = StreamController.broadcast();
  Timer? _heartbeat;

  Stream<OscMessage> get incoming => _incoming.stream;
  bool get isBound => _socket != null;

  /// Bind a local UDP socket on an ephemeral port.
  Future<void> bind() async {
    _socket = await RawDatagramSocket.bind(InternetAddress.anyIPv4, 0);
    _socket!.listen((event) {
      if (event == RawSocketEvent.read) {
        final dg = _socket!.receive();
        if (dg != null) {
          final msg = _decode(dg.data);
          if (msg != null) _incoming.add(msg);
        }
      }
    });
  }

  /// Send an OSC message to a specific host:port.
  void send(String host, int port, String address, List<OscArg> args) {
    if (_socket == null) return;
    final data = _encode(address, args);
    _socket!.send(data, InternetAddress(host), port);
  }

  /// Send a broadcast OSC message.
  void broadcast(int port, String address, List<OscArg> args) {
    if (_socket == null) return;
    _socket!.broadcastEnabled = true;
    final data = _encode(address, args);
    _socket!.send(data, InternetAddress('255.255.255.255'), port);
  }

  /// Start periodic heartbeat.
  void startHeartbeat(String host, int port, String clientName,
      {Duration interval = const Duration(seconds: 10)}) {
    _heartbeat?.cancel();
    _heartbeat = Timer.periodic(interval, (_) {
      send(host, port, '/monitor/$clientName/connect', []);
    });
    // Send initial connect immediately
    send(host, port, '/monitor/$clientName/connect', []);
  }

  void stopHeartbeat() {
    _heartbeat?.cancel();
    _heartbeat = null;
  }

  void dispose() {
    stopHeartbeat();
    _socket?.close();
    _socket = null;
    _incoming.close();
  }

  // ── OSC Encoding ──

  Uint8List _encode(String address, List<OscArg> args) {
    final buf = BytesBuilder();

    // Address string (null-terminated, padded to 4 bytes)
    buf.add(_encodeString(address));

    // Type tag string: , + type chars + null + padding
    final types = StringBuffer(',');
    for (final arg in args) {
      switch (arg) {
        case OscFloat():
          types.write('f');
        case OscInt():
          types.write('i');
        case OscString():
          types.write('s');
      }
    }
    buf.add(_encodeString(types.toString()));

    // Arguments
    for (final arg in args) {
      switch (arg) {
        case OscFloat(:final value):
          final bd = ByteData(4)..setFloat32(0, value, Endian.big);
          buf.add(bd.buffer.asUint8List());
        case OscInt(:final value):
          final bd = ByteData(4)..setInt32(0, value, Endian.big);
          buf.add(bd.buffer.asUint8List());
        case OscString(:final value):
          buf.add(_encodeString(value));
      }
    }

    return buf.toBytes();
  }

  Uint8List _encodeString(String s) {
    final bytes = [...s.codeUnits, 0]; // null-terminated
    while (bytes.length % 4 != 0) {
      bytes.add(0); // pad to 4-byte boundary
    }
    return Uint8List.fromList(bytes);
  }

  // ── OSC Decoding ──

  OscMessage? _decode(Uint8List data) {
    try {
      int offset = 0;

      // Read address
      final (address, addrEnd) = _readString(data, offset);
      offset = addrEnd;

      // Read type tag
      final (typeTag, typeEnd) = _readString(data, offset);
      offset = typeEnd;

      if (!typeTag.startsWith(',')) return null;
      final types = typeTag.substring(1);

      // Read arguments
      final args = <OscArg>[];
      for (final t in types.codeUnits) {
        switch (String.fromCharCode(t)) {
          case 'f':
            final bd = ByteData.sublistView(data, offset, offset + 4);
            args.add(OscFloat(bd.getFloat32(0, Endian.big)));
            offset += 4;
          case 'i':
            final bd = ByteData.sublistView(data, offset, offset + 4);
            args.add(OscInt(bd.getInt32(0, Endian.big)));
            offset += 4;
          case 's':
            final (s, end) = _readString(data, offset);
            args.add(OscString(s));
            offset = end;
          default:
            break; // skip unknown types
        }
      }

      return OscMessage(address: address, args: args);
    } catch (_) {
      return null;
    }
  }

  (String, int) _readString(Uint8List data, int offset) {
    int end = offset;
    while (end < data.length && data[end] != 0) {
      end++;
    }
    final s = String.fromCharCodes(data, offset, end);
    end++; // skip null
    while (end % 4 != 0) end++; // skip padding
    return (s, end);
  }
}

// ── OSC Types ──

sealed class OscArg {}

class OscFloat extends OscArg {
  final double value;
  OscFloat(this.value);
}

class OscInt extends OscArg {
  final int value;
  OscInt(this.value);
}

class OscString extends OscArg {
  final String value;
  OscString(this.value);
}

/// A decoded OSC message.
class OscMessage {
  final String address;
  final List<OscArg> args;
  OscMessage({required this.address, required this.args});
}
