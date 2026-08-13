//! Shared console write path.
//!
//! Sending a parameter to the desk is a two-protocol affair: GP OSC covers
//! the documented command set, and anything GP OSC can't express (analog
//! gain, phantom, GEQ, …) falls back to the reverse-engineered iPad
//! protocol. Every engine routes writes through [`ConsoleTx`] rather than
//! re-implementing the fallback — this is the single place where dialect
//! routing lives, and the seam where Pad-only console families (SD/Quantum)
//! plug in later.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::model::family::ConsoleProfile;
use crate::model::parameter::{ParameterAddress, ParameterValue};
use crate::osc::client::OscSender;
use crate::osc::encode;
use crate::osc::ipad_client::IpadSender;
use crate::osc::ipad_encode;
use tracing::{debug, warn};

/// How long a sent value counts as "ours" when its echo comes back.
/// Comfortably above a LAN round-trip; short enough that an operator
/// re-touching the same parameter isn't mistaken for an echo for long.
const SENT_LOG_WINDOW: Duration = Duration::from_millis(500);

/// Float tolerance when matching an echo against the sent value (the desk
/// may re-quantize floats). Matches the sidecar's echo tolerance.
const SENT_LOG_TOLERANCE: f32 = 0.01;

/// Shared log of recently-sent parameter values.
///
/// The iPad/Pad link echoes every write back at us (GP OSC does not). The
/// inbound side-effect chain (`console::inbound`) consults this log so an
/// echo of our own engine write is not mistaken for an operator move and
/// re-propagated through gangs / pan link / the automation-override
/// registry. Every [`ConsoleTx`] that shares this handle records into it on
/// each successful send.
#[derive(Clone, Default)]
pub struct SentLog {
    inner: Arc<std::sync::Mutex<SentLogInner>>,
}

#[derive(Default)]
struct SentLogInner {
    map: HashMap<ParameterAddress, (ParameterValue, Instant)>,
    /// When the map was last swept. Sweeping is rate-limited rather than run
    /// per send — see [`SentLog::note_sent`].
    last_prune: Option<Instant>,
}

impl SentLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successful send.
    ///
    /// Expired entries are swept at most once per [`SENT_LOG_WINDOW`], not on
    /// every send. Sweeping is only about bounding memory — [`Self::is_echo`]
    /// re-checks each entry's age, so a stale entry can never produce a false
    /// echo match. Sweeping per send made this O(n) on the outbound hot path,
    /// which a full-scope recall (~11k parameters, sent unpaced) turned into
    /// quadratic work: measurably ~250 ms of added CPU, worse on the ARM
    /// single-board targets headless mode is built for.
    pub fn note_sent(&self, addr: &ParameterAddress, value: &ParameterValue) {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        let due = inner
            .last_prune
            .is_none_or(|at| now.duration_since(at) >= SENT_LOG_WINDOW);
        if due {
            inner
                .map
                .retain(|_, (_, at)| now.duration_since(*at) < SENT_LOG_WINDOW);
            inner.last_prune = Some(now);
        }
        inner.map.insert(addr.clone(), (value.clone(), now));
    }

    /// True if `value` matches a recently-sent value for `addr` — i.e. this
    /// inbound message is (almost certainly) the console echoing our own
    /// write back. The entry is kept until it expires: the same echo can be
    /// observed more than once (e.g. Mode 3 sees both proxy directions).
    pub fn is_echo(&self, addr: &ParameterAddress, value: &ParameterValue) -> bool {
        let now = Instant::now();
        let inner = self.inner.lock().unwrap();
        match inner.map.get(addr) {
            Some((sent, at)) if now.duration_since(*at) < SENT_LOG_WINDOW => {
                values_match(sent, value)
            }
            _ => false,
        }
    }

    /// Number of retained entries. Test-only visibility into the sweep.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().unwrap().map.len()
    }
}

fn values_match(a: &ParameterValue, b: &ParameterValue) -> bool {
    match (a, b) {
        (ParameterValue::Float(x), ParameterValue::Float(y)) => (x - y).abs() <= SENT_LOG_TOLERANCE,
        _ => a == b,
    }
}

/// What happened to one parameter write. Engines that keep their own
/// bookkeeping (the snapshot engine's sent/skipped counters) match on this;
/// engines that just want a log line use [`ConsoleTx::send_parameter_logged`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendOutcome {
    /// Encoded as GP OSC and handed to the socket.
    SentGp,
    /// GP OSC can't express it; encoded via the iPad protocol and handed
    /// to the socket.
    SentPad,
    /// GP OSC encoded it but the socket send failed.
    GpSendFailed,
    /// The iPad dialect encoded it but the socket send failed.
    PadSendFailed,
    /// iPad-only parameter and no iPad sender is wired (Mode 1).
    NoPadSender,
    /// Neither dialect can encode this parameter.
    NoEncoding,
}

impl SendOutcome {
    /// True if the message was handed to a sender successfully.
    pub fn is_sent(self) -> bool {
        matches!(self, SendOutcome::SentGp | SendOutcome::SentPad)
    }
}

/// One handle owning both console senders. Clone-cheap (both senders are
/// `Arc`-backed), so spawned tasks (timed recalls, fades) take a clone.
#[derive(Clone)]
pub struct ConsoleTx {
    gp: OscSender,
    pad: Option<IpadSender>,
    /// Shared echo log — see [`SentLog`]. `None` until wired (tests, or a
    /// GP-only session where no echoes exist).
    sent_log: Option<SentLog>,
    /// The connected console's profile, supplying the Pad wire quirks used to
    /// encode. Defaults to the S-series profile so an unwired handle behaves
    /// exactly as before.
    profile: Arc<ConsoleProfile>,
}

impl ConsoleTx {
    /// GP OSC only — the iPad sender is wired in later (Mode 2/3) via
    /// [`ConsoleTx::set_pad_sender`].
    pub fn new(gp: OscSender) -> Self {
        Self {
            gp,
            pad: None,
            sent_log: None,
            profile: Arc::new(ConsoleProfile::default()),
        }
    }

    pub fn with_pad(gp: OscSender, pad: Option<IpadSender>) -> Self {
        Self {
            gp,
            pad,
            sent_log: None,
            profile: Arc::new(ConsoleProfile::default()),
        }
    }

    /// Wire (or unwire) the iPad-protocol sender.
    pub fn set_pad_sender(&mut self, pad: Option<IpadSender>) {
        self.pad = pad;
    }

    /// Attach the shared sent-value log so this handle's writes can be
    /// recognized when the console echoes them back (see [`SentLog`]).
    pub fn set_sent_log(&mut self, log: SentLog) {
        self.sent_log = Some(log);
    }

    /// Point this handle at the connected console's profile, so writes use
    /// that family's Pad wire quirks.
    pub fn set_profile(&mut self, profile: Arc<ConsoleProfile>) {
        self.profile = profile;
    }

    /// The profile currently in force.
    pub fn profile(&self) -> &ConsoleProfile {
        &self.profile
    }

    /// Whether an iPad-protocol sender is wired (Mode 2/3).
    pub fn has_pad(&self) -> bool {
        self.pad.is_some()
    }

    /// The GP OSC sender, for system commands (`/console/resend`, snapshot
    /// fire, ping/pong) that have no typed-parameter form yet. Escape hatch
    /// until system commands become dialect-mapped.
    pub fn gp_sender(&self) -> &OscSender {
        &self.gp
    }

    pub fn pad_sender(&self) -> Option<&IpadSender> {
        self.pad.as_ref()
    }

    /// Send a parameter to the console: GP OSC first, iPad protocol as the
    /// fallback for parameters GP OSC can't encode. No logging — callers
    /// either match on the outcome or use [`Self::send_parameter_logged`].
    pub async fn send_parameter(
        &self,
        addr: &ParameterAddress,
        value: &ParameterValue,
    ) -> SendOutcome {
        let outcome = self.send_parameter_inner(addr, value).await;
        if outcome.is_sent() {
            if let Some(log) = &self.sent_log {
                log.note_sent(addr, value);
            }
        }
        outcome
    }

    async fn send_parameter_inner(
        &self,
        addr: &ParameterAddress,
        value: &ParameterValue,
    ) -> SendOutcome {
        match encode::encode_parameter(addr, value) {
            Some((path, args)) => match self.gp.send(&path, args).await {
                Ok(()) => SendOutcome::SentGp,
                Err(e) => {
                    debug!(%addr, "GP OSC socket send failed: {e}");
                    SendOutcome::GpSendFailed
                }
            },
            None => match &self.pad {
                Some(pad) => {
                    match ipad_encode::encode_pad_parameter(addr, value, &self.profile.pad_quirks) {
                        Some((path, args)) => match pad.send(&path, args).await {
                            Ok(()) => SendOutcome::SentPad,
                            Err(e) => {
                                debug!(%addr, "iPad socket send failed: {e}");
                                SendOutcome::PadSendFailed
                            }
                        },
                        None => SendOutcome::NoEncoding,
                    }
                }
                None => SendOutcome::NoPadSender,
            },
        }
    }

    /// [`Self::send_parameter`] plus the standard log lines, prefixed with
    /// `ctx` (the engine name). Returns true if the message was handed to a
    /// sender successfully.
    pub async fn send_parameter_logged(
        &self,
        ctx: &'static str,
        addr: &ParameterAddress,
        value: &ParameterValue,
    ) -> bool {
        let outcome = self.send_parameter(addr, value).await;
        match outcome {
            SendOutcome::SentGp | SendOutcome::SentPad => {}
            SendOutcome::GpSendFailed => warn!(%addr, "{ctx}: GP OSC send failed"),
            SendOutcome::PadSendFailed => warn!(%addr, "{ctx}: iPad send failed"),
            SendOutcome::NoEncoding => {
                warn!(%addr, "{ctx}: cannot encode parameter for either protocol")
            }
            SendOutcome::NoPadSender => {
                debug!(%addr, "{ctx}: iPad-only parameter, no iPad sender wired")
            }
        }
        outcome.is_sent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::ParameterPath;
    use crate::osc::ipad_client::IpadClient;

    async fn gp_sender() -> OscSender {
        let client = crate::osc::client::OscClient::new(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:1".parse().unwrap(),
            None,
        )
        .await
        .unwrap();
        let (sender, _rx) = client.into_parts();
        sender
    }

    async fn pad_sender() -> IpadSender {
        let client = IpadClient::new(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:1".parse().unwrap(),
            None,
        )
        .await
        .unwrap();
        let (sender, _rx) = client.into_parts();
        sender
    }

    fn gp_addr() -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        }
    }

    fn pad_only_addr() -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::AnalogGain,
        }
    }

    #[tokio::test]
    async fn gp_covered_param_sends_via_gp() {
        let tx = ConsoleTx::new(gp_sender().await);
        let outcome = tx
            .send_parameter(&gp_addr(), &ParameterValue::Float(-10.0))
            .await;
        assert_eq!(outcome, SendOutcome::SentGp);
    }

    #[tokio::test]
    async fn pad_only_param_without_pad_sender_reports_no_pad() {
        let tx = ConsoleTx::new(gp_sender().await);
        let outcome = tx
            .send_parameter(&pad_only_addr(), &ParameterValue::Float(0.0))
            .await;
        assert_eq!(outcome, SendOutcome::NoPadSender);
        assert!(!outcome.is_sent());
    }

    #[tokio::test]
    async fn pad_only_param_with_pad_sender_sends_via_pad() {
        let tx = ConsoleTx::with_pad(gp_sender().await, Some(pad_sender().await));
        let outcome = tx
            .send_parameter(&pad_only_addr(), &ParameterValue::Float(0.0))
            .await;
        assert_eq!(outcome, SendOutcome::SentPad);
    }

    // ── SentLog ─────────────────────────────────────────────────────────

    fn fader(ch: u16) -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(ch),
            parameter: ParameterPath::Fader,
        }
    }

    #[test]
    fn sent_log_matches_an_echo_within_float_tolerance() {
        let log = SentLog::new();
        log.note_sent(&fader(1), &ParameterValue::Float(-10.0));
        // The desk re-quantizes floats slightly; that is still our echo.
        assert!(log.is_echo(&fader(1), &ParameterValue::Float(-10.005)));
        // A move beyond the tolerance is an operator move, not an echo.
        assert!(!log.is_echo(&fader(1), &ParameterValue::Float(-10.5)));
        // A different address is never our echo.
        assert!(!log.is_echo(&fader(2), &ParameterValue::Float(-10.0)));
    }

    #[test]
    fn sent_log_entries_expire_after_the_window() {
        let log = SentLog::new();
        log.note_sent(&fader(1), &ParameterValue::Float(-10.0));
        assert!(log.is_echo(&fader(1), &ParameterValue::Float(-10.0)));
        std::thread::sleep(SENT_LOG_WINDOW + Duration::from_millis(60));
        // Past the window the same value is an operator move again.
        assert!(!log.is_echo(&fader(1), &ParameterValue::Float(-10.0)));
    }

    #[test]
    fn sent_log_sweep_is_rate_limited_not_per_send() {
        // A full-scope recall bursts thousands of sends inside one window.
        // None of them may trigger a sweep after the first, and every one
        // must still be recognized as our echo.
        let log = SentLog::new();
        for ch in 1..=2000u16 {
            log.note_sent(&fader(ch), &ParameterValue::Float(-10.0));
        }
        assert_eq!(
            log.len(),
            2000,
            "nothing sent inside the window may be swept"
        );
        assert!(log.is_echo(&fader(1), &ParameterValue::Float(-10.0)));
        assert!(log.is_echo(&fader(2000), &ParameterValue::Float(-10.0)));
    }

    #[tokio::test]
    async fn default_profile_encodes_with_s21_quirks() {
        let tx = ConsoleTx::with_pad(gp_sender().await, Some(pad_sender().await));
        assert_eq!(
            tx.profile().pad_quirks,
            crate::model::family::PadQuirks::S21
        );
    }

    #[tokio::test]
    async fn profile_quirks_change_the_encoded_path() {
        use crate::model::family::{ConsoleProfile, PadQuirks};

        // CG 1 encodes as wire 0 under the S21 quirks…
        let addr = ParameterAddress {
            channel: ChannelId::ControlGroup(1),
            parameter: ParameterPath::CgLevel,
        };
        let s21 = ConsoleTx::with_pad(gp_sender().await, Some(pad_sender().await));
        let (path, _) = crate::osc::ipad_encode::encode_pad_parameter(
            &addr,
            &ParameterValue::Int(0),
            &s21.profile().pad_quirks,
        )
        .unwrap();
        assert_eq!(path, "/Control_Groups/0/CGs_level");

        // …and as wire 1 once the handle carries a profile without that quirk.
        let mut tx = ConsoleTx::with_pad(gp_sender().await, Some(pad_sender().await));
        let mut profile = ConsoleProfile::default();
        profile.pad_quirks = PadQuirks {
            control_groups_zero_based: false,
            ..PadQuirks::S21
        };
        tx.set_profile(Arc::new(profile));
        let (path, _) = crate::osc::ipad_encode::encode_pad_parameter(
            &addr,
            &ParameterValue::Int(0),
            &tx.profile().pad_quirks,
        )
        .unwrap();
        assert_eq!(path, "/Control_Groups/1/CGs_level");
    }

    #[tokio::test]
    async fn unwiring_pad_sender_restores_no_pad() {
        let mut tx = ConsoleTx::with_pad(gp_sender().await, Some(pad_sender().await));
        tx.set_pad_sender(None);
        assert!(!tx.has_pad());
        let outcome = tx
            .send_parameter(&pad_only_addr(), &ParameterValue::Float(0.0))
            .await;
        assert_eq!(outcome, SendOutcome::NoPadSender);
    }
}
