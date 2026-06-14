use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local};

/// Maximum number of log entries retained.
const MAX_LOG_ENTRIES: usize = 2000;

/// Direction of an OSC message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OscDirection {
    /// Message received from console / external source.
    In,
    /// Message sent to console / external target.
    Out,
}

impl fmt::Display for OscDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OscDirection::In => write!(f, "IN"),
            OscDirection::Out => write!(f, "OUT"),
        }
    }
}

/// Which protocol a logged message belongs to. The GP-OSC and DiGiCo iPad
/// links are colour-coded differently in the OSC Log so the operator can tell
/// the two exchanges apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OscProtocol {
    /// Generic-parameter OSC (the green IN / blue OUT link).
    #[default]
    GpOsc,
    /// DiGiCo iPad remote protocol (Modes 2/3).
    Ipad,
    /// External cue triggers — OSC to QLab / LiveProfessor / custom targets,
    /// and MIDI (CC / Note / Program Change). Colour-coded distinctly so the
    /// operator can tell show-control sends apart from console traffic.
    External,
}

/// A single logged OSC message.
#[derive(Clone, Debug)]
pub struct OscLogEntry {
    pub timestamp: DateTime<Local>,
    pub protocol: OscProtocol,
    pub direction: OscDirection,
    pub path: String,
    pub args: String,
}

impl OscLogEntry {
    /// GP-OSC entry (the default protocol).
    pub fn new(direction: OscDirection, path: String, args: String) -> Self {
        Self::with_protocol(OscProtocol::GpOsc, direction, path, args)
    }

    pub fn with_protocol(
        protocol: OscProtocol,
        direction: OscDirection,
        path: String,
        args: String,
    ) -> Self {
        Self {
            timestamp: Local::now(),
            protocol,
            direction,
            path,
            args,
        }
    }
}

/// Thread-safe, bounded ring buffer of OSC log entries.
/// Uses std::sync::Mutex (not tokio) so it can be locked from both sync UI and async tasks.
#[derive(Clone)]
pub struct OscLog {
    inner: Arc<Mutex<OscLogInner>>,
}

struct OscLogInner {
    entries: VecDeque<OscLogEntry>,
    /// Auto-scroll flag — when true the UI should scroll to the bottom.
    auto_scroll: bool,
    /// Monotonic counter so UI can detect new entries without diffing.
    revision: u64,
    /// Paused — when true, new entries are silently dropped.
    paused: bool,
}

impl OscLog {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(OscLogInner {
                entries: VecDeque::with_capacity(MAX_LOG_ENTRIES),
                auto_scroll: true,
                revision: 0,
                paused: false,
            })),
        }
    }

    /// Push a new log entry. Drops oldest if at capacity.
    pub fn push(&self, entry: OscLogEntry) {
        let mut inner = self.inner.lock().unwrap();
        if inner.paused {
            return;
        }
        if inner.entries.len() >= MAX_LOG_ENTRIES {
            inner.entries.pop_front();
        }
        inner.entries.push_back(entry);
        inner.revision += 1;
    }

    /// Convenience: log a sent message.
    pub fn log_out(&self, path: &str, args: &str) {
        self.push(OscLogEntry::new(
            OscDirection::Out,
            path.to_string(),
            args.to_string(),
        ));
    }

    /// Convenience: log a received message.
    pub fn log_in(&self, path: &str, args: &str) {
        self.push(OscLogEntry::new(
            OscDirection::In,
            path.to_string(),
            args.to_string(),
        ));
    }

    /// Convenience: log an iPad-protocol message heading to the console
    /// (iPad → Console).
    pub fn log_ipad_out(&self, path: &str, args: &str) {
        self.push(OscLogEntry::with_protocol(
            OscProtocol::Ipad,
            OscDirection::Out,
            path.to_string(),
            args.to_string(),
        ));
    }

    /// Convenience: log an iPad-protocol message heading to the iPad
    /// (Console → iPad).
    pub fn log_ipad_in(&self, path: &str, args: &str) {
        self.push(OscLogEntry::with_protocol(
            OscProtocol::Ipad,
            OscDirection::In,
            path.to_string(),
            args.to_string(),
        ));
    }

    /// Convenience: log an outbound external cue trigger (OSC to QLab /
    /// LiveProfessor / custom, or a MIDI message). `path` is the OSC address
    /// or a synthetic label like `"MIDI"`.
    pub fn log_external_out(&self, path: &str, args: &str) {
        self.push(OscLogEntry::with_protocol(
            OscProtocol::External,
            OscDirection::Out,
            path.to_string(),
            args.to_string(),
        ));
    }

    /// Get a snapshot of all current entries (for UI rendering).
    pub fn snapshot(&self) -> Vec<OscLogEntry> {
        let inner = self.inner.lock().unwrap();
        inner.entries.iter().cloned().collect()
    }

    /// Current revision number.
    pub fn revision(&self) -> u64 {
        self.inner.lock().unwrap().revision
    }

    /// Clear all entries.
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.entries.clear();
        inner.revision += 1;
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().entries.len()
    }

    pub fn auto_scroll(&self) -> bool {
        self.inner.lock().unwrap().auto_scroll
    }

    pub fn set_auto_scroll(&self, enabled: bool) {
        self.inner.lock().unwrap().auto_scroll = enabled;
    }

    pub fn paused(&self) -> bool {
        self.inner.lock().unwrap().paused
    }

    pub fn set_paused(&self, paused: bool) {
        self.inner.lock().unwrap().paused = paused;
    }
}
