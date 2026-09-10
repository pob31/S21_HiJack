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
        "mlive" => mlive(),
        "monlive" => monlive(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(25)),
        "mpos" => mpos(),
        "heat" => heat(),
        "master3c" => master3c(),
        "gradient" => gradient(),
        "rvc" => rvc(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(40)),
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
        "rgbshow" => rgbshow(
            args.get(2).map(String::as_str).unwrap_or("wave"),
            args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20),
        ),
        "slowcycle" => slowcycle(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1000)),
        "pulse" => pulse(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(180)),
        "encscan" => encscan(),
        "encscan2" => encscan2(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2)),
        "encrgb" => encrgb(
            args.get(2)
                .and_then(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0x30),
        ),
        "dialrgb" => dialrgb(args.get(2).map(String::as_str).unwrap_or("int")),
        "dialloop" => dialloop(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(150)),
        "dialsweep" => dialsweep(),
        "hidout" => hidout(),
        "hidloop" => hidloop(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(60)),
        "hidrgb" => hidrgb(),
        "hidrgbscan" => hidrgbscan(
            args.get(2)
                .and_then(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0xb6),
        ),
        "hidinit" => hidinit(),
        "hidclass" => hidclass(),
        "hidshow" => hidshow(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(40)),
        "lightshow" => lightshow(),
        "btime" => btime(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(45)),
        "clickprobe" => clickprobe(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(45)),
        "oscscan" => oscscan(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(45)),
        "rawprobe" => rawprobe(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(45)),
        "rgbmap" => rgbmap(
            args.get(2)
                .and_then(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0xb6),
            args.get(3)
                .and_then(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0x11),
            args.get(4).and_then(|s| s.parse().ok()).unwrap_or(900),
        ),
        "mrgb" => mrgb(),
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

/// Master-dial colour hunt WITH a live MCU connection held open.
///
/// `handshake` proved 0x72 (even bytes 9-16) lights only the 8 channel rings,
/// never the master dial. So the master ring wants a different message. This
/// completes the handshake and then, on the SAME still-open connection, sweeps
/// the remaining candidates the connection-less `mcolor2` couldn't test live:
///   A. ring CCs 0x38..0x3F (just above the 8 channel rings 0x30..0x37)
///   B. sibling vendor SysEx commands 0x70..0x7F (skip 0x72), 9 colour bytes
///
/// Unsafe command ranges (0x0A-0x0F go-offline/config, 0x61-0x63 resets) are
/// never touched. Nothing sent here persists — power-cycle clears everything.
fn mlive() -> R<()> {
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

    // ── Handshake, keep `conn` open for the whole sweep ──
    println!(">>> device query");
    conn.send(&[0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7])?;
    let reply = rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| "no reply to device query")?;
    if reply.len() < 18 || reply[5] != 0x01 {
        println!("unexpected reply: {}", hex_full(&reply));
        return Ok(());
    }
    let serial = &reply[6..13];
    let challenge = &reply[13..17];
    let resp = mcu_response(challenge);
    let mut hs = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x02];
    hs.extend(serial);
    hs.extend(resp);
    hs.push(0xF7);
    conn.send(&hs)?;
    match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(m) if m.len() > 5 && m[5] == 0x03 => println!("    ACCEPTED — connection live\n"),
        Ok(m) => println!("    reply: {}\n", hex_full(&m)),
        Err(_) => println!("    (no ACK — proceeding anyway)\n"),
    }

    // Baseline: channel rings off so any master change is unambiguous.
    for i in 0..8u8 {
        conn.send(&[0xB0, 0x30 + i, 0])?;
    }
    let mut strips_black = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
    strips_black.extend(vec![0u8; 8]);
    strips_black.push(0xF7);
    conn.send(&strips_black)?;
    sleep(Duration::from_millis(600));

    // ── Part A: ring CCs above the 8 channel rings ──
    println!("PART A — ring CCs 0x38..0x3F (watch ONLY the master dial)");
    for cc in 0x38..=0x3Fu8 {
        for v in [0x01u8, 0x0B, 0x2B, 0x41, 0x7F] {
            conn.send(&[0xB0, cc, v])?;
            sleep(Duration::from_millis(180));
        }
        println!("   CC 0x{cc:02X} swept 1/0x0B/0x2B/0x41/0x7F");
        sleep(Duration::from_millis(400));
        conn.send(&[0xB0, cc, 0])?;
    }

    // ── Part B: sibling vendor SysEx commands, 9 red bytes each ──
    println!("\nPART B — vendor SysEx cmds 0x70..0x7F (skip 0x72), 9 red bytes");
    for cmd in 0x70..=0x7Fu8 {
        if cmd == 0x72 {
            continue;
        }
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, cmd];
        m.extend(vec![1u8; 9]);
        m.push(0xF7);
        conn.send(&m)?;
        println!("   cmd 0x{cmd:02X} sent");
        sleep(Duration::from_millis(900));
    }

    // Leave the surface tidy.
    conn.send(&strips_black)?;
    for i in 0..8u8 {
        conn.send(&[0xB0, 0x30 + i, 0])?;
    }
    println!("\nDid the master dial light at any point?");
    println!("Part A: which CC value (report the 0x{{cc}} line)?");
    println!("Part B: which command byte?");
    println!("If nothing ever lit it, the master-dial RGB is almost certainly");
    println!("OSC-preset-only and not reachable in Mackie mode.");
    Ok(())
}

/// Live rotary -> value -> colour loop. Handshakes, then for `secs`: each of
/// the 8 encoders (relative CC 0x10..0x17) accumulates its own 0-100 value; on
/// every change the dial's ring shows the value as a dot (CC 0x30..0x37), its
/// colour steps a cool->warm palette ramp (0x72, fixed palette — no true
/// gradient), and its scribble strip shows the number. Each dial independent.
fn rvc(secs: u64) -> R<()> {
    // Encoder events (channel 0..7, signed ticks) from the input thread.
    let (tx, rx) = std::sync::mpsc::channel::<(u8, i32)>();
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
        mi.ignore(Ignore::ActiveSense);
        let port = mi
            .ports()
            .into_iter()
            .find(|p| mi.port_name(p).map(|n| &n == name).unwrap_or(false))
            .ok_or("port vanished")?;
        let t = tx.clone();
        conns.push(mi.connect(
            &port,
            "probe-in",
            move |_ts, msg, _| {
                // Relative V-Pot: 0xB0, cc 0x10..0x17, val<0x40 = +, else -(val-0x40).
                if msg.len() == 3 && msg[0] & 0xF0 == 0xB0 && (0x10..=0x17).contains(&msg[1]) {
                    let n = msg[1] - 0x10;
                    let v = msg[2];
                    let ticks = if v < 0x40 {
                        v as i32
                    } else {
                        -((v & 0x3F) as i32)
                    };
                    let _ = t.send((n, ticks));
                }
            },
            (),
        )?);
    }

    let out_name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&out_name)?;
    // Handshake so the surface streams + accepts feedback.
    let (htx, hrx) = std::sync::mpsc::channel::<Vec<u8>>();
    // Re-open one input purely to read the handshake reply.
    let mut mi = MidiInput::new("probe-hs")?;
    mi.ignore(Ignore::None);
    let hs_port = mi
        .ports()
        .into_iter()
        .find(|p| {
            mi.port_name(p)
                .map(|n| n.to_lowercase().contains(MATCH))
                .unwrap_or(false)
        })
        .ok_or("no D700 input")?;
    let _hs_conn = mi.connect(
        &hs_port,
        "hs",
        move |_t, m, _| {
            if m.first() == Some(&0xF0) {
                let _ = htx.send(m.to_vec());
            }
        },
        (),
    )?;
    conn.send(&[0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7])?;
    if let Ok(reply) = hrx.recv_timeout(Duration::from_secs(3)) {
        if reply.len() >= 18 && reply[5] == 0x01 {
            let serial = reply[6..13].to_vec();
            let resp = mcu_response(&reply[13..17]);
            let mut hs = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x02];
            hs.extend(&serial);
            hs.extend(resp);
            hs.push(0xF7);
            conn.send(&hs)?;
        }
    }

    // Cool -> warm ramp over the fixed palette (violet-ish low, red high).
    let colour_for = |v: f32| -> u8 {
        match v as u32 {
            0..=16 => 5,  // magenta / violet
            17..=33 => 4, // blue
            34..=50 => 6, // cyan
            51..=66 => 2, // green
            67..=83 => 3, // yellow
            _ => 1,       // red
        }
    };

    let mut values = [50.0f32; 8];
    let mut colours = [0u8; 8];

    // Static top row: "Dial N". Bottom row shows live values.
    let mut row1 = Vec::new();
    for i in 0..8 {
        row1.extend(pad7(&format!("Dial {}", i + 1)));
    }
    let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, 0x00];
    m.extend(row1);
    m.push(0xF7);
    conn.send(&m)?;

    // Paint initial state for all 8.
    for n in 0..8u8 {
        paint_dial(
            &mut conn,
            n,
            values[n as usize],
            colour_for(values[n as usize]),
        )?;
        colours[n as usize] = colour_for(values[n as usize]);
    }
    push_colours(&mut conn, &colours)?;

    println!("--- turn the 8 rotaries for {secs}s — each dial's colour tracks its value ---");
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(60)) {
            Ok((n, ticks)) => {
                let i = n as usize;
                values[i] = (values[i] + ticks as f32 * 3.0).clamp(0.0, 100.0);
                let c = colour_for(values[i]);
                let recolour = c != colours[i];
                colours[i] = c;
                paint_dial(&mut conn, n, values[i], c)?;
                if recolour {
                    push_colours(&mut conn, &colours)?;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }

    // Tidy: rings off, strips black.
    for n in 0..8u8 {
        conn.send(&[0xB0, 0x30 + n, 0])?;
    }
    push_colours(&mut conn, &[0u8; 8])?;
    println!("--- done ---");
    Ok(())
}

/// Show one dial's value: ring dot position (CC 0x30+n, MCU single-dot mode)
/// and the number on its scribble strip (row 2, offset 0x38 + n*7).
fn paint_dial(conn: &mut midir::MidiOutputConnection, n: u8, value: f32, _c: u8) -> R<()> {
    let pos = 1 + (value / 100.0 * 10.0).round() as u8; // 1..=11
    conn.send(&[0xB0, 0x30 + n, pos.min(11)])?;
    let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, 0x38 + n * 7];
    m.extend(pad7(&format!("{}%", value.round() as u32)));
    m.push(0xF7);
    conn.send(&m)?;
    Ok(())
}

/// Push all 8 ring colours in one 0x72 command.
fn push_colours(conn: &mut midir::MidiOutputConnection, colours: &[u8; 8]) -> R<()> {
    let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
    m.extend(colours.iter().copied());
    m.push(0xF7);
    conn.send(&m)?;
    Ok(())
}

/// Is the ring palette rich enough for a smooth gradient? We've only used
/// indices 1/4/5 (red/blue/magenta). This handshakes then walks the FULL 0x72
/// index range across the 8 dials in blocks, labelling each with its index, so
/// we can see whether values above 7 are new hues (=> gradient possible) or
/// just repeats of the basic 8 (=> gradient needs the OSC preset).
fn gradient() -> R<()> {
    // Handshake (colour behaved best connected).
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
        conns.push(mi.connect(
            &port,
            "probe-in",
            move |_ts, msg, _| {
                if msg.first() == Some(&0xF0) {
                    let _ = t.send(msg.to_vec());
                }
            },
            (),
        )?);
    }
    let out_name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&out_name)?;
    conn.send(&[0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7])?;
    if let Ok(reply) = rx.recv_timeout(Duration::from_secs(3)) {
        if reply.len() >= 18 && reply[5] == 0x01 {
            let serial = reply[6..13].to_vec();
            let resp = mcu_response(&reply[13..17]);
            let mut hs = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x02];
            hs.extend(&serial);
            hs.extend(resp);
            hs.push(0xF7);
            conn.send(&hs)?;
        }
    }
    println!("connection live\n");

    // Blocks of 8 consecutive indices, so adjacent dials reveal any smooth step.
    let blocks: [[u8; 8]; 4] = [
        [0, 1, 2, 3, 4, 5, 6, 7],
        [8, 9, 10, 11, 12, 13, 14, 15],
        [16, 20, 24, 28, 32, 40, 48, 56],
        [64, 72, 80, 90, 100, 110, 120, 127],
    ];

    for (bi, block) in blocks.iter().enumerate() {
        // Label each dial with its index value.
        let mut row = Vec::new();
        for idx in block {
            row.extend(pad7(&format!("i{idx}")));
        }
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, 0x00];
        m.extend(row);
        m.push(0xF7);
        conn.send(&m)?;

        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
        m.extend(block.iter().copied());
        m.push(0xF7);
        conn.send(&m)?;

        println!(
            ">>> BLOCK {} — dials show indices {:?} (hold 6s)",
            bi + 1,
            block
        );
        sleep(Duration::from_secs(6));
    }

    // Tidy.
    let mut black = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
    black.extend(vec![0u8; 8]);
    black.push(0xF7);
    conn.send(&black)?;

    println!("\nAcross the 4 blocks: how many DISTINCT colours did you see?");
    println!("If only ~8 (repeating), the palette is fixed -> no MIDI gradient.");
    println!("If indices kept producing NEW shades, a gradient IS possible.");
    Ok(())
}

/// Drive + monitor the master dial via CC 0x3C (its MIDI-mode Light/Position).
/// Tries raw first (MIDI mode is not MCU, so no handshake needed), then a
/// handshaked pass as fallback, then a short window to catch the dial's own
/// CC 0x3C when turned. A single 0-127 value = brightness/position, not hue.
fn master3c() -> R<()> {
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
                // Only surface CC traffic (the master dial's own 0x3C).
                if msg.first().map(|b| b & 0xF0) == Some(0xB0) {
                    println!("   <<< {tag} {} {}", hex(msg), decode(msg));
                }
            },
            (),
        )?);
    }

    let out_name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&out_name)?;

    let sweep = |conn: &mut midir::MidiOutputConnection| -> R<()> {
        for v in [0u8, 20, 40, 64, 90, 110, 127, 90, 64, 20, 0] {
            conn.send(&[0xB0, 0x3C, v])?;
            println!("   CC 0x3C = {v}");
            sleep(Duration::from_millis(450));
        }
        Ok(())
    };

    println!("PHASE 1 — RAW (no handshake): sweep CC 0x3C 0..127..0");
    sweep(&mut conn)?;

    println!("\nPHASE 2 — after MCU handshake, same sweep");
    conn.send(&[0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7])?;
    sleep(Duration::from_millis(400));
    // Best-effort handshake reply is handled by the surface; we can't read it
    // here (the input callback only prints CC), so just answer blind is skipped
    // — many MIDI-mode configs don't gate. Re-sweep regardless.
    sweep(&mut conn)?;

    println!("\nPHASE 3 — turn the MASTER DIAL now (watching for CC 0x3C in) ~8s");
    sleep(Duration::from_secs(8));

    conn.send(&[0xB0, 0x3C, 0])?;
    println!("\nDid the master dial light / move during phase 1 or 2?");
    println!("And did turning it print a '<<< ... CC ... 0x3C' line in phase 3?");
    Ok(())
}

/// Value-as-colour demo across the 8 channel dials: each gets a temp value and
/// a colour on a **dark-violet (low) -> red (high)** ramp, with the value shown
/// on its scribble strip. Fires TWO colour encodings so we learn which the D700
/// honours, watching the result decides the format:
///   1. RGB triplets — 0x72 + 24 bytes (3 per dial). A smooth ramp = RGB works.
///   2. indexed palette — 0x72 + 8 bytes (one index per dial), best-guess
///      violet->red using the X-Touch palette (5 magenta .. 1 red).
///
/// 7-bit clamps applied (SysEx data must be 0-127).
fn heat() -> R<()> {
    // Handshake so the surface is live (colour has behaved better connected).
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
        conns.push(mi.connect(
            &port,
            "probe-in",
            move |_ts, msg, _| {
                if msg.first() == Some(&0xF0) {
                    let _ = t.send(msg.to_vec());
                }
            },
            (),
        )?);
    }
    let out_name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&out_name)?;
    conn.send(&[0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7])?;
    if let Ok(reply) = rx.recv_timeout(Duration::from_secs(3)) {
        if reply.len() >= 18 && reply[5] == 0x01 {
            let serial = reply[6..13].to_vec();
            let resp = mcu_response(&reply[13..17]);
            let mut hs = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x02];
            hs.extend(&serial);
            hs.extend(resp);
            hs.push(0xF7);
            conn.send(&hs)?;
        }
    }
    println!("connection live\n");

    // Eight temp "values" (percent), ascending so the ramp is obvious.
    let vals: [u8; 8] = [8, 20, 33, 46, 58, 71, 84, 97];

    // Label each strip: row 1 "Dial N", row 2 "NN%".
    let mut row1 = Vec::new();
    let mut row2 = Vec::new();
    for (i, v) in vals.iter().enumerate() {
        row1.extend(pad7(&format!("Dial {}", i + 1)));
        row2.extend(pad7(&format!("{v}%")));
    }
    for (off, data) in [(0x00u8, &row1), (0x38u8, &row2)] {
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, off];
        m.extend(data.iter().copied());
        m.push(0xF7);
        conn.send(&m)?;
    }

    // Map a 0..100 value to a 7-bit violet->red RGB triplet.
    // Dark violet ~ (30,0,50); red ~ (127,0,0). R rises, B falls, G stays 0.
    let rgb = |v: u8| -> (u8, u8, u8) {
        let t = (v as f32 / 100.0).clamp(0.0, 1.0);
        let r = (30.0 + t * 97.0).round() as u8;
        let b = (50.0 * (1.0 - t)).round() as u8;
        (r.min(127), 0, b.min(127))
    };

    println!("ATTEMPT 1 — RGB triplets: 0x72 + 24 bytes (3 per dial)");
    let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
    for v in vals {
        let (r, g, b) = rgb(v);
        m.extend([r, g, b]);
        println!("   dial {:>2}%  rgb=({r},{g},{b})", v);
    }
    m.push(0xF7);
    conn.send(&m)?;
    println!("   -> smooth violet->red ramp across the 8 dials? (RGB works)");
    sleep(Duration::from_secs(6));

    println!("\nATTEMPT 2 — indexed palette: 0x72 + 8 bytes (violet->red buckets)");
    // X-Touch palette has no true gradient; approximate low->high as
    // magenta(5) -> blue(4) -> red(1) buckets so SOMETHING tracks value.
    let idx = |v: u8| -> u8 {
        match v {
            0..=39 => 5,  // magenta (violet-ish)
            40..=69 => 4, // blue
            _ => 1,       // red
        }
    };
    let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
    for v in vals {
        let c = idx(v);
        m.extend([c]);
        println!("   dial {:>2}%  index={c}", v);
    }
    m.push(0xF7);
    conn.send(&m)?;
    println!("   -> 8 dials in violet/blue/red buckets? (indexed palette)");
    sleep(Duration::from_secs(6));

    println!("\nWhich attempt tracked the values, and did the colours read as a");
    println!("violet->red progression? That tells us the D700's colour format.");
    Ok(())
}

/// Live master-dial FEEDBACK attempt. The dial reports as MCU channel 9
/// (pitch-bend ch9), so this handshakes and then, on the live connection:
///   A. echoes pitch-bend ch9 back across the range — does the ring show
///      POSITION (light up / move) the way a motor fader or ring would?
///   B. drives V-Pot-ring position on the "9th" ring CC (0x38) — some layouts
///      put the master ring just above the 8 channel rings.
///   C. one more 0x72 colour try with the 9th byte, cycling colours.
///
/// Watch the master dial for ANY change (position lights or colour).
fn mpos() -> R<()> {
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
        conns.push(mi.connect(
            &port,
            "probe-in",
            move |_ts, msg, _| {
                if msg.first() == Some(&0xF0) {
                    let _ = t.send(msg.to_vec());
                }
            },
            (),
        )?);
    }

    let out_name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&out_name)?;
    println!(">>> handshake");
    conn.send(&[0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7])?;
    if let Ok(reply) = rx.recv_timeout(Duration::from_secs(3)) {
        if reply.len() >= 18 && reply[5] == 0x01 {
            let serial = reply[6..13].to_vec();
            let resp = mcu_response(&reply[13..17]);
            let mut hs = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x02];
            hs.extend(&serial);
            hs.extend(resp);
            hs.push(0xF7);
            conn.send(&hs)?;
            println!("    connection live\n");
        }
    }

    println!("PART A — echo pitch-bend ch9 (master) position 0 -> full -> 0");
    for pos in [0u16, 2048, 4096, 8192, 12288, 16383, 8192, 0] {
        conn.send(&[0xE8, (pos & 0x7F) as u8, (pos >> 7) as u8])?;
        println!("   PB ch9 = {pos}");
        sleep(Duration::from_millis(700));
    }

    println!("\nPART B — V-Pot ring position on CC 0x38 (the '9th' ring)");
    for v in [0x01u8, 0x03, 0x06, 0x0B, 0x2B, 0x00] {
        conn.send(&[0xB0, 0x38, v])?;
        println!("   CC 0x38 = 0x{v:02X}");
        sleep(Duration::from_millis(600));
    }

    println!("\nPART C — 0x72 colour, 9th byte cycling colours (strips 1-8 black)");
    for (c, name) in [(1u8, "red"), (2, "green"), (4, "blue"), (7, "white")] {
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
        m.extend(vec![0u8; 8]);
        m.push(c);
        m.push(0xF7);
        conn.send(&m)?;
        println!("   0x72 9th byte = {c} ({name})");
        sleep(Duration::from_millis(1200));
    }

    // tidy
    conn.send(&[0xB0, 0x38, 0])?;
    let mut black = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
    black.extend(vec![0u8; 8]);
    black.push(0xF7);
    conn.send(&black)?;

    println!("\nDid the master dial do ANYTHING in A, B, or C?");
    Ok(())
}

/// Handshake, hold the connection live, then MONITOR input for `secs`.
///
/// The surface is handshake-gated, so a plain `mon` sees nothing. This brings
/// the connection up first, then prints decoded traffic — use it to confirm a
/// freshly-mapped control transmits (e.g. the master dial set to Jog should
/// emit CC 0x3C relative when turned).
fn monlive(secs: u64) -> R<()> {
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
        mi.ignore(Ignore::ActiveSense);
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
                    let _ = t.send(msg.to_vec());
                }
                println!("{:<16} {:<26} {}", tag, hex(msg), decode(msg));
            },
            (),
        )?);
    }

    let out_name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&out_name)?;
    println!(">>> handshake");
    conn.send(&[0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7])?;
    if let Ok(reply) = rx.recv_timeout(Duration::from_secs(3)) {
        if reply.len() >= 18 && reply[5] == 0x01 {
            let serial = reply[6..13].to_vec();
            let resp = mcu_response(&reply[13..17]);
            let mut hs = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x02];
            hs.extend(&serial);
            hs.extend(resp);
            hs.push(0xF7);
            conn.send(&hs)?;
            println!("    connection reply sent (surface should be live)");
        }
    } else {
        println!("    (no handshake reply — monitoring anyway)");
    }

    println!("\n--- turn the MASTER DIAL (and move faders/buttons) for {secs}s ---");
    println!("{:<16} {:<26} MEANING", "PORT", "RAW");
    sleep(Duration::from_secs(secs));
    println!("--- done ---");
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

/// The 8 SysEx colours ordered around the colour wheel rather than by value.
/// 1 red, 3 yellow, 2 green, 6 cyan, 4 blue, 5 magenta - each step is one
/// primary added or removed, so a sweep through them reads as a hue rotation
/// even though only six hues exist.
const HUES: [u8; 6] = [1, 3, 2, 6, 4, 5];

/// Per Asparion: set an encoder ring's colour by sending r/2, g/2, b/2 as the
/// SAME CC number on MIDI channels 1, 2 and 3. The ring refreshes only when the
/// blue (channel 3) message arrives, so order matters.
fn set_ring_rgb(conn: &mut midir::MidiOutputConnection, cc: u8, r: u8, g: u8, b: u8) -> R<()> {
    conn.send(&[0xB0, cc, r / 2])?; // channel 1 - red
    conn.send(&[0xB1, cc, g / 2])?; // channel 2 - green
    conn.send(&[0xB2, cc, b / 2])?; // channel 3 - blue, triggers refresh
    Ok(())
}

/// Animated colour across all 16 strips. Effects: wave, breathe, comet, fade.
fn rgbshow(effect: &str, secs: u64) -> R<()> {
    let ports = out_ports()?;
    let mut conns: Vec<_> = ports.iter().filter_map(|n| open_out(n).ok()).collect();
    if conns.is_empty() {
        return Err("no D700 output".into());
    }
    println!("effect '{effect}' on {} bank(s) for {secs}s", conns.len());

    let paint = |conns: &mut Vec<midir::MidiOutputConnection>, strips: &[u8; 16]| -> R<()> {
        for (bank, c) in conns.iter_mut().enumerate() {
            let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
            m.extend(&strips[bank * 8..bank * 8 + 8]);
            m.push(0xF7);
            c.send(&m)?;
        }
        Ok(())
    };

    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    let mut frame = 0usize;
    while std::time::Instant::now() < deadline {
        let mut strips = [0u8; 16];
        match effect {
            // Hue wave travelling left to right across all 16 strips.
            "wave" => {
                for (i, s) in strips.iter_mut().enumerate() {
                    *s = HUES[(i + frame) % HUES.len()];
                }
            }
            // All strips one hue, stepping round the wheel - a slow colour fade.
            "breathe" => {
                let c = HUES[(frame / 4) % HUES.len()];
                strips = [c; 16];
            }
            // A white head with a coloured tail, chasing round the surface.
            "comet" => {
                let head = frame % 16;
                for (i, s) in strips.iter_mut().enumerate() {
                    let d = (16 + head - i) % 16;
                    *s = match d {
                        0 => 7,
                        1 => HUES[frame % HUES.len()],
                        2 => HUES[(frame + 3) % HUES.len()],
                        _ => 0,
                    };
                }
            }
            // Split the surface into hue bands that drift - closest to a gradient.
            _ => {
                for (i, s) in strips.iter_mut().enumerate() {
                    let pos = (i * HUES.len()) / 16;
                    *s = HUES[(pos + frame / 3) % HUES.len()];
                }
            }
        }
        paint(&mut conns, &strips)?;
        frame += 1;
        sleep(Duration::from_millis(110));
    }

    // Leave the surface dark rather than mid-animation.
    paint(&mut conns, &[0u8; 16])?;
    println!("done ({frame} frames)");
    Ok(())
}

/// Step all 16 strips through the full 8-colour palette at a chosen interval,
/// naming each colour as it goes. Purpose: compare against the D700's own idle
/// animation. If they look the same, the device is cycling this same palette
/// and there is no finer colour resolution hiding anywhere.
fn slowcycle(ms: u64) -> R<()> {
    let names = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    let ports = out_ports()?;
    let mut conns: Vec<_> = ports.iter().filter_map(|n| open_out(n).ok()).collect();
    println!(
        "stepping the 8-colour palette every {ms}ms on {} bank(s)",
        conns.len()
    );
    println!("compare against the D700's own idle animation\n");
    for round in 0..2 {
        for (v, name) in names.iter().enumerate().skip(1) {
            for c in conns.iter_mut() {
                let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
                m.extend(vec![v as u8; 8]);
                m.push(0xF7);
                c.send(&m)?;
            }
            println!("  round {} : {v} {name}", round + 1);
            sleep(Duration::from_millis(ms));
        }
    }
    for c in conns.iter_mut() {
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
        m.extend(vec![7u8; 8]);
        m.push(0xF7);
        c.send(&m)?;
    }
    println!("\nleft on white. Does the D700's idle fade look like this, or smoother?");
    Ok(())
}

/// Minimal, unmistakable colour traffic for a USB capture: alternate all eight
/// strips RED / GREEN every 3 s. Exactly one SysEx per change, so the USB trace
/// shows a single isolated write rather than a burst to pick apart.
fn pulse(secs: u64) -> R<()> {
    let name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&name)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    let mut red = true;
    let mut n = 0u32;
    while std::time::Instant::now() < deadline {
        let c = if red { 1u8 } else { 2u8 };
        let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
        m.extend(vec![c; 8]);
        m.push(0xF7);
        conn.send(&m)?;
        n += 1;
        println!(
            "  [{n:3}] all strips -> {}  ({})",
            if red { "RED" } else { "GREEN" },
            hex_full(&m)
        );
        red = !red;
        sleep(Duration::from_secs(3));
    }
    println!("done: {n} colour writes");
    Ok(())
}

/// Which CC addresses encoder 1's ring? Asparion say "the midi code listed in
/// the configurator"; we do not have it, so try the two MCU candidates -
/// the V-pot input block (0x10-0x17) and the ring-LED block (0x30-0x37).
fn encscan() -> R<()> {
    let name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&name)?;
    println!("scanning CC candidates for encoder 1's ring colour");
    println!("each is set to VIVID RED for 2s, then off\n");
    for cc in [
        0x10u8, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36,
        0x37,
    ] {
        println!("  CC 0x{cc:02X} ({cc}) -> red");
        set_ring_rgb(&mut conn, cc, 255, 0, 0)?;
        sleep(Duration::from_millis(2000));
        set_ring_rgb(&mut conn, cc, 0, 0, 0)?;
        sleep(Duration::from_millis(300));
    }
    println!("\nWhich CC lit a ring, and which ring was it?");
    Ok(())
}

/// Second attempt at the ring-RGB CC, addressing two problems with the first:
///
/// 1. The surface was left WHITE by an earlier SysEx 0x72 write, which may mask
///    or override a per-ring colour. Blank everything to black first.
/// 2. Asparion's "on midi channel 1 2 3 resp. 2 3 4" is ambiguous. The first
///    scan used channels 1/2/3; `base` lets us try 2/3/4.
///
/// Also sends the documented enable (`value 1` on the first channel) before the
/// colour triple, in case a ring must be switched on to show anything.
fn encscan2(base: u8) -> R<()> {
    let name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&name)?;

    // Blank the SysEx colour layer so nothing masks a per-ring change.
    for port in out_ports()? {
        if let Ok(mut c) = open_out(&port) {
            let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
            m.extend(vec![0u8; 8]);
            m.push(0xF7);
            let _ = c.send(&m);
        }
    }
    println!("blanked SysEx colour layer to black");
    sleep(Duration::from_millis(600));

    let s0 = 0xB0 | (base - 1);
    let s1 = 0xB0 | base;
    let s2 = 0xB0 | (base + 1);
    println!(
        "scanning with MIDI channels {}/{}/{}  (status {s0:02X} {s1:02X} {s2:02X})",
        base,
        base + 1,
        base + 2
    );
    println!("each CC set to VIVID GREEN for 2s\n");

    for cc in [
        0x10u8, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36,
        0x37,
    ] {
        println!("  CC 0x{cc:02X} ({cc}) -> enable + green");
        conn.send(&[s0, cc, 1])?; // documented on/off: 1 = on
        sleep(Duration::from_millis(120));
        conn.send(&[s0, cc, 0])?; // r = 0
        conn.send(&[s1, cc, 127])?; // g = 254/2
        conn.send(&[s2, cc, 0])?; // b = 0, triggers refresh
        sleep(Duration::from_millis(1900));
        conn.send(&[s0, cc, 0])?;
        conn.send(&[s1, cc, 0])?;
        conn.send(&[s2, cc, 0])?;
        sleep(Duration::from_millis(250));
    }
    println!("\nAnything green? Which CC, and which ring?");
    Ok(())
}

/// Full-RGB demonstration on one ring: a proper hue sweep, impossible with the
/// 8-colour SysEx path.
fn encrgb(cc: u8) -> R<()> {
    let name = out_ports()?.into_iter().next().ok_or("no output")?;
    let mut conn = open_out(&name)?;
    println!("full-RGB sweep on CC 0x{cc:02X}\n");

    for (r, g, b, n) in [
        (255u8, 0u8, 0u8, "red"),
        (255, 128, 0, "orange"),
        (255, 255, 0, "yellow"),
        (0, 255, 0, "green"),
        (0, 255, 255, "cyan"),
        (0, 0, 255, "blue"),
        (128, 0, 255, "violet"),
        (255, 255, 255, "white"),
    ] {
        println!("  {n:8} rgb({r},{g},{b})");
        set_ring_rgb(&mut conn, cc, r, g, b)?;
        sleep(Duration::from_millis(1200));
    }

    println!("\n  smooth hue rotation - 128 steps");
    for i in 0..128u32 {
        let h = (i as f32) / 128.0 * 6.0;
        let x = (255.0 * (1.0 - (h % 2.0 - 1.0).abs())) as u8;
        let (r, g, b) = match h as u32 {
            0 => (255, x, 0),
            1 => (x, 255, 0),
            2 => (0, 255, x),
            3 => (0, x, 255),
            4 => (x, 0, 255),
            _ => (255, 0, x),
        };
        set_ring_rgb(&mut conn, cc, r, g, b)?;
        sleep(Duration::from_millis(45));
    }
    set_ring_rgb(&mut conn, cc, 0, 0, 0)?;
    println!("done");
    Ok(())
}

/// Drive operator-authored OSC addresses for dial RGB, so a USB capture can
/// show what the Connector emits to the device. Deliberately uses one primary
/// per dial and the value 254 (-> 127 after halving), which is distinctive
/// enough to spot by eye in a byte stream.
///
///   /dial/1/rgb  bank 1 dial 1   -> RED
///   /dial/9/rgb  bank 2 dial 1   -> GREEN
///   /dial/0/rgb  master dial     -> BLUE
fn dialrgb(fmt: &str) -> R<()> {
    use rosc::OscType::{Float, Int};
    use std::net::UdpSocket;
    let sock = UdpSocket::bind("0.0.0.0:0")?;

    let cases: [(&str, u8, u8, u8, &str); 3] = [
        ("/dial/1/rgb", 254, 0, 0, "RED   (bank 1, dial 1)"),
        ("/dial/9/rgb", 0, 254, 0, "GREEN (bank 2, dial 1)"),
        ("/dial/0/rgb", 0, 0, 254, "BLUE  (master dial)"),
    ];

    println!("sending to 127.0.0.1:7000 as '{fmt}'\n");
    for (path, r, g, b, label) in cases {
        let args = match fmt {
            "float" => vec![
                Float(r as f32 / 255.0),
                Float(g as f32 / 255.0),
                Float(b as f32 / 255.0),
            ],
            "half" => vec![Int(r as i32 / 2), Int(g as i32 / 2), Int(b as i32 / 2)],
            _ => vec![Int(r as i32), Int(g as i32), Int(b as i32)],
        };
        let msg = rosc::OscPacket::Message(rosc::OscMessage {
            addr: path.to_string(),
            args: args.clone(),
        });
        sock.send_to(&rosc::encoder::encode(&msg)?, "127.0.0.1:7000")?;
        println!("  {path:<16} {label}   {args:?}");
        sleep(Duration::from_millis(2500));
    }

    println!("\nthen black, to mark the end of the sequence in the trace");
    for (path, _, _, _, _) in cases {
        let args = match fmt {
            "float" => vec![Float(0.0), Float(0.0), Float(0.0)],
            _ => vec![Int(0), Int(0), Int(0)],
        };
        let msg = rosc::OscPacket::Message(rosc::OscMessage {
            addr: path.to_string(),
            args,
        });
        sock.send_to(&rosc::encoder::encode(&msg)?, "127.0.0.1:7000")?;
        sleep(Duration::from_millis(700));
    }
    println!("done");
    Ok(())
}

/// Float 0.0-1.0 RGB on the operator's OSC dial addresses, looping so a USB
/// capture can be started at any moment and still catch a full cycle.
/// One primary per dial keeps each command unmistakable in the trace.
fn dialloop(secs: u64) -> R<()> {
    use rosc::OscType::Float;
    use std::net::UdpSocket;
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    let send = |path: &str, r: f32, g: f32, b: f32| -> R<()> {
        let msg = rosc::OscPacket::Message(rosc::OscMessage {
            addr: path.to_string(),
            args: vec![Float(r), Float(g), Float(b)],
        });
        sock.send_to(&rosc::encoder::encode(&msg)?, "127.0.0.1:7000")?;
        Ok(())
    };

    println!("float 0.0-1.0 RGB, looping for {secs}s");
    println!("  /dial/1/rgb  bank 1 dial 1");
    println!("  /dial/9/rgb  bank 2 dial 1");
    println!("  /dial/0/rgb  master dial\n");

    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    let mut n = 0u32;
    while std::time::Instant::now() < deadline {
        for (path, r, g, b, label) in [
            ("/dial/1/rgb", 1.0f32, 0.0, 0.0, "dial 1  RED"),
            ("/dial/9/rgb", 0.0, 1.0, 0.0, "dial 9  GREEN"),
            ("/dial/0/rgb", 0.0, 0.0, 1.0, "master  BLUE"),
        ] {
            send(path, r, g, b)?;
            println!("  [{n:3}] {label}");
            n += 1;
            sleep(Duration::from_millis(2200));
            if std::time::Instant::now() >= deadline {
                break;
            }
        }
        // All off, marking the cycle boundary in the trace.
        for path in ["/dial/1/rgb", "/dial/9/rgb", "/dial/0/rgb"] {
            send(path, 0.0, 0.0, 0.0)?;
        }
        sleep(Duration::from_millis(1200));
    }
    for path in ["/dial/1/rgb", "/dial/9/rgb", "/dial/0/rgb"] {
        send(path, 0.0, 0.0, 0.0)?;
    }
    println!("done: {n} colour messages");
    Ok(())
}

/// Is the dial RGB genuinely graded, or quantised to a few steps?
///
/// Three sweeps, each fine enough that banding would be obvious:
///   1. brightness  - black to full red in 128 steps, then back
///   2. hue         - full colour wheel in 180 steps
///   3. white level - black to white in 128 steps (all three channels together)
///
/// Driven on all three dials at once so bank 1, bank 2 and master can be
/// compared for identical behaviour.
fn dialsweep() -> R<()> {
    use rosc::OscType::Float;
    use std::net::UdpSocket;
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    let paint = |r: f32, g: f32, b: f32| -> R<()> {
        for path in ["/dial/1/rgb", "/dial/9/rgb", "/dial/0/rgb"] {
            let msg = rosc::OscPacket::Message(rosc::OscMessage {
                addr: path.to_string(),
                args: vec![Float(r), Float(g), Float(b)],
            });
            sock.send_to(&rosc::encoder::encode(&msg)?, "127.0.0.1:7000")?;
        }
        Ok(())
    };

    println!("1/3  BRIGHTNESS: black -> red -> black, 128 steps each way (~13s)");
    println!("     watch for banding, or a smooth ramp");
    for i in 0..=127 {
        paint(i as f32 / 127.0, 0.0, 0.0)?;
        sleep(Duration::from_millis(50));
    }
    for i in (0..=127).rev() {
        paint(i as f32 / 127.0, 0.0, 0.0)?;
        sleep(Duration::from_millis(50));
    }

    println!("2/3  HUE: full colour wheel, 180 steps (~14s)");
    for i in 0..180 {
        let h = (i as f32) / 180.0 * 6.0;
        let x = 1.0 - (h % 2.0 - 1.0).abs();
        let (r, g, b) = match h as u32 {
            0 => (1.0, x, 0.0),
            1 => (x, 1.0, 0.0),
            2 => (0.0, 1.0, x),
            3 => (0.0, x, 1.0),
            4 => (x, 0.0, 1.0),
            _ => (1.0, 0.0, x),
        };
        paint(r, g, b)?;
        sleep(Duration::from_millis(80));
    }

    println!("3/3  WHITE LEVEL: black -> white, 128 steps (~6s)");
    for i in 0..=127 {
        let v = i as f32 / 127.0;
        paint(v, v, v)?;
        sleep(Duration::from_millis(50));
    }
    paint(0.0, 0.0, 0.0)?;
    println!("\ndone. Smooth throughout, or visible steps?");
    Ok(())
}

/// Does HID report 0x04 work OUTBOUND?
///
/// Input reports are `04 <port> <3 MIDI-style bytes>`, so the symmetric guess is
/// that writing the same shape drives the surface. Report 0x04 is the realtime
/// channel - structurally separate from the 0x20 config pages - so this is not
/// a configuration write and carries none of that risk.
///
/// Deliberately uses mid-range fader positions only. Finding 23: full-travel
/// commands slam the end stops.
fn hidout() -> R<()> {
    let api = hidapi::HidApi::new()?;
    const VID: u16 = 0x04D8;
    const PID: u16 = 0xE44E;

    println!("HID interfaces for {VID:04X}:{PID:04X}:");
    let mut path = None;
    for d in api.device_list() {
        if d.vendor_id() == VID && d.product_id() == PID {
            println!(
                "  iface {:>2}  usage_page={:#06x} usage={:#06x}  path={:?}",
                d.interface_number(),
                d.usage_page(),
                d.usage(),
                d.path()
            );
            if path.is_none() {
                path = Some(d.path().to_owned());
            }
        }
    }
    let path = path.ok_or("no D700 HID interface found")?;

    let dev = match api.open_path(&path) {
        Ok(d) => d,
        Err(e) => {
            println!("\nopen failed: {e}");
            println!("If this says access/exclusive, the Configurator holds the interface.");
            println!("Close the Asparion Configurator and retry.");
            return Ok(());
        }
    };
    println!("\nopened. writing report 0x04 -> fader 1, mid-range positions only");
    println!("(no end stops - see finding 23)\n");

    // 0x1000 = 25%, 0x2000 = 50%, 0x3000 = 75% of 14-bit travel.
    for (val, label) in [
        (0x1000u16, "25%"),
        (0x2000, "50%"),
        (0x3000, "75%"),
        (0x2000, "50%"),
    ] {
        let lsb = (val & 0x7F) as u8;
        let msb = (val >> 7) as u8;
        let report = [0x04u8, 0x00, 0xE0, lsb, msb];
        match dev.write(&report) {
            Ok(n) => println!(
                "  wrote {n} bytes: {}   (fader 1 -> {label})",
                report
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            Err(e) => {
                println!("  write failed: {e}");
                return Ok(());
            }
        }
        sleep(Duration::from_millis(1500));
    }

    println!("\nDid fader 1 move to 25%, 50%, 75%, then back to 50%?");
    Ok(())
}

/// Emit report 0x04 fader writes on a loop so a USB capture can catch them.
/// The point is to see what OUR write looks like on the wire next to the
/// Configurator's 5-byte frames - is it padded to 33, or something else?
fn hidloop(secs: u64) -> R<()> {
    let api = hidapi::HidApi::new()?;
    let path = api
        .device_list()
        .find(|d| d.vendor_id() == 0x04D8 && d.product_id() == 0xE44E)
        .map(|d| d.path().to_owned())
        .ok_or("no D700 HID interface")?;
    let dev = api.open_path(&path)?;

    println!("writing 04 00 E0 <lsb> <msb> every 1.5s for {secs}s");
    println!("mid-range only, no end stops\n");
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    let mut n = 0u32;
    let vals = [0x1000u16, 0x2000, 0x3000, 0x2000];
    while std::time::Instant::now() < deadline {
        let v = vals[(n as usize) % vals.len()];
        let report = [0x04u8, 0x00, 0xE0, (v & 0x7F) as u8, (v >> 7) as u8];
        match dev.write(&report) {
            Ok(w) => println!(
                "  [{n:3}] hid_write returned {w:2}  for  {}",
                report
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            Err(e) => {
                println!("  write failed: {e}");
                break;
            }
        }
        n += 1;
        sleep(Duration::from_millis(1500));
    }
    println!("done: {n} writes");
    Ok(())
}

/// Drive dial RGB over HID directly, bypassing the Connector entirely.
///
/// Captured from the Connector while it served OSC colour messages:
///   08 2a 0a b6 <index> 00 <R> <G> <B>
/// Full 8-bit per channel - better than the halved 0-127 of the MIDI method
/// Asparion described, and it needs no CC number.
fn hidrgb() -> R<()> {
    let api = hidapi::HidApi::new()?;
    let path = api
        .device_list()
        .find(|d| d.vendor_id() == 0x04D8 && d.product_id() == 0xE44E)
        .map(|d| d.path().to_owned())
        .ok_or("no D700 HID interface")?;
    let dev = api.open_path(&path)?;

    let set = |idx: u8, r: u8, g: u8, b: u8| -> R<()> {
        let msg = [0x08u8, 0x2a, 0x0a, 0xb6, idx, 0x00, r, g, b];
        dev.write(&msg)?;
        Ok(())
    };

    println!("writing 08 2a 0a b6 <idx> 00 <R> <G> <B> over HID\n");
    println!("1/2  named colours on indices 00, 01, 02");
    for (r, g, b, n) in [
        (255u8, 0u8, 0u8, "red"),
        (0, 255, 0, "green"),
        (0, 0, 255, "blue"),
        (255, 200, 0, "amber"),
        (255, 255, 255, "white"),
    ] {
        for idx in 0..3u8 {
            set(idx, r, g, b)?;
        }
        println!("   {n}");
        sleep(Duration::from_millis(1400));
    }

    println!("\n2/2  smooth hue rotation, 180 steps - full 8-bit");
    for i in 0..180u32 {
        let h = (i as f32) / 180.0 * 6.0;
        let x = (255.0 * (1.0 - (h % 2.0 - 1.0).abs())) as u8;
        let (r, g, b) = match h as u32 {
            0 => (255, x, 0),
            1 => (x, 255, 0),
            2 => (0, 255, x),
            3 => (0, x, 255),
            4 => (x, 0, 255),
            _ => (255, 0, x),
        };
        for idx in 0..3u8 {
            set(idx, r, g, b)?;
        }
        sleep(Duration::from_millis(70));
    }
    for idx in 0..3u8 {
        set(idx, 0, 0, 0)?;
    }
    println!("\ndone - did the dials colour WITHOUT the Connector in the path?");
    Ok(())
}

/// Which element does each index address? One at a time, slowly, so the device
/// is never asked to keep up and each result is unambiguous.
///
/// `type_byte` is the constant we saw as 0xb6; it may select the element class
/// (strip surround / master / something else), so it is parameterised.
fn hidrgbscan(type_byte: u8) -> R<()> {
    let api = hidapi::HidApi::new()?;
    let path = api
        .device_list()
        .find(|d| d.vendor_id() == 0x04D8 && d.product_id() == 0xE44E)
        .map(|d| d.path().to_owned())
        .ok_or("no D700 HID interface")?;
    let dev = api.open_path(&path)?;

    let set = |idx: u8, r: u8, g: u8, b: u8| -> R<()> {
        dev.write(&[0x08u8, 0x2a, 0x0a, type_byte, idx, 0x00, r, g, b])?;
        Ok(())
    };

    println!("type byte 0x{type_byte:02X}, indices 0x00..0x13, one at a time");
    println!("each set BRIGHT RED for 1.5s, then black\n");
    for idx in 0x00..=0x13u8 {
        print!("  idx 0x{idx:02X} ({idx:2}) -> red ... ");
        use std::io::Write as _;
        std::io::stdout().flush().ok();
        set(idx, 255, 0, 0)?;
        sleep(Duration::from_millis(1500));
        set(idx, 0, 0, 0)?;
        sleep(Duration::from_millis(400));
        println!("off");
    }
    println!("\nWhich indices lit something, and which element was it?");
    Ok(())
}

/// Replay the Connector's session-open handshake, then try colour.
///
/// Captured from the Connector's own startup:
///   OUT  08 2a 2b 29 2c 28 ...   session open
///   IN   08 2a 2b 00 ...         device acknowledges
///
/// Byte 4 of the 0x0a colour command is an element-class selector, not a
/// constant - the trace shows a2, b0, b2 and b6 - so all four are tried.
fn hidinit() -> R<()> {
    let api = hidapi::HidApi::new()?;
    let path = api
        .device_list()
        .find(|d| d.vendor_id() == 0x04D8 && d.product_id() == 0xE44E)
        .map(|d| d.path().to_owned())
        .ok_or("no D700 HID interface")?;
    let dev = api.open_path(&path)?;
    dev.set_blocking_mode(false).ok();

    let mut rx = [0u8; 64];
    let mut send = |label: &str, m: &[u8]| -> R<()> {
        dev.write(m)?;
        println!(
            "  OUT {label:<14} {}",
            m.iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        sleep(Duration::from_millis(220));
        while let Ok(n) = dev.read_timeout(&mut rx, 60) {
            if n == 0 {
                break;
            }
            println!(
                "  IN                 {}",
                rx[..n]
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        Ok(())
    };

    println!("=== session open ===");
    send(
        "open",
        &[0x08, 0x2a, 0x2b, 0x29, 0x2c, 0x28, 0x00, 0x00, 0x00],
    )?;
    send(
        "status",
        &[0x08, 0x2a, 0x2d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    )?;

    println!("\n=== colour, trying each element class ===");
    for class in [0xb6u8, 0xb2, 0xb0, 0xa2] {
        println!("\n  class 0x{class:02X} - indices 0..3 set RED for 2s");
        for idx in 0..4u8 {
            dev.write(&[0x08, 0x2a, 0x0a, class, idx, 0x00, 0xFF, 0x00, 0x00])?;
        }
        sleep(Duration::from_millis(2000));
        for idx in 0..4u8 {
            dev.write(&[0x08, 0x2a, 0x0a, class, idx, 0x00, 0x00, 0x00, 0x00])?;
        }
        sleep(Duration::from_millis(500));
    }
    println!("\nDid anything light red? Which element class?");
    Ok(())
}

/// Which element class drives the dial RGB? Handshake first, then b6 and b2
/// held long enough to be unmistakable, in distinct colours.
///
/// 0xb0 is skipped deliberately: it returned "device not functioning" and this
/// hardware has stalled once and wedged once already.
fn hidclass() -> R<()> {
    let api = hidapi::HidApi::new()?;
    let path = api
        .device_list()
        .find(|d| d.vendor_id() == 0x04D8 && d.product_id() == 0xE44E)
        .map(|d| d.path().to_owned())
        .ok_or("no D700 HID interface")?;
    let dev = api.open_path(&path)?;

    // Session open, as captured from the Connector's startup.
    dev.write(&[0x08, 0x2a, 0x2b, 0x29, 0x2c, 0x28, 0x00, 0x00, 0x00])?;
    sleep(Duration::from_millis(300));
    println!("session opened\n");

    let paint = |class: u8, r: u8, g: u8, b: u8| -> R<()> {
        for idx in 0..3u8 {
            dev.write(&[0x08, 0x2a, 0x0a, class, idx, 0x00, r, g, b])?;
            sleep(Duration::from_millis(40));
        }
        Ok(())
    };

    for (class, (r, g, b), name) in [
        (0xb6u8, (255u8, 0u8, 0u8), "RED"),
        (0xb2u8, (0, 0, 255), "BLUE"),
    ] {
        println!("  class 0x{class:02X} -> {name}, holding 5s");
        paint(class, r, g, b)?;
        sleep(Duration::from_secs(5));
        paint(class, 0, 0, 0)?;
        sleep(Duration::from_millis(900));
    }

    println!("\nWhich class lit the dials - 0xB6 (red) or 0xB2 (blue), or both?");
    Ok(())
}

/// The finished article: smooth 8-bit RGB on the D700's dials, driven directly
/// over HID with no Asparion software running.
///
///   08 2a 2b 29 2c 28 00 00 00          open session
///   08 2a 0a b6 <idx> 00 <R> <G> <B>    set colour, 0-255 per channel
///
/// Element class 0xb6, indices 0/1/2 = dial 1, dial 9, master. Each element is
/// given a phase offset so the three chase rather than moving in lockstep.
/// Update rate is kept modest - rapid bursts have stalled this device before.
fn hidshow(secs: u64) -> R<()> {
    let api = hidapi::HidApi::new()?;
    let path = api
        .device_list()
        .find(|d| d.vendor_id() == 0x04D8 && d.product_id() == 0xE44E)
        .map(|d| d.path().to_owned())
        .ok_or("no D700 HID interface")?;
    let dev = api.open_path(&path)?;
    dev.write(&[0x08, 0x2a, 0x2b, 0x29, 0x2c, 0x28, 0x00, 0x00, 0x00])?;
    sleep(Duration::from_millis(300));
    println!("session opened - full 8-bit hue rotation for {secs}s\n");

    /// Hue in [0,6) to 8-bit RGB.
    fn hue(h: f32) -> (u8, u8, u8) {
        let x = (255.0 * (1.0 - (h % 2.0 - 1.0).abs())) as u8;
        match h as u32 {
            0 => (255, x, 0),
            1 => (x, 255, 0),
            2 => (0, 255, x),
            3 => (0, x, 255),
            4 => (x, 0, 255),
            _ => (255, 0, x),
        }
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    let mut step = 0u32;
    while std::time::Instant::now() < deadline {
        for idx in 0..3u8 {
            // 1/3 of the wheel between each element, so they chase.
            let h = ((step as f32) / 90.0 + (idx as f32) * 2.0) % 6.0;
            let (r, g, b) = hue(h);
            dev.write(&[0x08, 0x2a, 0x0a, 0xb6, idx, 0x00, r, g, b])?;
            sleep(Duration::from_millis(25));
        }
        step += 1;
    }
    for idx in 0..3u8 {
        dev.write(&[0x08, 0x2a, 0x0a, 0xb6, idx, 0x00, 0, 0, 0])?;
        sleep(Duration::from_millis(30));
    }
    println!("done - {step} steps, no Connector, no Configurator, no MIDI");
    Ok(())
}

/// A choreographed demonstration using everything mapped over two days:
/// 16 motor faders, button LEDs, encoder rings, strip colour, full 8-bit RGB
/// dials over HID, and both displays.
///
/// Two constraints from the field notes are respected throughout:
///   * finding 23 - never command full travel; faders stay inside 0x0600..0x3A00
///   * finding 33 - HID writes are paced ~25ms apart, or the device stalls
fn lightshow() -> R<()> {
    let ports = out_ports()?;
    let mut midi: Vec<_> = ports.iter().filter_map(|n| open_out(n).ok()).collect();
    if midi.is_empty() {
        return Err("no D700 MIDI output".into());
    }
    let hid = hidapi::HidApi::new().ok().and_then(|api| {
        let p = api
            .device_list()
            .find(|d| d.vendor_id() == 0x04D8 && d.product_id() == 0xE44E)
            .map(|d| d.path().to_owned())?;
        api.open_path(&p).ok()
    });
    if let Some(h) = &hid {
        h.write(&[0x08, 0x2a, 0x2b, 0x29, 0x2c, 0x28, 0x00, 0x00, 0x00])
            .ok();
        sleep(Duration::from_millis(250));
    }
    println!(
        "MIDI banks: {}   HID RGB: {}",
        midi.len(),
        if hid.is_some() { "yes" } else { "no" }
    );

    // ---- helpers -----------------------------------------------------------
    let text = |m: &mut Vec<midir::MidiOutputConnection>, bank: usize, row: u8, t: &str| {
        if let Some(c) = m.get_mut(bank) {
            let mut msg = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, row];
            let mut body: Vec<u8> = t.bytes().take(56).collect();
            while body.len() < 56 {
                body.push(b' ');
            }
            msg.extend(body);
            msg.push(0xF7);
            let _ = c.send(&msg);
        }
    };
    let colours = |m: &mut Vec<midir::MidiOutputConnection>, c16: &[u8; 16]| {
        for (b, conn) in m.iter_mut().enumerate() {
            let mut msg = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x72];
            msg.extend(&c16[b * 8..b * 8 + 8]);
            msg.push(0xF7);
            let _ = conn.send(&msg);
        }
    };
    let fader = |m: &mut Vec<midir::MidiOutputConnection>, n: usize, v: u16| {
        let (bank, ch) = (n / 8, (n % 8) as u8);
        if let Some(c) = m.get_mut(bank) {
            let _ = c.send(&[0xE0 | ch, (v & 0x7F) as u8, (v >> 7) as u8]);
        }
    };
    let led = |m: &mut Vec<midir::MidiOutputConnection>, n: usize, base: u8, on: bool| {
        let (bank, i) = (n / 8, (n % 8) as u8);
        if let Some(c) = m.get_mut(bank) {
            let _ = c.send(&[0x90, base + i, if on { 127 } else { 0 }]);
        }
    };
    let ring = |m: &mut Vec<midir::MidiOutputConnection>, n: usize, pos: u8| {
        let (bank, i) = (n / 8, (n % 8) as u8);
        if let Some(c) = m.get_mut(bank) {
            let _ = c.send(&[0xB0, 0x30 + i, pos]);
        }
    };
    let dial = |h: &Option<hidapi::HidDevice>, idx: u8, r: u8, g: u8, b: u8| {
        if let Some(d) = h {
            let _ = d.write(&[0x08, 0x2a, 0x0a, 0xb6, idx, 0x00, r, g, b]);
        }
    };
    fn hue(h: f32) -> (u8, u8, u8) {
        let x = (255.0 * (1.0 - (h % 2.0 - 1.0).abs())) as u8;
        match h as u32 {
            0 => (255, x, 0),
            1 => (x, 255, 0),
            2 => (0, 255, x),
            3 => (0, x, 255),
            4 => (x, 0, 255),
            _ => (255, 0, x),
        }
    }
    const LO: u16 = 0x0600;
    const HI: u16 = 0x3A00;

    // ---- I. curtain up -----------------------------------------------------
    println!("I.   curtain up");
    colours(&mut midi, &[0u8; 16]);
    for n in 0..16 {
        fader(&mut midi, n, LO);
    }
    text(&mut midi, 0, 0x00, "  S21    HiJack  ");
    text(&mut midi, 1, 0x00, "   D700    RGB   ");
    text(&mut midi, 0, 0x38, " sixteen faders  ");
    text(&mut midi, 1, 0x38, "  two  displays  ");
    sleep(Duration::from_secs(2));

    // ---- II. fader wave, LEDs and rings following --------------------------
    println!("II.  wave");
    for step in 0..150 {
        let t = step as f32 / 12.0;
        for n in 0..16 {
            let phase = t - (n as f32) * 0.4;
            let s = (phase.sin() + 1.0) / 2.0;
            fader(&mut midi, n, LO + ((HI - LO) as f32 * s) as u16);
            ring(&mut midi, n, 1 + (s * 10.0) as u8);
        }
        if step % 3 == 0 {
            let head = (step / 3) % 16;
            for n in 0..16 {
                led(&mut midi, n, 0x10, n == head);
            }
        }
        if step % 6 == 0 {
            let mut c = [0u8; 16];
            for (n, v) in c.iter_mut().enumerate() {
                *v = HUES[(n + step / 6) % HUES.len()];
            }
            colours(&mut midi, &c);
        }
        if step % 4 == 0 {
            for idx in 0..3u8 {
                let (r, g, b) = hue((t * 0.6 + idx as f32 * 2.0) % 6.0);
                dial(&hid, idx, r, g, b);
                sleep(Duration::from_millis(25));
            }
        }
        sleep(Duration::from_millis(40));
    }

    // ---- III. converge -----------------------------------------------------
    println!("III. converge");
    text(&mut midi, 0, 0x00, "  colour   chase ");
    text(&mut midi, 1, 0x00, "   8-bit   RGB   ");
    for step in 0..90 {
        for n in 0..16 {
            let d = ((n as i32) - 8).abs() as f32;
            let s = ((step as f32 / 8.0) - d * 0.5).sin().max(0.0);
            fader(&mut midi, n, LO + ((HI - LO) as f32 * s) as u16);
        }
        let head = step % 16;
        for n in 0..16 {
            led(&mut midi, n, 0x00, n == head);
            led(&mut midi, n, 0x18, n == (15 - head));
        }
        if step % 4 == 0 {
            for idx in 0..3u8 {
                let (r, g, b) = hue((step as f32 / 6.0 + idx as f32) % 6.0);
                dial(&hid, idx, r, g, b);
                sleep(Duration::from_millis(25));
            }
        }
        sleep(Duration::from_millis(55));
    }

    // ---- IV. finale --------------------------------------------------------
    println!("IV.  finale");
    text(&mut midi, 0, 0x00, "   thank    you  ");
    text(&mut midi, 1, 0x00, "  Asparion D700  ");
    text(&mut midi, 0, 0x38, "                 ");
    text(&mut midi, 1, 0x38, "                 ");
    for flash in 0..6 {
        let on = flash % 2 == 0;
        for n in 0..16 {
            for base in [0x00u8, 0x08, 0x10, 0x18] {
                led(&mut midi, n, base, on);
            }
            fader(&mut midi, n, if on { HI } else { LO });
        }
        colours(&mut midi, &[if on { 7 } else { 0 }; 16]);
        for idx in 0..3u8 {
            let v = if on { 255 } else { 0 };
            dial(&hid, idx, v, v, v);
            sleep(Duration::from_millis(25));
        }
        sleep(Duration::from_millis(320));
    }

    println!("V.   curtain down");
    for n in 0..16 {
        for base in [0x00u8, 0x08, 0x10, 0x18] {
            led(&mut midi, n, base, false);
        }
        ring(&mut midi, n, 0);
        fader(&mut midi, n, LO);
    }
    colours(&mut midi, &[0u8; 16]);
    for idx in 0..3u8 {
        dial(&hid, idx, 0, 0, 0);
        sleep(Duration::from_millis(25));
    }
    text(&mut midi, 0, 0x00, "                 ");
    text(&mut midi, 1, 0x00, "                 ");
    println!("\nfin.");
    Ok(())
}

/// Button timing: is a "double click" a hardware bounce, a firmware-generated
/// double-click event, or two deliberate presses?
///
/// The existing `mon` prints no timestamps, so a 20 ms contact bounce and a
/// second press two seconds later look identical in its output. This logs every
/// note event with a millisecond timestamp and the gap since the previous event
/// on the SAME note, then summarises the gaps.
///
/// Reading the result:
///   < 30 ms    contact bounce or firmware double-fire - a defect
///   80-400 ms  human double-click - deliberate, and possibly a device feature
///   > 500 ms   two separate presses
fn btime(secs: u64) -> R<()> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

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

    // note -> (last event instant, gaps observed)
    let seen: Arc<Mutex<HashMap<u8, (std::time::Instant, Vec<u128>)>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let t0 = std::time::Instant::now();
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
        let seen = Arc::clone(&seen);
        conns.push(mi.connect(
            &port,
            "probe-in",
            move |_ts, msg, _| {
                if msg.len() < 3 || (msg[0] & 0xF0) != 0x90 {
                    return;
                }
                let (note, vel) = (msg[1], msg[2]);
                let now = std::time::Instant::now();
                let ms = now.duration_since(t0).as_millis();
                let mut g = seen.lock().unwrap();
                let gap = g
                    .get(&note)
                    .map(|(prev, _)| now.duration_since(*prev).as_millis());
                let e = g.entry(note).or_insert((now, Vec::new()));
                if let Some(d) = gap {
                    e.1.push(d);
                    e.0 = now;
                }
                let kind = if vel > 0 { "DOWN" } else { "UP  " };
                match gap {
                    Some(d) => {
                        println!("  {ms:>7} ms  {tag:<18} note 0x{note:02X} {kind}  (+{d} ms)")
                    }
                    None => println!("  {ms:>7} ms  {tag:<18} note 0x{note:02X} {kind}"),
                }
            },
            (),
        )?);
    }

    println!("listening for {secs}s on {} port(s)", conns.len());
    println!(
        "press each button ONCE, deliberately, with a clear pause between
"
    );
    sleep(Duration::from_secs(secs));

    println!(
        "
--- gaps between consecutive events on the same note ---"
    );
    let g = seen.lock().unwrap();
    let mut notes: Vec<_> = g.iter().collect();
    notes.sort_by_key(|(n, _)| **n);
    let mut bounces = 0usize;
    for (note, (_, gaps)) in notes {
        if gaps.is_empty() {
            continue;
        }
        let short = gaps.iter().filter(|d| **d < 30).count();
        bounces += short;
        let list: Vec<String> = gaps.iter().map(|d| format!("{d}")).collect();
        println!("  note 0x{note:02X}: {} ms", list.join(", "));
    }
    println!(
        "
{bounces} gap(s) under 30 ms - those are bounces or firmware doubles."
    );
    println!("80-400 ms gaps are human double-clicks; over 500 ms, separate presses.");
    Ok(())
}

/// Double-click investigation: MIDI and OSC on ONE timeline, with timestamps.
///
/// The operator mapped `clickLED` and `dClick` per control in the Connector, so
/// the device evidently has a firmware notion of a double click - that is not a
/// Mackie concept. Two questions follow, and both need timing:
///
///   1. **Additive or suppressive?** If a double click emits the single event
///      AND a dClick, it is additive and costs no latency. If it emits only
///      dClick, the single was withheld while the device waited - which delays
///      every single click by the detection window.
///   2. **Where does the doubling live?** If MIDI shows two note-ons where HID
///      or OSC shows one click plus one dClick, the doubling is in the MCU
///      translation rather than the switch.
///
/// Listens to every D700 MIDI input and the Connector's likely Tx ports at once,
/// timestamps everything against a common origin, and prints the merged timeline
/// sorted at the end (live output from several threads interleaves badly).
fn clickprobe(secs: u64) -> R<()> {
    use std::net::UdpSocket;
    use std::sync::{Arc, Mutex};

    let t0 = std::time::Instant::now();
    let log: Arc<Mutex<Vec<(u128, String)>>> = Arc::new(Mutex::new(Vec::new()));

    // ---- MIDI inputs -------------------------------------------------------
    let probe = MidiInput::new("probe")?;
    let names: Vec<String> = probe
        .ports()
        .iter()
        .filter_map(|p| probe.port_name(p).ok())
        .filter(|n| n.to_lowercase().contains(MATCH))
        .collect();
    let mut conns = Vec::new();
    for name in &names {
        let mut mi = MidiInput::new("probe")?;
        mi.ignore(Ignore::ActiveSense);
        let port = mi
            .ports()
            .into_iter()
            .find(|p| mi.port_name(p).map(|n| &n == name).unwrap_or(false))
            .ok_or("port vanished")?;
        let tag = if name.to_lowercase().contains("midiin2") {
            "MIDI b2"
        } else {
            "MIDI b1"
        };
        let log = Arc::clone(&log);
        conns.push(mi.connect(
            &port,
            "probe-in",
            move |_ts, msg, _| {
                let ms = std::time::Instant::now().duration_since(t0).as_millis();
                let d = match msg.len() {
                    3 => match msg[0] & 0xF0 {
                        0x90 => format!(
                            "note 0x{:02X} {}",
                            msg[1],
                            if msg[2] > 0 { "DOWN" } else { "UP" }
                        ),
                        0xB0 => format!("CC 0x{:02X} = {}", msg[1], msg[2]),
                        0xE0 => format!(
                            "PB ch{} = {}",
                            (msg[0] & 0x0F) + 1,
                            ((msg[2] as u16) << 7) | msg[1] as u16
                        ),
                        _ => return,
                    },
                    _ => return,
                };
                log.lock().unwrap().push((ms, format!("{tag:<8} {d}")));
            },
            (),
        )?);
    }
    println!("MIDI inputs: {}", conns.len());

    // ---- OSC listeners -----------------------------------------------------
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    let mut handles = Vec::new();
    for port in [7001u16, 8000, 8001, 9000] {
        let Ok(sock) = UdpSocket::bind(("0.0.0.0", port)) else {
            continue;
        };
        sock.set_read_timeout(Some(Duration::from_millis(200))).ok();
        println!("OSC listening on {port}");
        let log = Arc::clone(&log);
        handles.push(std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while std::time::Instant::now() < deadline {
                if let Ok((n, _)) = sock.recv_from(&mut buf) {
                    let ms = std::time::Instant::now().duration_since(t0).as_millis();
                    if let Ok((_, rosc::OscPacket::Message(m))) =
                        rosc::decoder::decode_udp(&buf[..n])
                    {
                        let a: Vec<String> = m.args.iter().map(|x| format!("{x:?}")).collect();
                        log.lock()
                            .unwrap()
                            .push((ms, format!("OSC      {} [{}]", m.addr, a.join(", "))));
                    }
                }
            }
        }));
    }

    println!(
        "
capturing {secs}s - SINGLE-click a control, pause, then DOUBLE-click it"
    );
    println!(
        "do one control at a time, with clear gaps between
"
    );
    sleep(Duration::from_secs(secs));
    for h in handles {
        let _ = h.join();
    }

    // ---- merged timeline ---------------------------------------------------
    let mut v = log.lock().unwrap().clone();
    v.sort_by_key(|(t, _)| *t);
    println!("--- merged timeline ---");
    let mut prev: Option<u128> = None;
    for (ms, what) in &v {
        match prev {
            Some(p) => println!("  {ms:>7} ms  (+{:>5}) {what}", ms - p),
            None => println!("  {ms:>7} ms  (     ) {what}"),
        }
        prev = Some(*ms);
    }
    println!(
        "
{} events. Look for: does a double click produce TWO MIDI",
        v.len()
    );
    println!("note-ons, and does OSC show a click AND a dClick, or dClick alone?");
    Ok(())
}

/// Where is the Connector transmitting? Bind a wide range of UDP ports and
/// report anything that arrives, with timestamps and deltas.
///
/// Used when the Connector's Tx port is unknown or has moved: rather than
/// hunting through its UI, listen everywhere plausible at once.
fn oscscan(secs: u64) -> R<()> {
    use std::net::UdpSocket;
    use std::sync::{Arc, Mutex};

    let t0 = std::time::Instant::now();
    let log: Arc<Mutex<Vec<(u128, u16, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    let mut handles = Vec::new();
    let mut bound = Vec::new();

    let mut candidates: Vec<u16> = Vec::new();
    candidates.extend(7001..=7010);
    candidates.extend(8000..=8010);
    candidates.extend(9000..=9010);
    candidates.extend([10000u16, 10023, 53000, 3819, 8080]);

    for port in candidates {
        let Ok(sock) = UdpSocket::bind(("0.0.0.0", port)) else {
            continue;
        };
        sock.set_read_timeout(Some(Duration::from_millis(200))).ok();
        bound.push(port);
        let log = Arc::clone(&log);
        handles.push(std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while std::time::Instant::now() < deadline {
                if let Ok((n, _)) = sock.recv_from(&mut buf) {
                    let ms = std::time::Instant::now().duration_since(t0).as_millis();
                    let d = match rosc::decoder::decode_udp(&buf[..n]) {
                        Ok((_, rosc::OscPacket::Message(m))) => {
                            let a: Vec<String> = m.args.iter().map(|x| format!("{x:?}")).collect();
                            format!("{} [{}]", m.addr, a.join(", "))
                        }
                        _ => format!("<{n} bytes, not an OSC message>"),
                    };
                    log.lock().unwrap().push((ms, port, d));
                }
            }
        }));
    }

    println!("listening on {} ports for {secs}s", bound.len());
    println!("range: 7001-7010, 8000-8010, 9000-9010, plus 10000/10023/53000/3819/8080");
    println!(
        "
work the controls now - single clicks, then double clicks
"
    );
    sleep(Duration::from_secs(secs));
    for h in handles {
        let _ = h.join();
    }

    let mut v = log.lock().unwrap().clone();
    v.sort_by_key(|(t, _, _)| *t);
    println!("--- timeline ---");
    let mut prev: Option<u128> = None;
    for (ms, port, what) in &v {
        match prev {
            Some(p) => println!("  {ms:>7} ms (+{:>5})  :{port}  {what}", ms - p),
            None => println!("  {ms:>7} ms (     )  :{port}  {what}"),
        }
        prev = Some(*ms);
    }
    if v.is_empty() {
        println!("  nothing arrived on any of those ports");
    } else {
        let mut ports: Vec<u16> = v.iter().map(|(_, p, _)| *p).collect();
        ports.sort_unstable();
        ports.dedup();
        println!(
            "
{} events. Connector is transmitting to: {ports:?}",
            v.len()
        );
    }
    Ok(())
}

/// Can we see the PHYSICAL press, or only the firmware's decision?
///
/// Over MIDI we only ever observe what the firmware chose to emit - with
/// double-click enabled that is a synthesised pulse arriving after the
/// detection window. The HID interface carries the RAW surface protocol of
/// which MIDI is a translation (see the field notes), so if the double-click
/// logic lives in that translation, HID should show the press immediately.
///
/// Reads HID input reports and MIDI on one timeline. The gap between a HID
/// event and its MIDI counterpart IS the firmware's added latency.
fn rawprobe(secs: u64) -> R<()> {
    use std::sync::{Arc, Mutex};

    let t0 = std::time::Instant::now();
    let log: Arc<Mutex<Vec<(u128, String)>>> = Arc::new(Mutex::new(Vec::new()));

    // ---- MIDI ---------------------------------------------------------------
    let probe = MidiInput::new("probe")?;
    let names: Vec<String> = probe
        .ports()
        .iter()
        .filter_map(|p| probe.port_name(p).ok())
        .filter(|n| n.to_lowercase().contains(MATCH))
        .collect();
    let mut conns = Vec::new();
    for name in &names {
        let mut mi = MidiInput::new("probe")?;
        mi.ignore(Ignore::ActiveSense);
        let port = mi
            .ports()
            .into_iter()
            .find(|p| mi.port_name(p).map(|n| &n == name).unwrap_or(false))
            .ok_or("port vanished")?;
        let tag = if name.to_lowercase().contains("midiin2") {
            "b2"
        } else {
            "b1"
        };
        let log = Arc::clone(&log);
        conns.push(mi.connect(
            &port,
            "probe-in",
            move |_ts, msg, _| {
                if msg.len() == 3 && (msg[0] & 0xF0) == 0x90 {
                    let ms = std::time::Instant::now().duration_since(t0).as_millis();
                    log.lock().unwrap().push((
                        ms,
                        format!(
                            "MIDI {tag}  note 0x{:02X} {}",
                            msg[1],
                            if msg[2] > 0 { "DOWN" } else { "UP" }
                        ),
                    ));
                }
            },
            (),
        )?);
    }
    println!("MIDI inputs: {}", conns.len());

    // ---- HID ----------------------------------------------------------------
    let api = hidapi::HidApi::new()?;
    let path = api
        .device_list()
        .find(|d| d.vendor_id() == 0x04D8 && d.product_id() == 0xE44E)
        .map(|d| d.path().to_owned())
        .ok_or("no D700 HID interface - is the Connector or Configurator holding it?")?;
    let dev = api.open_path(&path)?;
    dev.set_blocking_mode(false).ok();
    // The device stays quiet on HID until a host session exists.
    dev.write(&[0x08, 0x2a, 0x2b, 0x29, 0x2c, 0x28, 0x00, 0x00, 0x00])?;
    sleep(Duration::from_millis(250));
    println!(
        "HID interface open, session started
"
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    println!(
        "capturing {secs}s - tap, hold, then double-click the same button
"
    );

    let mut buf = [0u8; 64];
    while std::time::Instant::now() < deadline {
        match dev.read_timeout(&mut buf, 5) {
            Ok(n) if n >= 5 => {
                let ms = std::time::Instant::now().duration_since(t0).as_millis();
                // Report id is byte 0. Only 0x04 carries realtime control:
                // 04 <port> <status> <d1> <d2>. 0x08 is keepalive/status.
                if buf[0] != 0x04 {
                    if buf[0] == 0x08 && buf[2] == 0x00 {
                        continue; // keepalive - not interesting here
                    }
                    log.lock().unwrap().push((
                        ms,
                        format!(
                            "HID  report 0x{:02X}  {}",
                            buf[0],
                            buf[..n.min(9)]
                                .iter()
                                .map(|b| format!("{b:02X}"))
                                .collect::<Vec<_>>()
                                .join(" ")
                        ),
                    ));
                    continue;
                }
                let (port, st, d1, d2) = (buf[1], buf[2], buf[3], buf[4]);
                let what = match st & 0xF0 {
                    0x90 => format!("note 0x{d1:02X} {}", if d2 > 0 { "DOWN" } else { "UP" }),
                    0xB0 => format!("CC 0x{d1:02X} = {d2}"),
                    0xE0 => continue, // faders: too chatty for this question
                    _ => format!("{st:02X} {d1:02X} {d2:02X}"),
                };
                log.lock()
                    .unwrap()
                    .push((ms, format!("HID  bank{port}  {what}")));
            }
            _ => {}
        }
    }

    let mut v = log.lock().unwrap().clone();
    v.sort_by_key(|(t, _)| *t);
    println!("--- merged timeline ---");
    let mut prev: Option<u128> = None;
    for (ms, what) in &v {
        match prev {
            Some(p) => println!("  {ms:>7} ms (+{:>5})  {what}", ms - p),
            None => println!("  {ms:>7} ms (     )  {what}"),
        }
        prev = Some(*ms);
    }
    println!(
        "
{} events. A HID event BEFORE its MIDI counterpart means HID",
        v.len()
    );
    println!("sees the raw switch, and the gap is the firmware's added latency.");
    Ok(())
}

/// Map the HID colour element index space to physical dials.
///
/// The earlier `hidrgbscan` lit nothing because it never opened a session -
/// the device ignores colour writes until `08 2a 2b 29 2c 28` is acknowledged.
/// This opens one first, then walks the index space as a visible sweep: if the
/// mapping is sequential the operator sees a light travel across the surface,
/// which identifies it far faster than reporting 18 separate observations.
///
/// Pacing is ~25 ms between writes (field notes finding 33: bursts stall the
/// device), and `0xb0` is never sent - it is the class that produced a stall.
fn rgbmap(class: u8, last: u8, dwell: u64) -> R<()> {
    if class == 0xb0 {
        return Err("0xb0 stalled the device once; refusing to send it".into());
    }
    let api = hidapi::HidApi::new()?;
    let path = api
        .device_list()
        .find(|d| d.vendor_id() == 0x04D8 && d.product_id() == 0xE44E)
        .map(|d| d.path().to_owned())
        .ok_or("no D700 HID interface - close the Configurator and Connector")?;
    let dev = api.open_path(&path)?;

    dev.write(&[0x08, 0x2a, 0x2b, 0x29, 0x2c, 0x28, 0x00, 0x00, 0x00])?;
    sleep(Duration::from_millis(300));
    println!("session opened");

    let set = |idx: u8, r: u8, g: u8, b: u8| -> R<()> {
        dev.write(&[0x08, 0x2a, 0x0a, class, idx, 0x00, r, g, b])?;
        sleep(Duration::from_millis(25));
        Ok(())
    };

    // Blank the whole candidate range first, so only the swept index is lit.
    for idx in 0..=last {
        set(idx, 0, 0, 0)?;
    }
    println!(
        "blanked 0x00..=0x{last:02X}
"
    );
    sleep(Duration::from_millis(400));

    println!("sweeping class 0x{class:02X}, one index at a time, {dwell}ms each");
    println!(
        "watch for a light travelling across the surface
"
    );
    for idx in 0..=last {
        set(idx, 255, 255, 255)?;
        println!("  index 0x{idx:02X} ({idx:2})");
        sleep(Duration::from_millis(dwell));
        set(idx, 0, 0, 0)?;
    }
    println!(
        "
Did a light sweep across? If so, in what order, and how many"
    );
    println!("distinct elements lit? Which index was the MASTER dial?");
    Ok(())
}

/// Encoder RGB over MIDI, per Asparion's own Bitwig control script.
///
/// `Dxxx_encoders.js`, `EncoderStrip.prototype.setColor`:
/// ```text
/// r = parseInt(r * 127);
/// sendMidi(NOTEON | 1, VPOT_CLICK0 + index, r);   // channel 2 - red
/// sendMidi(NOTEON | 2, VPOT_CLICK0 + index, g);   // channel 3 - green
/// sendMidi(NOTEON | 3, VPOT_CLICK0 + index, b);   // channel 4 - blue, refreshes
/// ```
/// with `VPOT_CLICK0 = 32` (`0x20`), the MCU V-Pot press note, and the bank
/// selected by which MIDI port the message is sent to.
///
/// This is NOTE-ON, not CC - which is why 32 CC-based scans found nothing. The
/// colour component travels as the velocity byte, 0..127.
///
/// Unlike the HID colour path (finding 35), the note number is a PHYSICAL
/// position, so this needs no provisioning.
fn mrgb() -> R<()> {
    let ports = out_ports()?;
    let mut conns: Vec<_> = ports.iter().filter_map(|n| open_out(n).ok()).collect();
    if conns.is_empty() {
        return Err("no D700 MIDI output".into());
    }
    println!(
        "banks: {}
",
        conns.len()
    );

    // r/g/b are 0..=255 here; the wire wants 0..=127.
    let set = |c: &mut midir::MidiOutputConnection, idx: u8, r: u8, g: u8, b: u8| -> R<()> {
        let note = 0x20 + idx;
        c.send(&[0x91, note, r / 2])?;
        c.send(&[0x92, note, g / 2])?;
        c.send(&[0x93, note, b / 2])?; // blue last - triggers the refresh
        Ok(())
    };

    println!("1/3  encoder 1 of bank 1 through named colours");
    for (r, g, b, n) in [
        (255u8, 0u8, 0u8, "red"),
        (0, 255, 0, "green"),
        (0, 0, 255, "blue"),
        (255, 180, 0, "amber"),
        (255, 255, 255, "white"),
    ] {
        set(&mut conns[0], 0, r, g, b)?;
        println!("     {n}");
        sleep(Duration::from_millis(1200));
    }

    println!(
        "
2/3  a rainbow across all encoders, both banks"
    );
    for (bank, c) in conns.iter_mut().enumerate() {
        for i in 0..8u8 {
            let h = ((bank * 8 + i as usize) as f32) / 16.0 * 6.0;
            let x = (255.0 * (1.0 - (h % 2.0 - 1.0).abs())) as u8;
            let (r, g, b) = match h as u32 {
                0 => (255, x, 0),
                1 => (x, 255, 0),
                2 => (0, 255, x),
                3 => (0, x, 255),
                4 => (x, 0, 255),
                _ => (255, 0, x),
            };
            set(c, i, r, g, b)?;
            sleep(Duration::from_millis(60));
        }
    }
    sleep(Duration::from_secs(3));

    println!(
        "
3/3  smooth hue rotation, 8-bit, both banks"
    );
    for step in 0..160u32 {
        for (bank, c) in conns.iter_mut().enumerate() {
            for i in 0..8u8 {
                let pos = (bank * 8 + i as usize) as f32;
                let h = ((step as f32) / 20.0 + pos / 16.0 * 6.0) % 6.0;
                let x = (255.0 * (1.0 - (h % 2.0 - 1.0).abs())) as u8;
                let (r, g, b) = match h as u32 {
                    0 => (255, x, 0),
                    1 => (x, 255, 0),
                    2 => (0, 255, x),
                    3 => (0, x, 255),
                    4 => (x, 0, 255),
                    _ => (255, 0, x),
                };
                set(c, i, r, g, b)?;
            }
        }
        sleep(Duration::from_millis(45));
    }

    for c in conns.iter_mut() {
        for i in 0..8u8 {
            set(c, i, 0, 0, 0)?;
        }
    }
    println!(
        "
done - did the encoders colour, all 16, with no provisioning?"
    );
    Ok(())
}
