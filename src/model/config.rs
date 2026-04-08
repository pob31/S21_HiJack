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

/// Whether the console has the paid "+" upgrade (expanded channel count).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelOption {
    /// Base: 48 inputs / 16 mix outputs
    Base,
    /// Expanded (+): 60 inputs / 24 mix outputs
    Expanded,
}

impl ChannelOption {
    pub fn input_count(self) -> u8 {
        match self {
            ChannelOption::Base => 48,
            ChannelOption::Expanded => 60,
        }
    }

    pub fn mix_output_count(self) -> u8 {
        match self {
            ChannelOption::Base => 16,
            ChannelOption::Expanded => 24,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChannelOption::Base => "48 / 16",
            ChannelOption::Expanded => "60 / 24 (+)",
        }
    }
}

/// Console configuration discovered at startup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsoleConfig {
    pub console_name: String,
    pub console_serial: String,
    pub session_filename: Option<String>,

    /// Which channel option is active (base or expanded).
    #[serde(default)]
    pub channel_option: ChannelOption,

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

impl ConsoleConfig {
    /// Apply the channel option, updating input and mix output counts.
    /// The default aux/group split for base is 8/8, for expanded is 8/16.
    pub fn apply_channel_option(&mut self, option: ChannelOption) {
        self.channel_option = option;
        self.input_channel_count = option.input_count();
        // If aux/group split hasn't been discovered, set reasonable defaults
        let total_mix = option.mix_output_count();
        if self.aux_output_count + self.group_output_count == 0
            || self.aux_output_count + self.group_output_count != total_mix
        {
            self.aux_output_count = 8;
            self.group_output_count = total_mix - 8;
        }
    }
}

impl Default for ChannelOption {
    fn default() -> Self {
        ChannelOption::Base
    }
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        let option = ChannelOption::Base;
        Self {
            console_name: String::new(),
            console_serial: String::new(),
            session_filename: None,
            channel_option: option,
            input_channel_count: option.input_count(),
            aux_output_count: 8,
            group_output_count: option.mix_output_count() - 8,
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
