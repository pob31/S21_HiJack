//! D700 probe: dump everything the surface sends, and fire test messages at it.
//! Throwaway diagnostic - not part of S21_HiJack.

use midir::{Ignore, MidiInput, MidiOutput};
use std::env;
use std::thread::sleep;
use std::time::Duration;

type R<T> = Result<T, Box<dyn std::error::Error>>;

const MATCH: &str = "d 700";

fn main() -> R<()> {
    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("list");
    match cmd {
        "list" => list(),
        "mon" => monitor(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(25)),
        "lcd" => lcd(args.get(2).map(String::as_str).unwrap_or("S21 HIJACK")),
        "sweep" => sweep(),
        "leds" => leds(),
        "lcdid" => lcdid(),
        "split" => split(),
        "idtest" => idtest(),
        "idchar" => idchar(),
        "master" => master(),
        "colors" => colors(),
        "colors2" => colors2(),
        "cverify" => cverify(),
        "mcolor" => mcolor(),
        "mcolor2" => mcolor2(),
        "ident" => ident(),
        "handshake" => handshake(),
        "rows" => rows(),
        "rows2" => rows2(),
        "cmd18" => cmd18(),
        "demo" => demo(),
        "oscdump" => oscdump(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(45)),
        "oscsend" => oscsend(&args[2..]),
        "oscprobe" => oscprobe(),
        "oscprobe2" => oscprobe2(),
        "rgbtest" => rgbtest(args.get(2).map(String::as_str).unwrap_or("/masterdial/rgb")),
        "rgbwhich" => rgbwhich(args.get(2).map(String::as_str).unwrap_or("/masterdial/rgb")),
        "fadertest" => fadertest(),
        "faderf" => faderf(),
        "show" => show(),
        _ => {
            eprintln!("usage: d700 [list | mon <secs> | lcd <text> | sweep | leds]");
            Ok(())
        }
    }
}

fn list() -> R<()> {
    let mi = MidiInput::new("probe")?;
    println!("INPUTS:");
    for (i, p) in mi.ports().iter().enumerate() {
        println!("  [{i}] {}", mi.port_name(p)?);
    }
    let mo = MidiOutput::new("probe")?;
    println!("OUTPUTS:");
    for (i, p) in mo.ports().iter().enumerate() {
        println!("  [{i}] {}", mo.port_name(p)?);
    }
    Ok(())
}

/// Open every D700 input and print decoded traffic for `secs`.
fn monitor(secs: u64) -> R<()> {
    let probe = MidiInput::new("probe")?;
    let names: Vec<String> = probe
        .ports()
        .iter()
        .filter_map(|p| probe.port_name(p).ok())
        .filter(|n| n.to_lowercase().contains(MATCH))
        .collect();
    if names.is_empty() {
        println!("no D700 input ports found");
        return Ok(());
    }
    let mut conns = Vec::new();
    for name in &names {
        let mut mi = MidiInput::new("probe")?;
        mi.ignore(Ignore::ActiveSense);
        let port = mi
            .ports()
            .into_iter()
            .find(|p| mi.port_name(p).map(|n| &n == name).unwrap_or(false))
            .ok_or("port vanished")?;
        let tag = name.clone();
        conns.push(mi.connect(
            &port,
            "probe-in",
            move |_ts, msg, _| {
                println!("{:<18} {:<26} {}", tag, hex(msg), decode(msg));
            },
            (),
        )?);
        println!("listening on: {name}");
    }
    println!("\n--- press faders, knobs, buttons for {secs}s ---");
    println!("{:<18} {:<26} MEANING", "PORT", "RAW");
    sleep(Duration::from_secs(secs));
    println!("--- done ---");
    Ok(())
}

fn hex(b: &[u8]) -> String {
    let s: Vec<String> = b.iter().take(12).map(|x| format!("{x:02X}")).collect();
    let mut out = s.join(" ");
    if b.len() > 12 {
        out.push_str(" ..");
    }
    out
}

/// Untruncated hex - hex() caps at 12 bytes, which hides SysEx payloads.
fn hex_full(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode(m: &[u8]) -> String {
    if m.is_empty() {
        return "empty".into();
    }
    if m[0] == 0xF0 {
        let ascii: String = m
            .iter()
            .map(|&c| {
                if (0x20..0x7F).contains(&c) {
                    c as char
                } else {
                    '.'
                }
            })
            .collect();
        return format!("SYSEX len={} \"{}\"", m.len(), ascii);
    }
    let ch = (m[0] & 0x0F) + 1;
    match m[0] & 0xF0 {
        0x90 if m.len() >= 3 => {
            let kind = if m[2] > 0 { "ON " } else { "OFF" };
            format!(
                "NOTE {kind} ch{ch} note=0x{:02X} ({}) vel={}",
                m[1],
                note_name(m[1]),
                m[2]
            )
        }
        0x80 if m.len() >= 3 => {
            format!("NOTE OFF ch{ch} note=0x{:02X} ({})", m[1], note_name(m[1]))
        }
        0xB0 if m.len() >= 3 => format!(
            "CC ch{ch} cc={} (0x{:02X}) val={} {}",
            m[1],
            m[1],
            m[2],
            cc_hint(m[1], m[2])
        ),
        0xE0 if m.len() >= 3 => {
            let v = ((m[2] as u16) << 7) | m[1] as u16;
            format!(
                "PITCHBEND ch{ch} val={v} ({:.1}%)  <- FADER {ch}",
                v as f32 / 163.83
            )
        }
        0xD0 if m.len() >= 2 => format!("CHANPRESSURE ch{ch} val={} <- meter", m[1]),
        _ => "?".into(),
    }
}

/// Full standard MCU note map, so a capture reads as control names.
fn note_name(n: u8) -> &'static str {
    match n {
        0x00..=0x07 => "Rec/Arm",
        0x08..=0x0F => "Solo",
        0x10..=0x17 => "Mute",
        0x18..=0x1F => "Select",
        0x20..=0x27 => "V-Pot press",
        0x28 => "Assign Track",
        0x29 => "Assign Send",
        0x2A => "Assign Pan",
        0x2B => "Assign Plug-In",
        0x2C => "Assign EQ",
        0x2D => "Assign Instrument",
        0x2E => "Bank Left",
        0x2F => "Bank Right",
        0x30 => "Channel Left",
        0x31 => "Channel Right",
        0x32 => "Flip",
        0x33 => "Global View",
        0x34 => "Name/Value",
        0x35 => "SMPTE/Beats",
        0x36..=0x3D => "F1-F8",
        0x3E..=0x45 => "Global view bank",
        0x46 => "Shift",
        0x47 => "Option",
        0x48 => "Control",
        0x49 => "Alt/Cmd",
        0x4A => "Read/Off",
        0x4B => "Write",
        0x4C => "Trim",
        0x4D => "Touch",
        0x4E => "Latch",
        0x4F => "Group",
        0x50 => "Save",
        0x51 => "Undo",
        0x52 => "Cancel",
        0x53 => "Enter",
        0x54 => "Marker",
        0x55 => "Nudge",
        0x56 => "Cycle",
        0x57 => "Drop",
        0x58 => "Replace",
        0x59 => "Click",
        0x5A => "Solo (global)",
        0x5B => "REWIND",
        0x5C => "FAST FWD",
        0x5D => "STOP",
        0x5E => "PLAY",
        0x5F => "RECORD",
        0x60 => "Cursor Up",
        0x61 => "Cursor Down",
        0x62 => "Cursor Left",
        0x63 => "Cursor Right",
        0x64 => "Zoom",
        0x65 => "Scrub",
        0x66 => "User Switch A",
        0x67 => "User Switch B",
        0x68..=0x6F => "FADER TOUCH",
        0x70 => "MASTER FADER TOUCH",
        _ => "?? UNMAPPED",
    }
}

fn cc_hint(cc: u8, val: u8) -> String {
    if cc == 0x3C {
        let ticks = if val < 64 {
            val as i32
        } else {
            -(val as i32 - 64)
        };
        return format!("<- JOG WHEEL rel={ticks:+}");
    }
    if (0x30..=0x37).contains(&cc) {
        return format!("<- ring echo {}", cc - 0x2F);
    }
    if (0x40..=0x4B).contains(&cc) {
        return "<- 7-segment display".to_string();
    }
    if (0x10..=0x17).contains(&cc) {
        let ticks = if val < 64 {
            val as i32
        } else {
            -(val as i32 - 64)
        };
        format!("<- ENCODER {} rel={ticks:+}", cc - 0x0F)
    } else {
        String::new()
    }
}

fn out_ports() -> R<Vec<String>> {
    let mo = MidiOutput::new("probe")?;
    Ok(mo
        .ports()
        .iter()
        .filter_map(|p| mo.port_name(p).ok())
        .filter(|n| n.to_lowercase().contains(MATCH))
        .collect())
}

fn open_out(name: &str) -> R<midir::MidiOutputConnection> {
    let mo = MidiOutput::new("probe")?;
    let port = mo
        .ports()
        .into_iter()
        .find(|p| mo.port_name(p).map(|n| n == name).unwrap_or(false))
        .ok_or("out port not found")?;
    Ok(mo.connect(&port, "probe-out")?)
}

/// MCU LCD write: F0 00 00 66 <id> 12 <offset> <text> F7.
/// Device id varies by vendor, so try the four common ones on every port.
fn lcd(text: &str) -> R<()> {
    let ids: [(u8, &str); 4] = [
        (0x10, "Logic Control"),
        (0x11, "Logic Control XT"),
        (0x14, "Mackie Control"),
        (0x15, "Mackie Control XT"),
    ];
    for name in out_ports()? {
        let mut conn = open_out(&name)?;
        for (id, label) in ids {
            let mut msg = vec![0xF0, 0x00, 0x00, 0x66, id, 0x12, 0x00];
            msg.extend(text.as_bytes());
            msg.push(0xF7);
            println!(
                "{name}: id 0x{id:02X} ({label}) -> \"{text}\"   [{}]",
                hex(&msg)
            );
            conn.send(&msg)?;
            sleep(Duration::from_millis(1200));
        }
    }
    println!("\nWhich port + id made text appear? That is the display protocol.");
    Ok(())
}

/// Drive the 8 motor faders through a stroke on every D700 output.
fn sweep() -> R<()> {
    for name in out_ports()? {
        let mut conn = open_out(&name)?;
        println!("sweeping faders on {name}");
        for pos in [0u16, 4096, 8192, 12288, 16383, 8192, 0] {
            for ch in 0..8u8 {
                conn.send(&[0xE0 | ch, (pos & 0x7F) as u8, (pos >> 7) as u8])?;
            }
            sleep(Duration::from_millis(450));
        }
    }
    Ok(())
}

/// Light button LEDs (note-on vel 127) and encoder rings (CC 0x30..0x37).
/// MCU ring value = (mode << 4) | position, position 1..11, bit 6 = centre LED.
/// Modes: 0 single dot, 1 boost/cut from centre, 2 wrap from left, 3 spread.
fn leds() -> R<()> {
    for name in out_ports()? {
        let mut conn = open_out(&name)?;

        println!("[{name}] button LEDs: Rec, Solo, Mute, Select");
        for (base, label) in [
            (0x00u8, "Rec"),
            (0x08, "Solo"),
            (0x10, "Mute"),
            (0x18, "Select"),
        ] {
            println!("   lighting {label} 1-8");
            for i in 0..8u8 {
                conn.send(&[0x90, base + i, 127])?;
            }
            sleep(Duration::from_millis(900));
            for i in 0..8u8 {
                conn.send(&[0x90, base + i, 0])?;
            }
        }

        println!("[{name}] encoder rings: 4 modes x sweep");
        for (mode, label) in [
            (0u8, "single dot"),
            (1, "boost/cut"),
            (2, "wrap"),
            (3, "spread"),
        ] {
            println!("   ring mode {mode} ({label})");
            for pos in 1..=11u8 {
                for i in 0..8u8 {
                    conn.send(&[0xB0, 0x30 + i, (mode << 4) | pos])?;
                }
                sleep(Duration::from_millis(120));
            }
        }
        for i in 0..8u8 {
            conn.send(&[0xB0, 0x30 + i, 0])?;
        }
    }
    Ok(())
}

/// Distinct label per (port, device id) at a distinct strip offset.
fn lcdid() -> R<()> {
    let ids: [u8; 4] = [0x10, 0x11, 0x14, 0x15];
    let mut slot = 0usize;
    for (pi, name) in out_ports()?.iter().enumerate() {
        let mut conn = open_out(name)?;
        for id in ids {
            let offset = (slot * 7) as u8;
            let label = format!("P{}x{:02X}", pi + 1, id);
            let mut msg = vec![0xF0, 0x00, 0x00, 0x66, id, 0x12, offset];
            msg.extend(label.as_bytes());
            msg.push(0xF7);
            conn.send(&msg)?;
            println!(
                "strip {} (offset 0x{offset:02X}) <- \"{label}\"  via {name}",
                slot + 1
            );
            slot += 1;
            sleep(Duration::from_millis(250));
        }
    }
    Ok(())
}

/// Distinct text per output port: different = separate banks, same = mirrored.
fn split() -> R<()> {
    for (pi, name) in out_ports()?.iter().enumerate() {
        let mut conn = open_out(name)?;
        let label = format!("PORT-{}", pi + 1);
        for id in [0x10u8, 0x11, 0x14, 0x15] {
            for strip in 0..8u8 {
                let mut msg = vec![0xF0, 0x00, 0x00, 0x66, id, 0x12, strip * 7];
                msg.extend(label.as_bytes());
                msg.push(0xF7);
                conn.send(&msg)?;
            }
        }
        println!("wrote \"{label}\" to all 8 strips via {name}");
    }
    Ok(())
}

/// Settle the device-id question on ONE port: clear both rows, then write a
/// padded 7-char label to strip N using device id N. Only the ids the D700
/// honours will render. Padding matters - the LCD is a linear 56-char buffer
/// and a short write leaves the old bytes in place (this is the "PORT-1J" bug).
fn idtest() -> R<()> {
    let ids: [u8; 4] = [0x10, 0x11, 0x14, 0x15];
    let name = out_ports()?.into_iter().next().ok_or("no D700 output")?;
    let mut conn = open_out(&name)?;

    // Clear both rows with every id, so leftovers cannot masquerade as a hit.
    for id in ids {
        for row in [0x00u8, 0x38] {
            let mut msg = vec![0xF0, 0x00, 0x00, 0x66, id, 0x12, row];
            msg.extend(vec![b' '; 56]);
            msg.push(0xF7);
            conn.send(&msg)?;
        }
    }
    println!("cleared display on {name}");
    sleep(Duration::from_millis(600));

    for (slot, id) in ids.iter().enumerate() {
        let label = format!("ID-{id:02X}  ");
        let mut msg = vec![0xF0, 0x00, 0x00, 0x66, *id, 0x12, (slot * 7) as u8];
        msg.extend(label.as_bytes());
        msg.push(0xF7);
        conn.send(&msg)?;
        println!(
            "strip {} <- \"{}\" using device id 0x{id:02X}",
            slot + 1,
            label.trim()
        );
        sleep(Duration::from_millis(400));
    }
    println!("\nOnly strips whose device id the D700 honours will show text.");
    println!("Strips 1-4 = ids 0x10, 0x11, 0x14, 0x15. Blank = that id is ignored.");
    Ok(())
}

/// Follow-up to idtest: id 0x10 rendered "ID-!)" where "ID-10" was sent, while
/// 0x11/0x14/0x15 were exact. Repeated identical letters remove glyph ambiguity,
/// so whatever strip 1 shows is unambiguous evidence about the 0x10 path.
fn idchar() -> R<()> {
    let cases: [(u8, u8); 4] = [(0x10, b'A'), (0x11, b'B'), (0x14, b'C'), (0x15, b'D')];
    let name = out_ports()?.into_iter().next().ok_or("no D700 output")?;
    let mut conn = open_out(&name)?;

    // Clear with a known-good id (0x14 rendered exactly in the previous test).
    for row in [0x00u8, 0x38] {
        let mut msg = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, row];
        msg.extend(vec![b' '; 56]);
        msg.push(0xF7);
        conn.send(&msg)?;
    }
    println!("cleared via id 0x14");
    sleep(Duration::from_millis(700));

    for (slot, (id, ch)) in cases.iter().enumerate() {
        let text: Vec<u8> = vec![*ch; 7];
        let mut msg = vec![0xF0, 0x00, 0x00, 0x66, *id, 0x12, (slot * 7) as u8];
        msg.extend(&text);
        msg.push(0xF7);
        conn.send(&msg)?;
        println!(
            "strip {} <- \"{}\" (7x '{}') using id 0x{id:02X}",
            slot + 1,
            String::from_utf8_lossy(&text),
            *ch as char
        );
        sleep(Duration::from_millis(400));
    }
    println!("\nExpect: AAAAAAA BBBBBBB CCCCCCC DDDDDDD");
    println!("If strip 1 is not 7 clean A's, id 0x10 is not a plain LCD write.");
    Ok(())
}

/// Master-section notes confirmed by capture, in panel order.
const MASTER: [(u8, &str); 13] = [
    (0x2A, "Pan"),
    (0x2C, "EQ"),
    (0x29, "Send"),
    (0x2B, "FX"),
    (0x36, "*"),
    (0x59, "Icon 1"),
    (0x56, "Icon 2"),
    (0x5F, "Record"),
    (0x5E, "Play"),
    (0x5D, "Stop"),
    (0x2E, "Arrow L"),
    (0x2F, "Arrow R"),
    (0x38, "Knob press"),
];

/// Light each master-section button in turn, then all at once.
fn master() -> R<()> {
    let name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&name)?;
    println!("[{name}] master section, one at a time:");
    for (note, label) in MASTER {
        println!("   0x{note:02X}  {label}");
        conn.send(&[0x90, note, 127])?;
        sleep(Duration::from_millis(450));
        conn.send(&[0x90, note, 0])?;
    }
    println!("   all together x3");
    for _ in 0..3 {
        for (note, _) in MASTER {
            conn.send(&[0x90, note, 127])?;
        }
        sleep(Duration::from_millis(350));
        for (note, _) in MASTER {
            conn.send(&[0x90, note, 0])?;
        }
        sleep(Duration::from_millis(250));
    }
    Ok(())
}

/// Scribble-strip colour, X-Touch style: F0 00 00 66 <id> 72 <8 colours> F7.
/// 0 black, 1 red, 2 green, 3 yellow, 4 blue, 5 magenta, 6 cyan, 7 white.
/// Untested on Asparion - this is the experiment.
fn colors() -> R<()> {
    let names = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    for name in out_ports()? {
        let mut conn = open_out(&name)?;

        // Label the strips so colour changes are visible against text.
        for strip in 0..8u8 {
            let label = format!("{:<7}", names[strip as usize]);
            let mut msg = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, strip * 7];
            msg.extend(label.as_bytes()[..7].to_vec());
            msg.push(0xF7);
            conn.send(&msg)?;
        }

        println!("[{name}] rainbow: each strip its own colour");
        let mut msg = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
        msg.extend([0u8, 1, 2, 3, 4, 5, 6, 7]);
        msg.push(0xF7);
        conn.send(&msg)?;
        sleep(Duration::from_millis(2000));

        for (c, cname) in names.iter().enumerate().skip(1) {
            println!("   all strips -> {cname}");
            let mut msg = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
            msg.extend(vec![c as u8; 8]);
            msg.push(0xF7);
            conn.send(&msg)?;
            sleep(Duration::from_millis(900));
        }
    }
    println!("\nDid the strip backlights change colour? If nothing changed,");
    println!("the D700 does not implement the 0x72 colour command.");
    Ok(())
}

/// Everything at once: faders, channel LEDs, rings, master buttons, displays.
fn show() -> R<()> {
    let ports = out_ports()?;
    let mut conns: Vec<_> = ports.iter().filter_map(|n| open_out(n).ok()).collect();
    println!("light show on {} port(s)", conns.len());

    for (i, c) in conns.iter_mut().enumerate() {
        let text = format!("{:<7}", if i == 0 { "S21" } else { "HIJACK" });
        for strip in 0..8u8 {
            let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, strip * 7];
            m.extend(text.as_bytes()[..7].to_vec());
            m.push(0xF7);
            c.send(&m)?;
        }
    }

    // Chase: channel LEDs + rings + faders travel left to right.
    for pass in 0..3 {
        for i in 0..8u8 {
            for c in conns.iter_mut() {
                for base in [0x00u8, 0x08, 0x10, 0x18] {
                    c.send(&[0x90, base + i, 127])?;
                }
                c.send(&[0xB0, 0x30 + i, 0x2B])?;
                let pos: u16 = if pass % 2 == 0 { 14000 } else { 2000 };
                c.send(&[0xE0 | i, (pos & 0x7F) as u8, (pos >> 7) as u8])?;
            }
            sleep(Duration::from_millis(90));
            for c in conns.iter_mut() {
                for base in [0x00u8, 0x08, 0x10, 0x18] {
                    c.send(&[0x90, base + i, 0])?;
                }
                c.send(&[0xB0, 0x30 + i, 0])?;
            }
        }
        for (note, _) in MASTER {
            conns[0].send(&[0x90, note, 127])?;
        }
        sleep(Duration::from_millis(200));
        for (note, _) in MASTER {
            conns[0].send(&[0x90, note, 0])?;
        }
    }

    // Park everything.
    for c in conns.iter_mut() {
        for i in 0..8u8 {
            c.send(&[0xE0 | i, 0, 0])?;
            c.send(&[0xB0, 0x30 + i, 0])?;
        }
    }
    println!("done");
    Ok(())
}

/// How wide is the colour vocabulary? X-Touch uses the low 3 bits for colour
/// and spare bits for row inversion. Walk 0..31 eight at a time, labelling each
/// strip with the raw value so the hardware reports what it understands.
fn colors2() -> R<()> {
    let name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&name)?;

    for block in 0..4u8 {
        let base = block * 8;

        // Blank out first so the transition between blocks is obvious.
        for strip in 0..8u8 {
            let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, strip * 7];
            m.extend(vec![b' '; 7]);
            m.push(0xF7);
            conn.send(&m)?;
        }
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
        m.extend(vec![0u8; 8]);
        m.push(0xF7);
        conn.send(&m)?;
        sleep(Duration::from_millis(900));

        for strip in 0..8u8 {
            let label = format!("v{:<6}", base + strip);
            let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, strip * 7];
            m.extend(label.as_bytes()[..7].to_vec());
            m.push(0xF7);
            conn.send(&m)?;
        }
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
        m.extend((0..8u8).map(|i| base + i));
        m.push(0xF7);
        conn.send(&m)?;

        println!(
            ">>> BLOCK {}: values {}..{} on strips 1..8 - holding 6s",
            block + 1,
            base,
            base + 7
        );
        if block == 0 {
            println!("    (reference: 0 black, 1 red, 2 green, 3 yellow,");
            println!("               4 blue, 5 magenta, 6 cyan, 7 white)");
        }
        sleep(Duration::from_secs(6));
    }

    let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
    m.extend(vec![7u8; 8]);
    m.push(0xF7);
    conn.send(&m)?;
    println!(
        "
For blocks 2-4: same eight colours repeating, or something new"
    );
    println!("(extra colours, inverted text, dimmed backlight)?");
    Ok(())
}

/// Name each strip with the colour it is being set to, and hold, so the operator
/// can confirm the value->colour mapping directly rather than by recollection.
/// Drives EVERY bank, so a 16-fader rack shows the reference on both modules.
fn cverify() -> R<()> {
    let names = ["red", "green", "yellow", "blue", "magenta", "cyan", "white"];
    for (pi, port) in out_ports()?.iter().enumerate() {
        let mut conn = open_out(port)?;
        for (i, n) in names.iter().enumerate() {
            let label = format!("{:<7}", n);
            let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, (i * 7) as u8];
            m.extend(label.as_bytes()[..7].to_vec());
            m.push(0xF7);
            conn.send(&m)?;
        }
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, 7 * 7];
        m.extend(b"-black-".to_vec());
        m.push(0xF7);
        conn.send(&m)?;

        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
        m.extend([1u8, 2, 3, 4, 5, 6, 7, 0]);
        m.push(0xF7);
        conn.send(&m)?;
        println!(
            "bank {} ({port}): red green yellow blue magenta cyan white black",
            pi + 1
        );
    }
    println!(
        "
Both banks should now show the SAME labelled reference."
    );
    println!("Do the colours match the names on each module?");
    Ok(())
}

/// Can the master dial take a colour? The 0x72 command defines 8 bytes for the
/// 8 strips, so if Asparion extended it the master sits in a 9th (or later) byte.
/// Phase 1 asks "does anything beyond strip 8 respond at all", phase 2 isolates
/// which byte position drives it.
fn mcolor() -> R<()> {
    for name in out_ports()? {
        let mut conn = open_out(&name)?;
        println!("\n===== {name} =====");

        // Phase 1: 16 red bytes. Strips 1-8 go red either way; the question is
        // whether anything else on the panel follows.
        println!("phase 1: 16 colour bytes, all red - does the MASTER DIAL turn red?");
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
        m.extend(vec![1u8; 16]);
        m.push(0xF7);
        conn.send(&m)?;
        sleep(Duration::from_secs(4));

        // Back to black so phase 2 starts from a clean slate.
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
        m.extend(vec![0u8; 16]);
        m.push(0xF7);
        conn.send(&m)?;
        sleep(Duration::from_millis(800));

        // Phase 2: strips 1-8 stay black; light exactly one later position.
        println!("phase 2: strips black, one extra byte green at a time");
        for pos in 9..=16usize {
            let mut colours = vec![0u8; pos];
            colours[pos - 1] = 2; // green
            let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
            m.extend(&colours);
            m.push(0xF7);
            conn.send(&m)?;
            println!("   byte #{pos} = green");
            sleep(Duration::from_millis(1500));
        }
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
        m.extend(vec![0u8; 16]);
        m.push(0xF7);
        conn.send(&m)?;
    }
    println!("\nPhase 1: did anything beyond the 8 strips turn red?");
    println!("Phase 2: if the master lit, at which byte number?");
    Ok(())
}

/// Second attempt at the master dial. Two angles the first pass did not try:
/// a ring CC above the 8 strip rings, and a sibling SysEx command beside 0x72.
///
/// The SysEx sweep is deliberately confined to 0x70..0x7F - the vendor
/// extension region where 0x72 lives. The low MCU command range is avoided on
/// purpose: it contains "go offline" (0x0F) and reset commands (0x61-0x63).
fn mcolor2() -> R<()> {
    let name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&name)?;

    println!("PART A: ring CCs above the 8 strip rings (0x38..0x3F)");
    println!("        watching for ANY light on the master dial");
    for cc in 0x38..=0x3Fu8 {
        for v in [0x0Bu8, 0x2B, 0x00] {
            conn.send(&[0xB0, cc, v])?;
            sleep(Duration::from_millis(220));
        }
        println!("   CC 0x{cc:02X} swept");
    }

    println!("\nPART B: SysEx command bytes 0x70..0x7F, 9 red bytes each");
    for cmd in 0x70..=0x7Fu8 {
        if cmd == 0x72 {
            println!("   0x{cmd:02X} skipped (known strip-colour command)");
            continue;
        }
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, cmd];
        m.extend(vec![1u8; 9]);
        m.push(0xF7);
        conn.send(&m)?;
        println!("   0x{cmd:02X} sent");
        sleep(Duration::from_millis(900));
    }

    // Leave the strips in a known state regardless of what happened.
    let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
    m.extend(vec![7u8; 8]);
    m.push(0xF7);
    conn.send(&m)?;
    for i in 0..8u8 {
        conn.send(&[0xB0, 0x30 + i, 0])?;
    }

    println!("\nPart A: did any CC light the master dial?");
    println!("Part B: did any command byte change it?");
    println!("If the surface behaves oddly, power-cycle it - nothing sent here persists.");
    Ok(())
}

/// Ask the device to identify itself, listening while we ask.
///
/// All three queries are read-only - no state is changed. The prize is a vendor
/// manufacturer id: Asparion's own commands would live under `F0 <their id>`,
/// entirely outside the `F0 00 00 66` Mackie space we have been probing.
fn ident() -> R<()> {
    // Listen on every D700 input first, so replies are not missed.
    let probe = MidiInput::new("probe")?;
    let in_names: Vec<String> = probe
        .ports()
        .iter()
        .filter_map(|p| probe.port_name(p).ok())
        .filter(|n| n.to_lowercase().contains(MATCH))
        .collect();
    let mut conns = Vec::new();
    for name in &in_names {
        let mut mi = MidiInput::new("probe")?;
        mi.ignore(Ignore::None);
        let port = mi
            .ports()
            .into_iter()
            .find(|p| mi.port_name(p).map(|n| &n == name).unwrap_or(false))
            .ok_or("port vanished")?;
        let tag = name.clone();
        conns.push(mi.connect(
            &port,
            "probe-in",
            move |_t, msg, _| {
                println!("   <<< {tag} ({} bytes)", msg.len());
                println!("       {}", hex_full(msg));
                let ascii: String = msg
                    .iter()
                    .map(|&c| {
                        if (0x20..0x7F).contains(&c) {
                            c as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                println!("       ascii: \"{ascii}\"");
            },
            (),
        )?);
    }
    println!("listening on {} input(s)\n", conns.len());

    let queries: [(&str, Vec<u8>); 3] = [
        (
            "Universal Identity Request (MIDI standard)",
            vec![0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7],
        ),
        (
            "MCU device query",
            vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7],
        ),
        (
            "MCU version request",
            vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x14, 0xF7],
        ),
    ];

    for out_name in out_ports()? {
        let mut conn = open_out(&out_name)?;
        for (label, bytes) in &queries {
            println!(">>> {out_name}: {label}");
            println!("    {}", hex(bytes));
            conn.send(bytes)?;
            sleep(Duration::from_millis(1500));
        }
    }
    println!("\nAny '<<<' line above is the device answering. A manufacturer id in an");
    println!("identity reply opens a whole command space we have not touched.");
    Ok(())
}

/// The MCU challenge-response. The device sends a 4-byte challenge in its
/// connection query; the host must answer with these four derived bytes or the
/// connection is refused. Algorithm is the long-published Logic Control one.
fn mcu_response(c: &[u8]) -> [u8; 4] {
    let (c0, c1, c2, c3) = (c[0] as u32, c[1] as u32, c[2] as u32, c[3] as u32);
    let r0 = c0.wrapping_add(c1 ^ 0x0A).wrapping_sub(c3);
    let r1 = (c2 >> 4) ^ (c0.wrapping_add(c3));
    let r2 = c3.wrapping_sub(c2 << 2) ^ (c0 | c1);
    let r3 = c1.wrapping_sub(c2).wrapping_add(0xF0 ^ (c3 << 4));
    [
        (r0 & 0x7F) as u8,
        (r1 & 0x7F) as u8,
        (r2 & 0x7F) as u8,
        (r3 & 0x7F) as u8,
    ]
}

/// Complete the MCU connection handshake, then retry the master dial colour.
/// If the surface gates features behind a live host connection, this is what
/// the earlier probes were missing.
fn handshake() -> R<()> {
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();

    let probe = MidiInput::new("probe")?;
    let in_names: Vec<String> = probe
        .ports()
        .iter()
        .filter_map(|p| probe.port_name(p).ok())
        .filter(|n| n.to_lowercase().contains(MATCH))
        .collect();
    let mut conns = Vec::new();
    for name in &in_names {
        let mut mi = MidiInput::new("probe")?;
        mi.ignore(Ignore::None);
        let port = mi
            .ports()
            .into_iter()
            .find(|p| mi.port_name(p).map(|n| &n == name).unwrap_or(false))
            .ok_or("port vanished")?;
        let t = tx.clone();
        let tag = name.clone();
        conns.push(mi.connect(
            &port,
            "probe-in",
            move |_ts, msg, _| {
                if msg.first() == Some(&0xF0) {
                    println!("   <<< {tag}: {}", hex_full(msg));
                    let _ = t.send(msg.to_vec());
                }
            },
            (),
        )?);
    }

    let out_name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&out_name)?;

    println!(">>> device query");
    conn.send(&[0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7])?;

    let reply = rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| "no reply")?;
    if reply.len() < 18 || reply[5] != 0x01 {
        println!("unexpected reply shape: {}", hex_full(&reply));
        return Ok(());
    }
    let serial = &reply[6..13];
    let challenge = &reply[13..17];
    println!(
        "    serial    {}  (\"{}\")",
        hex_full(serial),
        String::from_utf8_lossy(serial)
    );
    println!("    challenge {}", hex_full(challenge));

    let resp = mcu_response(challenge);
    println!("    response  {}", hex_full(&resp));

    let mut msg = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x02];
    msg.extend(serial);
    msg.extend(resp);
    msg.push(0xF7);
    println!(">>> host connection reply\n    {}", hex_full(&msg));
    conn.send(&msg)?;

    match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(m) if m.len() > 5 && m[5] == 0x03 => println!("    ACCEPTED (0x03 confirmation)"),
        Ok(m) if m.len() > 5 && m[5] == 0x04 => println!("    REFUSED (0x04 error)"),
        Ok(m) => println!("    other: {}", hex_full(&m)),
        Err(_) => println!("    (no confirmation - many surfaces stay silent)"),
    }

    println!("\n>>> now retrying master dial colour with the connection live");
    for (label, bytes) in [
        ("0x72 with 9 bytes, 9th red", {
            let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
            m.extend(vec![0u8; 8]);
            m.push(1);
            m.push(0xF7);
            m
        }),
        ("0x72 with 16 bytes, all green", {
            let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
            m.extend(vec![2u8; 16]);
            m.push(0xF7);
            m
        }),
    ] {
        println!("    {label}");
        conn.send(&bytes)?;
        sleep(Duration::from_secs(3));
    }

    let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
    m.extend(vec![7u8; 8]);
    m.push(0xF7);
    conn.send(&m)?;
    println!("\nDid the master dial colour change this time?");
    Ok(())
}

/// How many display rows are there really?
///
/// MCU defines two rows of 56: offsets 0x00-0x37 and 0x38-0x6F. SysEx data
/// bytes cap at 0x7F, so 0x70-0x7F is the only space a third row could occupy
/// under this command - 16 characters, not a full row. If the D700 has more
/// lines than two it likely needs a different command, but offsets are free to
/// test and rule out.
fn rows() -> R<()> {
    let name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&name)?;

    let write = |conn: &mut midir::MidiOutputConnection, off: u8, text: &[u8]| -> R<()> {
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, off];
        m.extend(text);
        m.push(0xF7);
        conn.send(&m)?;
        Ok(())
    };

    // Clear the documented two rows.
    write(&mut conn, 0x00, &[b' '; 56])?;
    write(&mut conn, 0x38, &[b' '; 56])?;
    sleep(Duration::from_millis(500));

    println!("step 1: row 1 (offset 0x00) <- 'AAAAAAA' per strip");
    let row1: Vec<u8> = (0..8).flat_map(|_| b"AAAAAAA".to_vec()).collect();
    write(&mut conn, 0x00, &row1)?;
    sleep(Duration::from_secs(3));

    println!("step 2: row 2 (offset 0x38) <- 'BBBBBBB' per strip");
    let row2: Vec<u8> = (0..8).flat_map(|_| b"BBBBBBB".to_vec()).collect();
    write(&mut conn, 0x38, &row2)?;
    sleep(Duration::from_secs(4));

    println!("step 3: offset 0x70 <- 16 x 'C'  (a third row would start here)");
    write(&mut conn, 0x70, &[b'C'; 16])?;
    sleep(Duration::from_secs(4));

    println!("step 4: walking offsets 0x70..0x7F one at a time with 'D'");
    for off in 0x70..=0x7Fu8 {
        write(&mut conn, off, b"D")?;
        sleep(Duration::from_millis(220));
    }
    sleep(Duration::from_secs(2));

    println!("\n1. Did row 1 fill with A?");
    println!("2. Did a SECOND line fill with B - and is it on the same display?");
    println!("3. Did any C or D appear anywhere - a third line, or stray characters?");
    Ok(())
}

/// Hunt for a command that writes the display lines beyond MCU's two.
///
/// 0x12 addresses a 128-char buffer; two rows of 56 fill 112 of it, so extra
/// physical lines must live behind another command. Colour (0x72) proved
/// Asparion implements vendor extensions, so a display extension is plausible.
///
/// Deliberately EXCLUDED as unsafe: 0x0A-0x0F (device config and "go offline")
/// and 0x61-0x63 (fader / LED / global reset). Sweeping those on a working rig
/// to find a text command is a bad trade.
fn rows2() -> R<()> {
    let name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&name)?;

    // Known-good rows carry markers, so a NEW line is unmistakable.
    for (off, ch) in [(0x00u8, b'1'), (0x38, b'2')] {
        let text: Vec<u8> = (0..8).flat_map(|_| vec![ch; 7]).collect();
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, off];
        m.extend(text);
        m.push(0xF7);
        conn.send(&m)?;
    }
    println!("rows 1 and 2 marked with 1s and 2s\n");
    sleep(Duration::from_millis(800));

    let skip = |c: u8| {
        (0x0A..=0x0F).contains(&c)
            || (0x61..=0x63).contains(&c)
            || c == 0x12
            || c == 0x14
            || c == 0x72
    };

    println!("sweeping command bytes for a third-line write");
    println!("payload = offset 0x00 + 7 chars naming the command\n");
    for cmd in 0x10..=0x7Fu8 {
        if skip(cmd) {
            continue;
        }
        let label = format!("c{cmd:02X}<<<<");
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, cmd, 0x00];
        m.extend(label.as_bytes()[..7].to_vec());
        m.push(0xF7);
        conn.send(&m)?;
        print!("{cmd:02X} ");
        use std::io::Write as _;
        std::io::stdout().flush().ok();
        sleep(Duration::from_millis(320));
    }
    println!("\n\nIf a THIRD line ever showed text, it read cNN<<<< - tell me NN.");
    println!("Rows 1 and 2 should still read 1s and 2s.");
    Ok(())
}

/// 0x18 also writes the display. Two questions follow: which OTHER commands in
/// that neighbourhood write, and does 0x18 address a bigger buffer than 0x12?
/// If it does, the physical lines beyond two may be reachable after all.
fn cmd18() -> R<()> {
    let name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&name)?;

    let send = |conn: &mut midir::MidiOutputConnection, cmd: u8, off: u8, t: &str| -> R<()> {
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, cmd, off];
        m.extend(t.as_bytes());
        m.push(0xF7);
        conn.send(&m)?;
        Ok(())
    };

    // ---- Test A: which commands write? One per strip, so all are visible at once.
    for off in [0x00u8, 0x38] {
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, off];
        m.extend(vec![b'.'; 56]);
        m.push(0xF7);
        conn.send(&m)?;
    }
    sleep(Duration::from_millis(400));

    let cands: [u8; 7] = [0x10, 0x11, 0x13, 0x15, 0x16, 0x17, 0x18];
    println!("TEST A - one candidate command per strip, row 1:");
    for (i, cmd) in cands.iter().enumerate() {
        let label = format!("cmd-{cmd:02X}");
        send(&mut conn, *cmd, (i * 7) as u8, &format!("{label:<7}"))?;
        println!("   strip {} <- command 0x{cmd:02X}", i + 1);
        sleep(Duration::from_millis(300));
    }
    println!("   (strip 8 left as dots - reference)");
    sleep(Duration::from_secs(5));

    // ---- Test B: does 0x18 reach past the two documented rows?
    for off in [0x00u8, 0x38] {
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, off];
        m.extend(vec![b'.'; 56]);
        m.push(0xF7);
        conn.send(&m)?;
    }
    sleep(Duration::from_millis(400));

    println!("\nTEST B - command 0x18 at row offsets 0x00 / 0x38 / 0x70:");
    for (off, tag) in [(0x00u8, "R1--18-"), (0x38, "R2--18-"), (0x70, "R3--18-")] {
        send(&mut conn, 0x18, off, tag)?;
        println!("   offset 0x{off:02X} <- \"{tag}\"");
        sleep(Duration::from_millis(900));
    }
    sleep(Duration::from_secs(3));

    println!("\nTEST A: which strips show 'cmd-NN' rather than dots?");
    println!("TEST B: did R1/R2/R3 land on line 1, line 2, and anywhere else?");
    Ok(())
}

/// Exactly 7 characters: truncate or pad. The LCD is a flat buffer, so a short
/// write leaves stale bytes behind - this is the "PORT-1J" lesson as a function.
fn pad7(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().take(7).collect();
    while v.len() < 7 {
        v.push(b' ');
    }
    v
}

/// Health check that doubles as a preview: channel name on row 1, live value on
/// row 2, colour by channel type. Uses ONLY 0x12 (text) and 0x72 (colour) -
/// the commands established as safe. Nothing else is sent.
fn demo() -> R<()> {
    // (name, value, colour) - colour: 1 red 2 green 3 yellow 4 blue 5 magenta 6 cyan 7 white
    let bank1: [(&str, &str, u8); 8] = [
        ("Kick", "-6.2dB", 3),
        ("Snare", "-3.0dB", 3),
        ("HiHat", "-12.4d", 3),
        ("Bass", "-4.8dB", 2),
        ("Gtr L", "-8.1dB", 6),
        ("Gtr R", "-8.1dB", 6),
        ("Keys", "-5.5dB", 5),
        ("Vox", " 0.0dB", 1),
    ];
    let bank2: [(&str, &str, u8); 8] = [
        ("Aux 1", "-10.0d", 4),
        ("Aux 2", "-14.2d", 4),
        ("Aux 3", "-8.8dB", 4),
        ("Aux 4", "-6.0dB", 4),
        ("Grp 1", "-2.1dB", 5),
        ("Grp 2", "-2.1dB", 5),
        ("Mtx 1", "-18.0d", 6),
        ("Main", " 0.0dB", 7),
    ];

    for (pi, port) in out_ports()?.iter().enumerate() {
        let mut conn = open_out(port)?;
        let strips = if pi == 0 { &bank1 } else { &bank2 };

        let mut row1 = Vec::new();
        let mut row2 = Vec::new();
        for (name, value, _) in strips.iter() {
            row1.extend(pad7(name));
            row2.extend(pad7(value));
        }

        for (off, data) in [(0x00u8, &row1), (0x38u8, &row2)] {
            let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, off];
            m.extend(data.iter().copied());
            m.push(0xF7);
            conn.send(&m)?;
        }

        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
        m.extend(strips.iter().map(|(_, _, c)| *c));
        m.push(0xF7);
        conn.send(&m)?;

        println!(
            "bank {}: {}",
            pi + 1,
            strips
                .iter()
                .map(|(n, _, _)| *n)
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    println!("\nRow 1 = channel name, row 2 = level, colour = channel type.");
    println!("Only 0x12 and 0x72 were sent.");
    Ok(())
}

/// Listen on EVERY plausible Connector TX port at once, so a config change
/// does not cost another capture run. 7000 is the Connector's own Rx and is
/// skipped - it is already bound by the Connector itself.
fn oscdump(secs: u64) -> R<()> {
    use std::collections::BTreeMap;
    use std::net::UdpSocket;
    use std::sync::{Arc, Mutex};

    let ports = [7001u16, 8000, 8001, 9000, 9001, 10000];
    let seen: Arc<Mutex<BTreeMap<String, usize>>> = Arc::new(Mutex::new(BTreeMap::new()));
    let total = Arc::new(Mutex::new(0usize));
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    let mut handles = Vec::new();

    for port in ports {
        let sock = match UdpSocket::bind(("0.0.0.0", port)) {
            Ok(s) => s,
            Err(e) => {
                println!("  port {port}: cannot bind ({e})");
                continue;
            }
        };
        sock.set_read_timeout(Some(Duration::from_millis(300))).ok();
        println!("  listening on {port}");
        let seen = Arc::clone(&seen);
        let total = Arc::clone(&total);
        handles.push(std::thread::spawn(move || {
            let mut buf = [0u8; 65536];
            while std::time::Instant::now() < deadline {
                if let Ok((n, src)) = sock.recv_from(&mut buf) {
                    *total.lock().unwrap() += 1;
                    match rosc::decoder::decode_udp(&buf[..n]) {
                        Ok((_, packet)) => {
                            let mut g = seen.lock().unwrap();
                            print!("  :{port} ");
                            print_packet(&packet, src, &mut g);
                        }
                        Err(e) => println!("  :{port} {src} undecodable ({n} B): {e:?}"),
                    }
                }
            }
        }));
    }

    println!(
        "
move faders / press buttons on the D700 for {secs}s
"
    );
    for h in handles {
        let _ = h.join();
    }

    let seen = seen.lock().unwrap();
    println!(
        "
--- {} packets, {} distinct addresses ---",
        total.lock().unwrap(),
        seen.len()
    );
    for (addr, n) in seen.iter() {
        println!("  {n:>5}  {addr}");
    }
    if seen.is_empty() {
        println!("  (nothing arrived - check the Connector's Tx port and that");
        println!("   the D700 is on the OSC preset)");
    }
    Ok(())
}

fn print_packet(
    p: &rosc::OscPacket,
    src: std::net::SocketAddr,
    seen: &mut std::collections::BTreeMap<String, usize>,
) {
    match p {
        rosc::OscPacket::Message(m) => {
            *seen.entry(m.addr.clone()).or_insert(0) += 1;
            let args: Vec<String> = m.args.iter().map(|a| format!("{a:?}")).collect();
            println!("  {src}  {}  [{}]", m.addr, args.join(", "));
        }
        rosc::OscPacket::Bundle(b) => {
            for inner in &b.content {
                print_packet(inner, src, seen);
            }
        }
    }
}

/// Send one OSC message to the Connector's Rx port (7000).
/// Bare numeric args become floats; anything else is sent as a string.
fn oscsend(args: &[String]) -> R<()> {
    use std::net::UdpSocket;
    let path = args.first().ok_or("usage: oscsend <path> [args...]")?;
    let osc_args: Vec<rosc::OscType> = args[1..]
        .iter()
        .map(|a| match a.parse::<f32>() {
            Ok(f) => rosc::OscType::Float(f),
            Err(_) => rosc::OscType::String(a.clone()),
        })
        .collect();

    let msg = rosc::OscPacket::Message(rosc::OscMessage {
        addr: path.clone(),
        args: osc_args.clone(),
    });
    let buf = rosc::encoder::encode(&msg)?;
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.send_to(&buf, "127.0.0.1:7000")?;
    println!("sent to 127.0.0.1:7000  {path}  {osc_args:?}");
    Ok(())
}

/// Probe the Connector's INBOUND path: can OSC set fader positions, text and
/// colour? Safe to run - an unrecognised OSC address is ignored, unlike the
/// SysEx sweep that wedged the controller.
///
/// The outbound addresses (/play, /click, /repeat, /device/track/bank/+-) are
/// the DrivenByMoss scheme, so /track/{n}/... is the leading hypothesis.
fn oscprobe() -> R<()> {
    use std::net::UdpSocket;
    let sock = UdpSocket::bind("0.0.0.0:0")?;

    let send = |path: &str, args: Vec<rosc::OscType>| -> R<()> {
        let msg = rosc::OscPacket::Message(rosc::OscMessage {
            addr: path.to_string(),
            args: args.clone(),
        });
        sock.send_to(&rosc::encoder::encode(&msg)?, "127.0.0.1:7000")?;
        println!("   {path}  {args:?}");
        sleep(Duration::from_millis(1100));
        Ok(())
    };

    use rosc::OscType::{Float, Int, String as S};

    println!("PHASE 1 - FADER 1. Watch for the motor moving.");
    send("/track/1/volume", vec![Float(0.9)])?;
    send("/track/1/volume", vec![Int(100)])?;
    send("/track/1/fader", vec![Float(0.2)])?;
    send("/track/1/level", vec![Float(0.9)])?;
    send("/fader/1", vec![Float(0.2)])?;
    send("/volume/1", vec![Float(0.9)])?;
    send("/1/fader1", vec![Float(0.2)])?;
    send("/master/volume", vec![Float(0.8)])?;

    println!("\nPHASE 2 - TEXT on strip 1. Watch the display.");
    send("/track/1/name", vec![S("OSCNAME".into())])?;
    send("/track/1/label", vec![S("OSCLBL".into())])?;
    send("/track/1/text", vec![S("OSCTXT".into())])?;
    send("/display/1", vec![S("OSCDSP".into())])?;
    send("/track/1/vu", vec![Float(0.7)])?;

    println!("\nPHASE 3 - COLOUR on strip 1. Watch the strip and dial.");
    send("/track/1/color", vec![Float(1.0), Float(0.0), Float(0.0)])?;
    send("/track/1/color", vec![Int(255), Int(0), Int(0)])?;
    send("/track/1/color", vec![S("red".into())])?;
    send("/track/1/colour", vec![Float(0.0), Float(1.0), Float(0.0)])?;
    send("/track/1/color", vec![Int(1)])?;

    println!("\nPHASE 4 - BUTTON LEDS on strip 1.");
    send("/track/1/mute", vec![Int(1)])?;
    send("/track/1/solo", vec![Int(1)])?;
    send("/track/1/select", vec![Int(1)])?;
    send("/track/1/recarm", vec![Int(1)])?;

    println!("\nDid ANYTHING move, light up, or print? Tell me which line.");
    Ok(())
}

/// Second inbound attempt, using ONLY addresses the Connector has been
/// observed to emit. If it speaks a vocabulary, that is the vocabulary - and
/// in the DrivenByMoss scheme these same addresses carry LED state inbound.
fn oscprobe2() -> R<()> {
    use std::net::UdpSocket;
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    let send = |path: &str, args: Vec<rosc::OscType>, wait: u64| -> R<()> {
        let msg = rosc::OscPacket::Message(rosc::OscMessage {
            addr: path.to_string(),
            args: args.clone(),
        });
        sock.send_to(&rosc::encoder::encode(&msg)?, "127.0.0.1:7000")?;
        println!("   {path}  {args:?}");
        sleep(Duration::from_millis(wait));
        Ok(())
    };
    use rosc::OscType::{Float, Int};

    println!("PHASE A - ask the Connector to refresh / announce");
    send("/refresh", vec![], 900)?;
    send("/reload", vec![], 900)?;

    println!("\nPHASE B - the exact addresses it EMITS, sent back with a value.");
    println!("          Watch the master-section LEDs (Play, Stop, Record, *).");
    for path in ["/play", "/stop", "/record", "/click", "/repeat"] {
        send(path, vec![Int(1)], 1000)?;
    }
    println!("   ...now with float 1.0");
    for path in ["/play", "/stop", "/record", "/click", "/repeat"] {
        send(path, vec![Float(1.0)], 800)?;
    }
    println!("   ...now bare, no argument (exactly as it sends them)");
    for path in ["/play", "/stop", "/record", "/click", "/repeat"] {
        send(path, vec![], 800)?;
    }

    println!("\nPHASE C - clear them again");
    for path in ["/play", "/stop", "/record", "/click", "/repeat"] {
        send(path, vec![Int(0)], 250)?;
    }

    println!("\nDid any master-section LED light during phase B?");
    println!("If bare messages worked, the map is symmetric - it speaks one vocabulary.");
    Ok(())
}

/// Drive an operator-authored OSC address that takes an RGB colour.
/// Argument shape is unknown, so try each plausible encoding with a distinct
/// colour - whichever makes the dial change tells us the format.
fn rgbtest(path: &str) -> R<()> {
    use rosc::OscType::{Float, Int, String as S};
    use std::net::UdpSocket;
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    let send = |label: &str, args: Vec<rosc::OscType>| -> R<()> {
        let msg = rosc::OscPacket::Message(rosc::OscMessage {
            addr: path.to_string(),
            args: args.clone(),
        });
        sock.send_to(&rosc::encoder::encode(&msg)?, "127.0.0.1:7000")?;
        println!("   {label:<28} {args:?}");
        sleep(Duration::from_millis(2200));
        Ok(())
    };

    println!("sending to 127.0.0.1:7000  ->  {path}\n");

    println!("A. three floats 0.0-1.0");
    send("A1 RED", vec![Float(1.0), Float(0.0), Float(0.0)])?;
    send("A2 GREEN", vec![Float(0.0), Float(1.0), Float(0.0)])?;
    send("A3 BLUE", vec![Float(0.0), Float(0.0), Float(1.0)])?;

    println!("B. three ints 0-255");
    send("B1 RED", vec![Int(255), Int(0), Int(0)])?;
    send("B2 GREEN", vec![Int(0), Int(255), Int(0)])?;
    send("B3 BLUE", vec![Int(0), Int(0), Int(255)])?;

    println!("C. three ints 0-127 (MIDI-ish range)");
    send("C1 RED", vec![Int(127), Int(0), Int(0)])?;
    send("C2 GREEN", vec![Int(0), Int(127), Int(0)])?;

    println!("D. single packed / indexed value");
    send("D1 packed red", vec![Int(16711680)])?;
    send("D2 index 1", vec![Int(1)])?;
    send("D3 index 4", vec![Int(4)])?;

    println!("E. string forms");
    send("E1 hex", vec![S("#FF0000".into())])?;
    send("E2 csv", vec![S("255,0,0".into())])?;
    send("E3 name", vec![S("red".into())])?;

    println!("\nWhich line changed the master dial? The label identifies the format.");
    Ok(())
}

/// Which argument format does the RGB address actually accept? Each candidate
/// gets its own colour, with black in between, so several working formats can
/// still be told apart by what the operator saw.
fn rgbwhich(path: &str) -> R<()> {
    use rosc::OscType::{Float, Int, String as S};
    use std::net::UdpSocket;
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    let fire = |args: Vec<rosc::OscType>| -> R<()> {
        let msg = rosc::OscPacket::Message(rosc::OscMessage {
            addr: path.to_string(),
            args,
        });
        sock.send_to(&rosc::encoder::encode(&msg)?, "127.0.0.1:7000")?;
        Ok(())
    };
    let black_f = vec![Float(0.0), Float(0.0), Float(0.0)];

    let cases: [(&str, &str, Vec<rosc::OscType>); 5] = [
        (
            "A",
            "RED    - three floats 0.0-1.0",
            vec![Float(1.0), Float(0.0), Float(0.0)],
        ),
        (
            "B",
            "GREEN  - three ints 0-255",
            vec![Int(0), Int(255), Int(0)],
        ),
        (
            "C",
            "BLUE   - three ints 0-127",
            vec![Int(0), Int(0), Int(127)],
        ),
        ("D", "packed - single int 0xFFFF00", vec![Int(16776960)]),
        (
            "E",
            "string - \"#FF00FF\" magenta",
            vec![S("#FF00FF".into())],
        ),
    ];

    println!("target: {path}\n");
    for (tag, label, args) in cases {
        fire(black_f.clone())?;
        sleep(Duration::from_millis(1200));
        println!("   [{tag}]  {label}");
        fire(args)?;
        sleep(Duration::from_millis(3000));
    }
    fire(black_f)?;
    println!("\nWhich letters produced a colour, and which colour did you see?");
    println!("A=red B=green C=blue D=yellow E=magenta");
    Ok(())
}

/// Can OSC drive the motor faders? Operator has mapped /fader/1 .. /fader/4.
/// A staircase is used deliberately - four faders at four different heights is
/// unmistakable, where "they all moved" could be imagination.
fn fadertest() -> R<()> {
    use rosc::OscType::{Float, Int};
    use std::net::UdpSocket;
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    let fire = |n: u8, args: Vec<rosc::OscType>| -> R<()> {
        let msg = rosc::OscPacket::Message(rosc::OscMessage {
            addr: format!("/fader/{n}"),
            args,
        });
        sock.send_to(&rosc::encoder::encode(&msg)?, "127.0.0.1:7000")?;
        Ok(())
    };

    println!("PHASE 1 - floats. All four to the bottom, then a STAIRCASE.");
    for n in 1..=4u8 {
        fire(n, vec![Float(0.0)])?;
    }
    sleep(Duration::from_secs(2));
    println!("   staircase: 0.25 / 0.50 / 0.75 / 1.00");
    for (n, v) in [(1u8, 0.25f32), (2, 0.50), (3, 0.75), (4, 1.00)] {
        fire(n, vec![Float(v)])?;
        sleep(Duration::from_millis(400));
    }
    sleep(Duration::from_secs(3));

    println!("   reverse staircase: 1.00 / 0.75 / 0.50 / 0.25");
    for (n, v) in [(1u8, 1.00f32), (2, 0.75), (3, 0.50), (4, 0.25)] {
        fire(n, vec![Float(v)])?;
        sleep(Duration::from_millis(400));
    }
    sleep(Duration::from_secs(3));

    println!("\nPHASE 2 - ints 0-127, in case floats are not the shape");
    for n in 1..=4u8 {
        fire(n, vec![Int(0)])?;
    }
    sleep(Duration::from_secs(2));
    for (n, v) in [(1u8, 32i32), (2, 64), (3, 96), (4, 127)] {
        fire(n, vec![Int(v)])?;
        sleep(Duration::from_millis(400));
    }
    sleep(Duration::from_secs(3));

    println!("\nPHASE 3 - ints 0-16383 (14-bit, matching MIDI pitch bend)");
    for (n, v) in [(1u8, 4096i32), (2, 8192), (3, 12288), (4, 16383)] {
        fire(n, vec![Int(v)])?;
        sleep(Duration::from_millis(400));
    }
    sleep(Duration::from_secs(3));

    println!("\nDid faders 1-4 move? At which phase, and did they form a staircase?");
    Ok(())
}

/// Float-only fader drive, confirmed by the operator as the mapped range
/// (0.0 - 1.0). Slow and obvious: hold each shape long enough to read.
fn faderf() -> R<()> {
    use rosc::OscType::Float;
    use std::net::UdpSocket;
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    let set = |n: u8, v: f32| -> R<()> {
        let msg = rosc::OscPacket::Message(rosc::OscMessage {
            addr: format!("/fader/{n}"),
            args: vec![Float(v)],
        });
        sock.send_to(&rosc::encoder::encode(&msg)?, "127.0.0.1:7000")?;
        Ok(())
    };

    println!("1. all four to the BOTTOM (0.0)");
    for n in 1..=4u8 {
        set(n, 0.0)?;
    }
    sleep(Duration::from_secs(3));

    println!("2. all four to the TOP (1.0)");
    for n in 1..=4u8 {
        set(n, 1.0)?;
    }
    sleep(Duration::from_secs(3));

    println!("3. STAIRCASE 0.25 / 0.50 / 0.75 / 1.00");
    for (n, v) in [(1u8, 0.25f32), (2, 0.50), (3, 0.75), (4, 1.00)] {
        set(n, v)?;
    }
    sleep(Duration::from_secs(4));

    println!("4. INVERTED staircase 1.00 / 0.75 / 0.50 / 0.25");
    for (n, v) in [(1u8, 1.00f32), (2, 0.75), (3, 0.50), (4, 0.25)] {
        set(n, v)?;
    }
    sleep(Duration::from_secs(4));

    println!("5. smooth sweep, all four together, up and down");
    for pass in 0..2 {
        let steps: Vec<f32> = if pass == 0 {
            (0..=20).map(|i| i as f32 / 20.0).collect()
        } else {
            (0..=20).rev().map(|i| i as f32 / 20.0).collect()
        };
        for v in steps {
            for n in 1..=4u8 {
                set(n, v)?;
            }
            sleep(Duration::from_millis(90));
        }
    }

    println!("6. park at the bottom");
    for n in 1..=4u8 {
        set(n, 0.0)?;
    }
    println!("\nDid faders 1-4 move?");
    Ok(())
}
