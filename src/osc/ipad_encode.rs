use rosc::OscType;

use super::ipad_values;
use crate::model::channel::ChannelId;
use crate::model::family::{BoolWire, LevelWire, PadWire, PanWire};
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};

/// Encode a parameter address and value into an OSC path and args for the
/// given wire dialect.
///
/// Returns None for parameters with no representation on the shared Pad tree
/// (S-series GP OSC-only paths). Applies the surface's path prefix and level
/// scaling, and the family's pan and boolean wire conventions.
pub fn encode_pad_parameter(
    addr: &ParameterAddress,
    value: &ParameterValue,
    wire: &PadWire,
) -> Option<(String, Vec<OscType>)> {
    let path = pad_path(&addr.channel, &addr.parameter, wire)?;
    let effective_value = convert_value_for_pad(&addr.parameter, value, wire);
    let args = vec![value_to_osc_type(&effective_value, wire.quirks.bool_wire)];
    Some((path, args))
}

/// [`encode_pad_parameter`] under the hardware-verified S21 dialect.
#[inline]
pub fn encode_ipad_parameter(
    addr: &ParameterAddress,
    value: &ParameterValue,
) -> Option<(String, Vec<OscType>)> {
    encode_pad_parameter(addr, value, &PadWire::S21)
}

/// Encode a query message — the parameter's own address with `/?` appended and
/// no argument. The desk replies on the un-suffixed path.
pub fn encode_pad_query(
    channel: &ChannelId,
    parameter: &ParameterPath,
    wire: &PadWire,
) -> Option<String> {
    Some(format!("{}/?", pad_path(channel, parameter, wire)?))
}

/// [`encode_pad_query`] under the hardware-verified S21 dialect.
#[inline]
pub fn encode_ipad_query(channel: &ChannelId, parameter: &ParameterPath) -> Option<String> {
    encode_pad_query(channel, parameter, &PadWire::S21)
}

/// The full address for a parameter on this surface: surface prefix, then the
/// channel prefix, then the parameter suffix.
fn pad_path(channel: &ChannelId, parameter: &ParameterPath, wire: &PadWire) -> Option<String> {
    let chan = channel.to_pad_path_prefix(&wire.quirks);
    let suffix = parameter.to_pad_suffix(&wire.quirks)?;
    Some(format!("{}{chan}/{suffix}", wire.surface.path_prefix()))
}

/// Convert an internal value to its on-the-wire form: pan range, and level
/// scaling on surfaces that carry normalized positions instead of dB.
fn convert_value_for_pad(
    parameter: &ParameterPath,
    value: &ParameterValue,
    wire: &PadWire,
) -> ParameterValue {
    match parameter {
        ParameterPath::Pan | ParameterPath::SendPan(_) => match (value, wire.quirks.pan_wire) {
            (ParameterValue::Float(f), PanWire::ZeroToOne) => {
                ParameterValue::Float(ipad_values::internal_pan_to_ipad(*f))
            }
            // SignedUnit matches the internal representation — no conversion.
            _ => value.clone(),
        },
        p if p.is_fader_level() && wire.surface.level_wire() == LevelWire::Normalized01 => {
            match value {
                ParameterValue::Float(db) => {
                    ParameterValue::Float(ipad_values::db_to_normalized(*db))
                }
                _ => value.clone(),
            }
        }
        _ => value.clone(),
    }
}

fn value_to_osc_type(value: &ParameterValue, bool_wire: BoolWire) -> OscType {
    match value {
        ParameterValue::Float(f) => OscType::Float(*f),
        ParameterValue::Int(i) => OscType::Int(*i),
        // The S21 sends toggles (mute, solo, *_enabled, …) as a FLOAT 0.0/1.0
        // — confirmed in the live OSC log ("0f"/"1f"). This is a different
        // convention from GP OSC (OSC booleans); only genuinely boolean params
        // reach this arm, so enum-ish Pad ints (stereo_mode, main_alt_in) —
        // which are `ParameterValue::Int` — are unaffected. Inbound parsing
        // accepts all three encodings regardless (`ipad_parse::extract_bool`),
        // so this choice only governs what we send.
        ParameterValue::Bool(b) => match bool_wire {
            BoolWire::Float01 => OscType::Float(if *b { 1.0 } else { 0.0 }),
            BoolWire::OscBool => OscType::Bool(*b),
            BoolWire::Int01 => OscType::Int(if *b { 1 } else { 0 }),
        },
        ParameterValue::String(s) => OscType::String(s.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_fader() {
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        let (path, args) = encode_ipad_parameter(&addr, &ParameterValue::Float(-10.0)).unwrap();
        assert_eq!(path, "/Input_Channels/1/fader");
        assert_eq!(args, vec![OscType::Float(-10.0)]);
    }

    #[test]
    fn encode_ipad_only_param() {
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::InsertAEnabled,
        };
        let (path, args) = encode_ipad_parameter(&addr, &ParameterValue::Bool(true)).unwrap();
        assert_eq!(path, "/Input_Channels/1/Insert/insert_A_in");
        // iPad toggles are float 0.0/1.0, not int.
        assert_eq!(args, vec![OscType::Float(1.0)]);
    }

    #[test]
    fn encode_graphic_eq() {
        let addr = ParameterAddress {
            channel: ChannelId::GraphicEq(3),
            parameter: ParameterPath::GeqBandGain(12),
        };
        let (path, args) = encode_ipad_parameter(&addr, &ParameterValue::Float(6.0)).unwrap();
        assert_eq!(path, "/Graphic_EQ/3/geq_gain_12");
        assert_eq!(args, vec![OscType::Float(6.0)]);
    }

    #[test]
    fn encode_pan_with_conversion() {
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Pan,
        };
        // Internal 0.0 (center) → iPad 0.5
        let (path, args) = encode_ipad_parameter(&addr, &ParameterValue::Float(0.0)).unwrap();
        assert_eq!(path, "/Input_Channels/1/Panner/pan");
        match &args[0] {
            OscType::Float(f) => assert!((*f - 0.5).abs() < 1e-6),
            _ => panic!("Expected float"),
        }
    }

    #[test]
    fn encode_send_pan_with_conversion() {
        let addr = ParameterAddress {
            channel: ChannelId::Input(5),
            parameter: ParameterPath::SendPan(3),
        };
        // Internal -1.0 (hard left) → iPad 0.0
        let (path, args) = encode_ipad_parameter(&addr, &ParameterValue::Float(-1.0)).unwrap();
        assert_eq!(path, "/Input_Channels/5/Aux_Send/3/send_pan");
        match &args[0] {
            OscType::Float(f) => assert!((*f - 0.0).abs() < 1e-6),
            _ => panic!("Expected float"),
        }
    }

    #[test]
    fn encode_gp_only_returns_none() {
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::DigitubeEnabled,
        };
        assert!(encode_ipad_parameter(&addr, &ParameterValue::Bool(true)).is_none());
    }

    #[test]
    fn encode_query() {
        let path = encode_ipad_query(&ChannelId::Input(1), &ParameterPath::Fader).unwrap();
        assert_eq!(path, "/Input_Channels/1/fader/?");

        let path = encode_ipad_query(&ChannelId::ControlGroup(1), &ParameterPath::Fader).unwrap();
        assert_eq!(path, "/Control_Groups/0/fader/?"); // CG 0-based

        assert!(encode_ipad_query(&ChannelId::Input(1), &ParameterPath::DigitubeEnabled).is_none());
    }

    #[test]
    fn encode_control_group_0_based() {
        let addr = ParameterAddress {
            channel: ChannelId::ControlGroup(1),
            parameter: ParameterPath::Fader,
        };
        let (path, _) = encode_ipad_parameter(&addr, &ParameterValue::Float(0.0)).unwrap();
        assert_eq!(path, "/Control_Groups/0/fader");
    }

    /// The iPad numbers EQ bands in reverse of the internal model: internal
    /// band b encodes to iPad wire band (5 - b).
    #[test]
    fn encode_eq_band_reversed() {
        for (internal, wire) in [(1u8, 4u8), (2, 3), (3, 2), (4, 1)] {
            let addr = ParameterAddress {
                channel: ChannelId::Input(2),
                parameter: ParameterPath::EqBandGain(internal),
            };
            let (path, _) = encode_ipad_parameter(&addr, &ParameterValue::Float(0.0)).unwrap();
            assert_eq!(
                path,
                format!("/Input_Channels/2/EQ/eq_gain_{wire}"),
                "internal band {internal} should encode to iPad wire band {wire}"
            );
        }
    }

    /// Every EQ band parameter must survive an iPad suffix encode → parse
    /// round-trip with its band index intact (the reversal is its own inverse).
    #[test]
    fn eq_band_suffix_round_trip_all_params() {
        for b in 1u8..=4 {
            let params = [
                ParameterPath::EqBandFrequency(b),
                ParameterPath::EqBandGain(b),
                ParameterPath::EqBandQ(b),
                ParameterPath::EqBandCurve(b),
                ParameterPath::EqBandDynEnabled(b),
                ParameterPath::EqBandDynThreshold(b),
                ParameterPath::EqBandDynRatio(b),
                ParameterPath::EqBandDynAttack(b),
                ParameterPath::EqBandDynRelease(b),
                ParameterPath::EqBandDynOverUnder(b),
            ];
            for param in params {
                let suffix = param
                    .to_ipad_suffix()
                    .unwrap_or_else(|| panic!("EQ band param must encode: {param:?}"));
                assert_eq!(
                    ParameterPath::from_ipad_suffix(&suffix),
                    Some(param.clone()),
                    "suffix round-trip failed for {param:?} (suffix {suffix})"
                );
            }
        }
    }

    /// The iPad multiband compressor swaps Mid/High bands; encode → parse must
    /// preserve the internal band index for every per-band Dyn1 parameter.
    #[test]
    fn dyn1_multiband_band_swap_round_trip() {
        for b in 1u8..=3 {
            let params = [
                ParameterPath::Dyn1Threshold(b),
                ParameterPath::Dyn1Knee(b),
                ParameterPath::Dyn1Ratio(b),
                ParameterPath::Dyn1Attack(b),
                ParameterPath::Dyn1Release(b),
                ParameterPath::Dyn1Gain(b),
                ParameterPath::Dyn1Listen(b),
            ];
            for param in params {
                let suffix = param
                    .to_ipad_suffix()
                    .unwrap_or_else(|| panic!("Dyn1 band param must encode: {param:?}"));
                assert_eq!(
                    ParameterPath::from_ipad_suffix(&suffix),
                    Some(param.clone()),
                    "suffix round-trip failed for {param:?} (suffix {suffix})"
                );
            }
        }
        // Explicit wire mapping: internal Mid (2) ↔ iPad band 3, High (3) ↔ band 2.
        assert_eq!(
            ParameterPath::Dyn1Threshold(2).to_ipad_suffix().unwrap(),
            "Dynamics/comp_thresh_3"
        );
        assert_eq!(
            ParameterPath::Dyn1Threshold(3).to_ipad_suffix().unwrap(),
            "Dynamics/comp_thresh_2"
        );
    }
}
