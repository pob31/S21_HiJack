//! Scribble strips and strip colour for the fader sidecar — pure.
//!
//! Decides what each strip of the surface shows (the bound console
//! channel's name, the parameter's value, a short parameter tag, and a
//! colour) and encodes it for the surface. The sidecar service owns the
//! timing and the diffing; nothing here touches MIDI or locks, so the
//! choices and the byte layouts are unit-testable.
//!
//! Byte layouts are the ones confirmed on hardware in
//! `Documentation/D700_CONTROL_GUIDE.md` §4.4–4.6 (native D700 rows, MCU
//! LCD, RGB note-ons) and `D700_FIELD_NOTES.md` finding 10 (SysEx `0x72`,
//! the X-Touch strip colour). Only those commands are sent: an
//! undocumented SysEx command once put the D700's displays into a state
//! that needed a power cycle.

use std::collections::HashMap;

use crate::model::channel::ChannelId;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use crate::model::sidecar::{
    BindingTarget, STRIPS_PER_PORT, SidecarBinding, SidecarConfig, SurfaceDisplay, hue,
};
use crate::model::state::ConsoleState;

/// What one strip shows. Blank (the default) for a strip nothing claims.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StripContent {
    /// Top row: the console channel's name.
    pub name: String,
    /// Second row: the bound parameter's value.
    pub value: String,
    /// Third row (D700 only): which parameter it is.
    pub tag: String,
    /// 8-bit RGB; black = unlit.
    pub color: [u8; 3],
}

/// The content of every claimed strip, keyed by (port, strip). A strip is
/// claimed by an enabled binding whose control sits on it (see
/// [`SidecarBinding::display_strip`]); when a fader and an encoder share a
/// strip, the fader wins.
pub fn strip_contents(
    cfg: &SidecarConfig,
    state: &ConsoleState,
) -> HashMap<(u8, u8), StripContent> {
    let mut owner: HashMap<(u8, u8), &SidecarBinding> = HashMap::new();
    for b in cfg.bindings.iter().filter(|b| b.enabled) {
        let Some(strip) = b.display_strip() else {
            continue;
        };
        let slot = owner.entry((b.port, strip)).or_insert(b);
        if !slot.mode.is_absolute() && b.mode.is_absolute() {
            *slot = b;
        }
    }
    owner
        .into_iter()
        .map(|(key, b)| (key, binding_content(b, state)))
        .collect()
}

fn binding_content(b: &SidecarBinding, state: &ConsoleState) -> StripContent {
    match &b.target {
        BindingTarget::ConsoleParameter { address } => StripContent {
            name: channel_name(&address.channel, state),
            value: match state.get(address) {
                Some(ParameterValue::Float(v)) => format_value(&address.parameter, *v),
                _ => String::new(),
            },
            tag: parameter_tag(address),
            color: b.strip_color(),
        },
        // No console state behind an external target: its label, and no
        // value to show.
        BindingTarget::RawOsc { .. } => StripContent {
            name: b.label.clone(),
            value: String::new(),
            tag: "OSC".into(),
            color: b.strip_color(),
        },
    }
}

/// The channel's name on the desk, or a short label when it has none (or
/// the mirror hasn't seen it yet).
fn channel_name(channel: &ChannelId, state: &ConsoleState) -> String {
    let addr = ParameterAddress {
        channel: channel.clone(),
        parameter: ParameterPath::Name,
    };
    if let Some(ParameterValue::String(name)) = state.get(&addr)
        && !name.trim().is_empty()
    {
        return name.trim().to_string();
    }
    match channel {
        ChannelId::Input(n) => format!("In {n}"),
        ChannelId::Aux(n) => format!("Aux {n}"),
        ChannelId::Group(n) => format!("Grp {n}"),
        ChannelId::Matrix(n) => format!("Mtx {n}"),
        ChannelId::ControlGroup(n) => format!("CG {n}"),
        ChannelId::GraphicEq(n) => format!("GEQ {n}"),
        ChannelId::MatrixInput(n) => format!("MtxIn {n}"),
    }
}

/// A short parameter tag for the D700's 8-character third row.
fn parameter_tag(address: &ParameterAddress) -> String {
    match &address.parameter {
        ParameterPath::Fader => "Fader".into(),
        ParameterPath::SendLevel(n) => format!("Send {n}"),
        ParameterPath::SendPan(n) => format!("SPan {n}"),
        ParameterPath::MatrixSendLevel(n) => format!("Mtx {n}"),
        ParameterPath::CgLevel => "CG lvl".into(),
        ParameterPath::AnalogGain => "Gain".into(),
        ParameterPath::EqBandGain(b) => format!("EQ{b} gain"),
        ParameterPath::EqBandFrequency(b) => format!("EQ{b} freq"),
        p => p.label(),
    }
}

/// The value as the strip shows it: dB for levels and gains (`-inf` at the
/// fader's bottom), L/C/R for pan, Hz for frequencies, else a plain number.
/// Kept within 7 characters where it can be, the Mackie field width.
pub fn format_value(parameter: &ParameterPath, v: f32) -> String {
    use ParameterPath as P;
    if !v.is_finite() {
        return String::new();
    }
    let db = |v: f32| {
        let s = if v.abs() < 0.05 {
            "0.0".to_string()
        } else {
            format!("{v:+.1}")
        };
        if s.len() <= 4 {
            format!("{s} dB")
        } else {
            format!("{s}dB")
        }
    };
    match parameter {
        p if p.is_fader_level() => {
            // The fader's bottom detent is −150 dB on the wire; anything
            // that far down reads as off.
            if v <= -140.0 { "-inf".into() } else { db(v) }
        }
        P::AnalogGain
        | P::Trim
        | P::EqBandGain(_)
        | P::GeqBandGain(_)
        | P::Dyn1Gain(_)
        | P::Dyn2Gain
        | P::Dyn1Threshold(_)
        | P::Dyn2Threshold
        | P::Dyn2Range
        | P::EqBandDynThreshold(_) => db(v),
        P::Pan | P::SendPan(_) | P::Balance => {
            let pct = (v * 100.0).round();
            if pct == 0.0 {
                "C".into()
            } else if pct < 0.0 {
                format!("L{}", -pct)
            } else {
                format!("R{pct}")
            }
        }
        P::HighpassFrequency
        | P::LowpassFrequency
        | P::EqBandFrequency(_)
        | P::Dyn1CrossoverHigh
        | P::Dyn1CrossoverLow => {
            if v >= 1000.0 {
                format!("{:.2}k", v / 1000.0)
            } else {
                format!("{v:.0}Hz")
            }
        }
        _ => format!("{v:.2}"),
    }
}

// ─── Wire encoding ───────────────────────────────────────────────────

/// Mackie SysEx header up to the device id.
const SYSEX_HEAD: [u8; 4] = [0xF0, 0x00, 0x00, 0x66];

/// The Mackie device id for a port: the main unit (`0x14`) on port 1, an
/// Extender (`0x15`) on port 2 — an X-Touch + Extender pair needs that.
/// The D700 honours both alike (field note 7).
fn device_id(port: u8) -> u8 {
    if port == 0 { 0x14 } else { 0x15 }
}

/// Text rows per strip for a protocol: (row, field width).
pub fn rows(display: SurfaceDisplay) -> &'static [(u8, usize)] {
    match display {
        SurfaceDisplay::Off => &[],
        SurfaceDisplay::Mackie => &[(0, 7), (1, 7)],
        SurfaceDisplay::AsparionD700 => &[(0, 12), (1, 12), (2, 8)],
    }
}

/// The text for a row of a strip's content.
pub fn row_text(content: &StripContent, row: u8) -> &str {
    match row {
        0 => &content.name,
        1 => &content.value,
        _ => &content.tag,
    }
}

/// ASCII only, padded or cut to exactly `width`: the displays take 7-bit
/// ASCII, and a short write leaves stale characters behind (gotcha 4).
pub fn fit(text: &str, width: usize) -> Vec<u8> {
    let mut out: Vec<u8> = text
        .chars()
        .map(|c| {
            if (' '..='~').contains(&c) {
                c as u8
            } else {
                b'?'
            }
        })
        .take(width)
        .collect();
    out.resize(width, b' ');
    out
}

/// One row of one strip's text.
pub fn encode_text(
    display: SurfaceDisplay,
    port: u8,
    strip: u8,
    row: u8,
    text: &str,
) -> Option<Vec<u8>> {
    let strip = strip.min(STRIPS_PER_PORT - 1);
    let mut msg = SYSEX_HEAD.to_vec();
    match display {
        SurfaceDisplay::Off => return None,
        SurfaceDisplay::Mackie => {
            // Flat 2 × 56 buffer: row 1 starts at 0x38, 7 chars per strip.
            msg.extend([device_id(port), 0x12, row * 0x38 + strip * 7]);
            msg.extend(fit(text, 7));
        }
        SurfaceDisplay::AsparionD700 => {
            msg.push(device_id(port));
            if row < 2 {
                // The row byte is 1-based on the wire.
                msg.extend([0x1A, strip * 12, row + 1]);
                msg.extend(fit(text, 12));
            } else {
                msg.extend([0x19, strip * 8]);
                msg.extend(fit(text, 8));
            }
        }
    }
    msg.push(0xF7);
    Some(msg)
}

/// A colour as the X-Touch 3-bit code (bit 0 red, bit 1 green, bit 2 blue):
/// the nearest of its six colours by hue, white for a near-grey, off for a
/// near-black. Cyan is a narrow band, so the desk's steel blue for inputs
/// reads as blue, and its CG brown as yellow — not per-component
/// thresholds, which turned the desk's darker colours off.
pub fn color_code(rgb: [u8; 3]) -> u8 {
    let max = rgb.into_iter().max().unwrap_or(0);
    let min = rgb.into_iter().min().unwrap_or(0);
    if max < 32 {
        return 0;
    }
    let Some(h) = hue(rgb).filter(|_| f32::from(max - min) / f32::from(max) >= 0.25) else {
        return 7;
    };
    match h {
        h if !(30.0..330.0).contains(&h) => 1, // red
        h if h < 90.0 => 3,                    // yellow
        h if h < 165.0 => 2,                   // green
        h if h < 195.0 => 6,                   // cyan
        h if h < 270.0 => 4,                   // blue
        _ => 5,                                // magenta
    }
}

/// The messages that colour a port's eight strips.
///
/// Mackie: SysEx `0x72`, eight 3-bit colours at once. D700: the same
/// `0x72` (strip and backlight), then full RGB per encoder — red, green,
/// then blue last, which is what makes the ring refresh (§4.4). Components
/// are 0–127, so 8-bit colour is halved.
pub fn encode_colors(display: SurfaceDisplay, port: u8, colors: &[[u8; 3]]) -> Vec<Vec<u8>> {
    if display == SurfaceDisplay::Off {
        return Vec::new();
    }
    let mut msg = SYSEX_HEAD.to_vec();
    msg.extend([device_id(port), 0x72]);
    msg.extend(
        (0..usize::from(STRIPS_PER_PORT)).map(|i| colors.get(i).copied().map_or(0, color_code)),
    );
    msg.push(0xF7);
    let mut out = vec![msg];
    if display == SurfaceDisplay::AsparionD700 {
        for (i, rgb) in colors.iter().take(usize::from(STRIPS_PER_PORT)).enumerate() {
            let note = 0x20 + i as u8;
            out.push(vec![0x91, note, rgb[0] >> 1]);
            out.push(vec![0x92, note, rgb[1] >> 1]);
            out.push(vec![0x93, note, rgb[2] >> 1]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::ConsoleConfig;
    use crate::model::sidecar::{ControlMode, ControlSelector, RelativeMode, Taper};
    use uuid::Uuid;

    fn binding(
        port: u8,
        control: ControlSelector,
        mode: ControlMode,
        ch: ChannelId,
        p: ParameterPath,
    ) -> SidecarBinding {
        SidecarBinding {
            id: Uuid::new_v4(),
            label: "label".into(),
            control,
            mode,
            target: BindingTarget::ConsoleParameter {
                address: ParameterAddress {
                    channel: ch,
                    parameter: p,
                },
            },
            taper: Taper::FaderDb { max_db: 10.0 },
            motor_feedback: true,
            touch: None,
            relative_step: 0.01,
            enabled: true,
            port,
            color: None,
        }
    }

    fn fader(port: u8, strip: u8, ch: ChannelId) -> SidecarBinding {
        binding(
            port,
            ControlSelector::PitchBend { channel: strip + 1 },
            ControlMode::PitchBend14,
            ch,
            ParameterPath::Fader,
        )
    }

    #[test]
    fn strips_show_channel_name_value_and_type_colour() {
        let mut state = ConsoleState::new(ConsoleConfig::default());
        let name = ParameterAddress {
            channel: ChannelId::Input(12),
            parameter: ParameterPath::Name,
        };
        let level = ParameterAddress {
            channel: ChannelId::Input(12),
            parameter: ParameterPath::Fader,
        };
        state.update(name, ParameterValue::String("Kick In".into()));
        state.update(level, ParameterValue::Float(-6.2));
        let cfg = SidecarConfig {
            enabled: true,
            bindings: vec![
                fader(1, 3, ChannelId::Input(12)),
                fader(0, 0, ChannelId::Aux(2)),
            ],
            display: SurfaceDisplay::AsparionD700,
        };
        let got = strip_contents(&cfg, &state);
        assert_eq!(
            got[&(1, 3)],
            StripContent {
                name: "Kick In".into(),
                value: "-6.2 dB".into(),
                tag: "Fader".into(),
                color: ChannelId::Input(12).led_color(),
            }
        );
        // No name, no value yet: a short channel label and an empty value.
        assert_eq!(got[&(0, 0)].name, "Aux 2");
        assert_eq!(got[&(0, 0)].value, "");
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn fader_wins_its_strip_over_the_encoder() {
        let state = ConsoleState::new(ConsoleConfig::default());
        let encoder = binding(
            0,
            ControlSelector::Cc {
                channel: 1,
                cc: 0x12,
            },
            ControlMode::Relative(RelativeMode::SignMagnitude),
            ChannelId::Input(1),
            ParameterPath::Pan,
        );
        assert_eq!(encoder.display_strip(), Some(2));
        let cfg = SidecarConfig {
            enabled: true,
            bindings: vec![encoder, fader(0, 2, ChannelId::Group(4))],
            display: SurfaceDisplay::Mackie,
        };
        assert_eq!(strip_contents(&cfg, &state)[&(0, 2)].name, "Grp 4");
    }

    #[test]
    fn controls_off_the_strips_and_disabled_bindings_claim_nothing() {
        let state = ConsoleState::new(ConsoleConfig::default());
        let master = fader(0, 8, ChannelId::Input(1)); // pitch bend ch 9
        let mut parked = fader(0, 1, ChannelId::Input(2));
        parked.enabled = false;
        let cfg = SidecarConfig {
            enabled: true,
            bindings: vec![master, parked],
            display: SurfaceDisplay::Mackie,
        };
        assert!(strip_contents(&cfg, &state).is_empty());
    }

    #[test]
    fn values_format_by_parameter_family() {
        assert_eq!(format_value(&ParameterPath::Fader, -150.0), "-inf");
        assert_eq!(format_value(&ParameterPath::Fader, 0.0), "0.0 dB");
        assert_eq!(format_value(&ParameterPath::Fader, 5.0), "+5.0 dB");
        assert_eq!(format_value(&ParameterPath::Fader, -42.5), "-42.5dB");
        assert_eq!(format_value(&ParameterPath::SendLevel(3), -10.0), "-10.0dB");
        assert_eq!(format_value(&ParameterPath::Pan, 0.0), "C");
        assert_eq!(format_value(&ParameterPath::Pan, -0.3), "L30");
        assert_eq!(format_value(&ParameterPath::Pan, 1.0), "R100");
        assert_eq!(
            format_value(&ParameterPath::HighpassFrequency, 80.0),
            "80Hz"
        );
        assert_eq!(
            format_value(&ParameterPath::EqBandFrequency(2), 2500.0),
            "2.50k"
        );
        assert_eq!(format_value(&ParameterPath::Fader, f32::NAN), "");
        for v in [-150.0, -99.9, -42.5, 0.0, 10.0] {
            assert!(format_value(&ParameterPath::Fader, v).len() <= 7);
        }
    }

    #[test]
    fn text_is_ascii_and_exactly_the_field_width() {
        assert_eq!(fit("Kick", 7), b"Kick   ".to_vec());
        assert_eq!(fit("Overheads L", 7), b"Overhea".to_vec());
        assert_eq!(fit("Bässe", 7), b"B?sse  ".to_vec());
    }

    #[test]
    fn mackie_text_lands_in_the_flat_buffer() {
        let top = encode_text(SurfaceDisplay::Mackie, 0, 2, 0, "Kick").unwrap();
        assert_eq!(top[..7], [0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, 0x0E]);
        assert_eq!(&top[7..14], b"Kick   ");
        assert_eq!(top[14], 0xF7);
        // Second row, last strip, on the Extender's id.
        let bottom = encode_text(SurfaceDisplay::Mackie, 1, 7, 1, "-6.2 dB").unwrap();
        assert_eq!(bottom[4..7], [0x15, 0x12, 0x38 + 0x31]);
        assert_eq!(encode_text(SurfaceDisplay::Off, 0, 0, 0, "x"), None);
    }

    #[test]
    fn d700_text_uses_the_native_rows() {
        let name = encode_text(SurfaceDisplay::AsparionD700, 0, 3, 0, "Kick In").unwrap();
        assert_eq!(name[4..8], [0x14, 0x1A, 36, 0x01]);
        assert_eq!(&name[8..20], b"Kick In     ");
        assert_eq!(name[20], 0xF7);
        let value = encode_text(SurfaceDisplay::AsparionD700, 1, 3, 1, "-6.2 dB").unwrap();
        assert_eq!(value[5..8], [0x1A, 36, 0x02]);
        let tag = encode_text(SurfaceDisplay::AsparionD700, 0, 3, 2, "Fader").unwrap();
        assert_eq!(tag[4..7], [0x14, 0x19, 24]);
        assert_eq!(&tag[7..15], b"Fader   ");
        assert_eq!(tag.len(), 16);
    }

    #[test]
    fn colours_quantize_for_mackie_and_go_full_rgb_blue_last_on_the_d700() {
        let colors = [[255, 190, 0], [0, 200, 255], [0, 0, 0], [255, 255, 255]];
        let mackie = encode_colors(SurfaceDisplay::Mackie, 0, &colors);
        assert_eq!(mackie.len(), 1);
        // yellow, cyan, off, white, then the unset strips off.
        assert_eq!(mackie[0][4..], [0x14, 0x72, 3, 6, 0, 7, 0, 0, 0, 0, 0xF7]);

        let d700 = encode_colors(SurfaceDisplay::AsparionD700, 1, &colors);
        assert_eq!(d700.len(), 1 + 3 * colors.len());
        assert_eq!(d700[1], vec![0x91, 0x20, 127]);
        assert_eq!(d700[2], vec![0x92, 0x20, 95]);
        assert_eq!(d700[3], vec![0x93, 0x20, 0]);
        assert_eq!(d700[6], vec![0x93, 0x21, 127]);
        assert!(encode_colors(SurfaceDisplay::Off, 0, &colors).is_empty());
    }

    /// On an 8-colour strip the desk's colours read as the operator names
    /// them: blue inputs, purple auxes, burgundy groups, yellow CGs, green
    /// matrices.
    #[test]
    fn desk_colours_quantize_to_their_names() {
        use crate::model::channel::*;
        assert_eq!(color_code(INPUT_COLOR), 4, "blue");
        assert_eq!(color_code(AUX_COLOR), 5, "magenta");
        assert_eq!(color_code(GROUP_COLOR), 1, "red");
        assert_eq!(color_code(CG_COLOR), 3, "yellow");
        assert_eq!(color_code(MATRIX_COLOR), 2, "green");
        // And in their LED form, as the strips send them.
        for ch in [
            ChannelId::Input(1),
            ChannelId::Aux(1),
            ChannelId::Group(1),
            ChannelId::ControlGroup(1),
            ChannelId::Matrix(1),
        ] {
            assert_eq!(color_code(ch.led_color()), color_code(ch.type_color()));
        }
        // Matrix green is pure green on the LEDs, not the picker's teal.
        assert_eq!(ChannelId::Matrix(1).led_color(), [0, 255, 0]);
        assert_eq!(color_code([0, 0, 0]), 0);
        assert_eq!(color_code([200, 200, 210]), 7);
    }
}
