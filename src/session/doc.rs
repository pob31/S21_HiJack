//! Console-agnostic model of a console session file.
//!
//! Both readers (S21 SQLite, SD/Quantum binary) produce a [`SessionDoc`]. Every
//! value is a named [`Param`] under a stable dotted key (`eq.band1.freq`), so
//! reports can group by section and snapshots can be diffed key by key.
//! A value that is decoded but not yet confirmed against ground truth carries
//! `confirmed: false`; see `Documentation/SESSION_FILES_FIELD_NOTES.md`.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SessionDoc {
    pub source: Source,
    pub show_name: String,
    pub sample_rate: Option<u32>,
    /// Every strip the file declares, in file order.
    pub strips: Vec<Strip>,
    /// The state the desk was in when the file was saved.
    pub live: Vec<StripState>,
    pub snapshots: Vec<SnapshotDoc>,
    /// SD snapshot groups (1-based `SnapshotDoc::group` indexes into this).
    pub snapshot_groups: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Source {
    pub file_name: String,
    pub format: Format,
}

#[derive(Debug, Clone, Serialize)]
pub enum Format {
    /// S21/S31 SQLite session, with its schema version (`PRAGMA user_version`).
    S21Session { user_version: i64 },
    /// SD-range or Quantum binary session.
    SdSes {
        /// Model tag from the header: `SD8`, `SD9`, `SDC` (SD12 family), `Q2`.
        model: String,
        version: char,
        build: u16,
    },
}

impl Format {
    pub fn describe(&self) -> String {
        match self {
            Format::S21Session { user_version } => {
                format!("DiGiCo S21/S31 session (schema {user_version})")
            }
            Format::SdSes {
                model,
                version,
                build,
            } => {
                let family = match model.as_str() {
                    "Q2" => "Quantum",
                    "SDC" => "SD12",
                    "SD8" => "SD8",
                    "SD9" => "SD9",
                    other => other,
                };
                format!("DiGiCo {family} session (format v{version}, software build {build})")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum StripKind {
    Input,
    Aux,
    Group,
    /// Right leg of a stereo aux or group.
    BusRight,
    Master,
    Monitoring,
    ControlGroup,
    Solo,
    Talkback,
    Fx,
    MatrixInput,
    Matrix,
    GraphicEq,
    /// Quantum nodal processing on one send ("Ch 28 > Aux 2").
    Node,
    /// Quantum Spice Rack slot.
    SpiceRack,
    Generator,
    InputSocket,
    OutputSocket,
    Other,
}

impl StripKind {
    pub fn label(self) -> &'static str {
        match self {
            StripKind::Input => "Input",
            StripKind::Aux => "Aux",
            StripKind::Group => "Group",
            StripKind::BusRight => "Bus (right)",
            StripKind::Master => "Master",
            StripKind::Monitoring => "Monitoring",
            StripKind::ControlGroup => "Control group",
            StripKind::Solo => "Solo",
            StripKind::Talkback => "Talkback",
            StripKind::Fx => "FX",
            StripKind::MatrixInput => "Matrix input",
            StripKind::Matrix => "Matrix",
            StripKind::GraphicEq => "Graphic EQ",
            StripKind::Node => "Nodal processing",
            StripKind::SpiceRack => "Spice Rack",
            StripKind::Generator => "Generator",
            StripKind::InputSocket => "Input socket",
            StripKind::OutputSocket => "Output socket",
            StripKind::Other => "Other",
        }
    }

    /// Strips that carry mixing state worth printing.
    pub fn is_mixing(self) -> bool {
        !matches!(
            self,
            StripKind::BusRight
                | StripKind::Generator
                | StripKind::InputSocket
                | StripKind::OutputSocket
                | StripKind::Other
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Strip {
    pub kind: StripKind,
    /// 0-based position within its kind, as stored in the file.
    pub index: u16,
    pub name: String,
    pub stereo: bool,
}

impl Strip {
    pub fn title(&self) -> String {
        if self.name.trim().is_empty() {
            format!("{} {}", self.kind.label(), self.index + 1)
        } else {
            self.name.clone()
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StripState {
    /// Index into `SessionDoc::strips`.
    pub strip: usize,
    pub params: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Param {
    /// Stable dotted key; the part before the first dot is the section.
    pub key: String,
    pub label: String,
    pub value: Value,
    /// `false` for values decoded from a field that is not yet confirmed.
    pub confirmed: bool,
}

impl Param {
    pub fn section(&self) -> &str {
        self.key.split('.').next().unwrap_or("")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Unit {
    Db,
    Hz,
    Seconds,
    Ratio,
    Percent,
    /// Pan / balance, −1 (left) … +1 (right).
    Pan,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Value {
    Num(f64, Unit),
    Bool(bool),
    /// One of a fixed set of named choices.
    Choice(String),
    Text(String),
    /// A reference to another strip, resolved to its name.
    Ref(String),
    /// An undecoded field, shown as stored.
    Raw(String),
}

impl Value {
    /// Human-readable rendering used by reports.
    pub fn display(&self) -> String {
        match self {
            Value::Num(v, unit) => format_num(*v, *unit),
            Value::Bool(b) => if *b { "on" } else { "off" }.to_string(),
            Value::Choice(s) | Value::Text(s) | Value::Ref(s) | Value::Raw(s) => s.clone(),
        }
    }
}

fn format_num(v: f64, unit: Unit) -> String {
    match unit {
        Unit::Db => {
            if v <= -149.0 {
                "−∞ dB".to_string()
            } else {
                format!("{v:.1} dB")
            }
        }
        Unit::Hz => {
            if v >= 1000.0 {
                format!("{:.2} kHz", v / 1000.0)
            } else {
                format!("{v:.1} Hz")
            }
        }
        Unit::Seconds => {
            if v < 0.001 {
                format!("{:.0} µs", v * 1_000_000.0)
            } else if v < 1.0 {
                format!("{:.1} ms", v * 1000.0)
            } else {
                format!("{v:.2} s")
            }
        }
        Unit::Ratio => format!("{v:.2}:1"),
        Unit::Percent => format!("{v:.0} %"),
        Unit::Pan => {
            if v.abs() < 0.005 {
                "C".to_string()
            } else if v < 0.0 {
                format!("L{:.0}", -v * 100.0)
            } else {
                format!("R{:.0}", v * 100.0)
            }
        }
        Unit::Plain => {
            if v.fract() == 0.0 {
                format!("{v:.0}")
            } else {
                format!("{v:.2}")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotDoc {
    /// Display number (SD: 7.5; S21: 1, 2, …).
    pub number: f64,
    pub name: String,
    /// 1-based index into `SessionDoc::snapshot_groups`, if grouped.
    pub group: Option<usize>,
    pub crossfade_s: Option<f64>,
    pub notes: String,
    /// S21: the snapshot holds edits that were not committed (`x != x_saved`).
    pub unsaved: bool,
    pub states: Vec<StripState>,
}

/// Small builder used by both readers.
#[derive(Default)]
pub struct ParamList(pub Vec<Param>);

impl ParamList {
    pub fn push(&mut self, key: impl Into<String>, label: impl Into<String>, value: Value) {
        self.0.push(Param {
            key: key.into(),
            label: label.into(),
            value,
            confirmed: true,
        });
    }

    pub fn push_unconfirmed(
        &mut self,
        key: impl Into<String>,
        label: impl Into<String>,
        value: Value,
    ) {
        self.0.push(Param {
            key: key.into(),
            label: label.into(),
            value,
            confirmed: false,
        });
    }

    pub fn num(&mut self, key: &str, label: &str, v: f64, unit: Unit) {
        self.push(key, label, Value::Num(v, unit));
    }

    pub fn flag(&mut self, key: &str, label: &str, on: bool) {
        self.push(key, label, Value::Bool(on));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_render_in_console_units() {
        assert_eq!(Value::Num(-150.0, Unit::Db).display(), "−∞ dB");
        assert_eq!(Value::Num(-2.54, Unit::Db).display(), "-2.5 dB");
        assert_eq!(Value::Num(28.44, Unit::Hz).display(), "28.4 Hz");
        assert_eq!(Value::Num(5023.77, Unit::Hz).display(), "5.02 kHz");
        assert_eq!(Value::Num(0.000_827, Unit::Seconds).display(), "827 µs");
        assert_eq!(Value::Num(0.2685, Unit::Seconds).display(), "268.5 ms");
        assert_eq!(Value::Num(-0.3092, Unit::Pan).display(), "L31");
        assert_eq!(Value::Num(0.0, Unit::Pan).display(), "C");
    }
}
