use serde::{Deserialize, Serialize};

/// Channel stereo mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelMode {
    Mono,   // 1
    Stereo, // 2
}

impl ChannelMode {
    pub fn from_int(v: i32) -> Self {
        match v {
            2 => ChannelMode::Stereo,
            _ => ChannelMode::Mono,
        }
    }

    pub fn to_int(&self) -> i32 {
        match self {
            ChannelMode::Mono => 1,
            ChannelMode::Stereo => 2,
        }
    }
}

/// Console configuration discovered at startup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsoleConfig {
    pub console_name: String,
    pub console_serial: String,
    pub session_filename: Option<String>,

    pub input_channel_count: u8,  // 48 or 60
    pub aux_output_count: u8,     // depends on aux/group split
    pub group_output_count: u8,
    pub matrix_output_count: u8,  // 8
    pub matrix_input_count: u8,   // 10
    pub control_group_count: u8,  // 10
    pub graphic_eq_count: u8,     // 16
    pub talkback_output_count: u8,

    /// Per mix output: true = aux, false = group/bus
    pub mix_output_types: Vec<bool>,
    /// Per mix output: Mono or Stereo
    pub mix_output_modes: Vec<ChannelMode>,
    /// Per input: Mono or Stereo
    pub input_modes: Vec<ChannelMode>,
    /// Per group: Mono or Stereo
    pub group_modes: Vec<ChannelMode>,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            console_name: String::new(),
            console_serial: String::new(),
            session_filename: None,
            input_channel_count: 48,
            aux_output_count: 8,
            group_output_count: 16,
            matrix_output_count: 8,
            matrix_input_count: 10,
            control_group_count: 10,
            graphic_eq_count: 16,
            talkback_output_count: 0,
            mix_output_types: Vec::new(),
            mix_output_modes: Vec::new(),
            input_modes: Vec::new(),
            group_modes: Vec::new(),
        }
    }
}
