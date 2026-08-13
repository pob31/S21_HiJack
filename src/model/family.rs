//! Console family, wire quirks, and derived console profile.
//!
//! The app speaks two DiGiCo dialects: the S-series "GP OSC" command set and
//! the reverse-engineered "Pad" protocol (the S21 iPad app on S-series; the
//! "DiGiCo Pad" external-control device on the SD and Quantum ranges). The
//! Pad address tree is shared across all three families — what differs is a
//! handful of wire-level quirks plus the connection lifecycle.
//!
//! Rather than branch on a console model everywhere, those differences live
//! here as **data**: pick a [`ConsoleFamily`], derive a [`ConsoleProfile`],
//! and pass its [`PadQuirks`] to the codec. Capacities (channel counts, bus
//! modes) are NOT in the profile — those are discovered at runtime from the
//! console itself and live in
//! [`ConsoleConfig`](crate::model::config::ConsoleConfig). That split is what
//! lets one profile cover a whole console range whose models differ in size
//! and licensed channel count.
//!
//! **Verification status:** the S-series values are confirmed against a real
//! S21. The SD/Quantum values are hypotheses drawn from community Pad-protocol
//! implementations and are flagged as such on each constant; they are meant to
//! be corrected from a hardware probe, and every one of them is overridable at
//! runtime via [`ConsoleConfig::pad_quirk_overrides`](crate::model::config::ConsoleConfig).

use serde::{Deserialize, Serialize};

/// Which DiGiCo console range we're talking to.
///
/// Persisted in the show file (via `ConsoleConfig`) and defaulted to
/// [`ConsoleFamily::SSeries`] so every pre-existing show keeps its behaviour.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum ConsoleFamily {
    /// S21 / S31 — GP OSC plus the iPad protocol. Quirks hardware-verified.
    #[default]
    SSeries,
    /// SD range (SD7/SD9/SD10/SD12/…) — "DiGiCo Pad" device, no GP OSC.
    SdRange,
    /// Quantum range (Quantum 225/338/5/7/852/…) — "DiGiCo Pad" device.
    Quantum,
}

impl ConsoleFamily {
    pub fn label(&self) -> &'static str {
        match self {
            ConsoleFamily::SSeries => "S Series",
            ConsoleFamily::SdRange => "SD Range",
            ConsoleFamily::Quantum => "Quantum",
        }
    }

    /// All families, for UI pickers and exhaustive tests.
    pub const ALL: [ConsoleFamily; 3] = [
        ConsoleFamily::SSeries,
        ConsoleFamily::SdRange,
        ConsoleFamily::Quantum,
    ];
}

/// How a boolean is represented on the Pad wire.
///
/// The S21 iPad protocol sends floats (1.0 / 0.0) rather than OSC's own
/// boolean type tags. Other families may differ — inbound parsing accepts all
/// three forms regardless (see `ipad_parse::extract_bool`), so this only
/// affects what we *send*.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BoolWire {
    /// OSC 1.1 `T` / `F` type tags.
    OscBool,
    /// Float 1.0 / 0.0 — S21-verified.
    Float01,
    /// Int 1 / 0.
    Int01,
}

/// How pan position is represented on the Pad wire. Internally pan is always
/// `-1.0 ..= +1.0` with 0.0 centre.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PanWire {
    /// `0.0 ..= 1.0` with 0.5 centre — S21-verified.
    ZeroToOne,
    /// Same as internal: `-1.0 ..= +1.0`.
    SignedUnit,
}

/// Wire-level quirks of the Pad dialect for one console family.
///
/// Every field defaults to its S21 value so a partially-specified override in
/// a hand-edited show file only changes what it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PadQuirks {
    /// EQ band indices run backwards on the wire (internal band 1 = wire band
    /// 4). An S21 firmware artifact; assumed absent elsewhere.
    #[serde(default = "default_true")]
    pub eq_bands_reversed: bool,
    /// Multiband dynamics 1 Mid and High bands are swapped on the wire
    /// (internal 2 ↔ wire 3). An S21 firmware artifact.
    #[serde(default = "default_true")]
    pub dyn1_mid_high_swapped: bool,
    /// Outbound boolean encoding.
    #[serde(default = "default_bool_wire")]
    pub bool_wire: BoolWire,
    /// Pan value range on the wire.
    #[serde(default = "default_pan_wire")]
    pub pan_wire: PanWire,
    /// Control Groups are numbered from 0 on the wire (internal CG 1 = wire
    /// 0). Everything else is 1-based.
    #[serde(default = "default_true")]
    pub control_groups_zero_based: bool,
}

fn default_true() -> bool {
    true
}
fn default_bool_wire() -> BoolWire {
    BoolWire::Float01
}
fn default_pan_wire() -> PanWire {
    PanWire::ZeroToOne
}

impl PadQuirks {
    /// S21/S31 — every value confirmed against a real desk.
    pub const S21: PadQuirks = PadQuirks {
        eq_bands_reversed: true,
        dyn1_mid_high_swapped: true,
        bool_wire: BoolWire::Float01,
        pan_wire: PanWire::ZeroToOne,
        control_groups_zero_based: true,
    };

    /// SD/Quantum — **hypothesis, not verified.**
    ///
    /// Reasoning: the band reversal and the multiband swap look like S21
    /// firmware artifacts (no other DiGiCo implementation reports them), so
    /// they're assumed absent. The value-encoding conventions (float booleans,
    /// 0..1 pan, 0-based Control Groups) come from the shared Pad-app lineage
    /// and are assumed to carry over. Confirm each with the Phase 2 hardware
    /// probe before trusting recall on these desks.
    pub const SD_HYPOTHESIS: PadQuirks = PadQuirks {
        eq_bands_reversed: false,
        dyn1_mid_high_swapped: false,
        bool_wire: BoolWire::Float01,
        pan_wire: PanWire::ZeroToOne,
        control_groups_zero_based: true,
    };
}

impl Default for PadQuirks {
    fn default() -> Self {
        Self::S21
    }
}

/// How level parameters are scaled on the wire.
///
/// Distinct from [`FaderLaw`], which describes a *physical* fader's travel
/// taper for the sidecar. This is purely the number in the OSC message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LevelWire {
    /// The wire carries dB directly (S-series GP OSC and the Pad dialect).
    Db,
    /// The wire carries a normalized `0.0 ..= 1.0` position, as DiGiCo's
    /// "Other OSC" list specifies (`fader, f, 1` = maximum).
    Normalized01,
}

/// A control surface the console exposes — one OSC dialect on one connection.
///
/// A console family may offer more than one. They differ in how the address is
/// spelled and how values are scaled, but the SD "Other OSC" and Pad surfaces
/// share the same underlying TitleCase parameter tree, so one codec serves all
/// three (see `ParameterPath::to_pad_suffix`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConsoleSurface {
    /// S-series general-purpose OSC: flat `/channel/{n}/{suffix}`, dB values,
    /// documented bidirectional feedback (`/console/resend`, ping/pong).
    SSeriesGp,
    /// SD/Quantum "Other OSC": `/sd/` + the TitleCase tree, normalized values.
    /// Documented by DiGiCo as send-only, but a field report has `/?` queries
    /// working; whether it *pushes* surface moves is the Phase 2a probe's
    /// decisive question.
    SdOther,
    /// The DiGiCo Pad (iPad) protocol: the bare TitleCase tree, dB values,
    /// `/?` queries and push feedback. Reverse-engineered, and the console
    /// accepts only one Pad device at a time.
    Pad,
}

impl ConsoleSurface {
    /// Literal prepended to every address on this surface (no trailing slash).
    pub fn path_prefix(self) -> &'static str {
        match self {
            ConsoleSurface::SdOther => "/sd",
            ConsoleSurface::SSeriesGp | ConsoleSurface::Pad => "",
        }
    }

    /// How level values are scaled on this surface.
    pub fn level_wire(self) -> LevelWire {
        match self {
            ConsoleSurface::SdOther => LevelWire::Normalized01,
            ConsoleSurface::SSeriesGp | ConsoleSurface::Pad => LevelWire::Db,
        }
    }

    /// Whether this surface addresses parameters through the TitleCase tree
    /// (`/Input_Channels/1/…`) that the Pad codec emits. False only for the
    /// S-series flat `/channel/{n}/…` numbering.
    pub fn uses_pad_tree(self) -> bool {
        matches!(self, ConsoleSurface::SdOther | ConsoleSurface::Pad)
    }

    pub fn label(self) -> &'static str {
        match self {
            ConsoleSurface::SSeriesGp => "GP OSC",
            ConsoleSurface::SdOther => "Other OSC (/sd)",
            ConsoleSurface::Pad => "DiGiCo Pad",
        }
    }
}

/// The complete wire dialect for one connection: which surface is in use, plus
/// the quirk flags believed to apply to that console.
///
/// Travels together through the codec because both are needed to render an
/// address and a value: the surface decides the path prefix and level scaling,
/// the quirks decide band ordering and value encodings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PadWire {
    pub surface: ConsoleSurface,
    pub quirks: PadQuirks,
}

impl PadWire {
    /// The hardware-verified S21 dialect on the Pad surface — the shape every
    /// pre-existing test pins.
    pub const S21: PadWire = PadWire {
        surface: ConsoleSurface::Pad,
        quirks: PadQuirks::S21,
    };

    pub fn new(surface: ConsoleSurface, quirks: PadQuirks) -> Self {
        Self { surface, quirks }
    }
}

impl Default for PadWire {
    fn default() -> Self {
        Self::S21
    }
}

/// How fader positions relate to the value on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FaderLaw {
    /// The wire carries dB directly (S21-verified; SD/Quantum assumed).
    RawDb,
    /// The wire carries a normalized `0.0 ..= 1.0` position.
    Normalized01,
}

/// How link health is probed. Consumed in Phase 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HeartbeatStrategy {
    /// GP OSC `/console/ping` → `/console/pong`.
    GpPingPong,
    /// Pad dialect: periodic `/Console/Name/?` and watch for the reply.
    ConsoleNameQuery,
}

/// How the state mirror is initially populated. Consumed in Phase 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EnumerationStrategy {
    /// GP OSC `/console/resend` — the desk floods every parameter.
    GpResendDump,
    /// Pad dialect: per-parameter `/?` queries, paced one-in-flight (some
    /// desks drop query floods).
    PacedPadQueries,
}

/// App capabilities that depend on the console family.
///
/// Deliberately minimal — a variant is added when a consumer exists, so the
/// enum never lists a feature nothing gates on. Phase 2 grows this as the
/// per-family tab gating lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppFeature {
    /// The recall-scope / channel-safe editor, whose block layout mirrors the
    /// S-series console screen.
    RecallScopeUi,
}

/// Everything family-dependent, derived from a [`ConsoleFamily`] plus any
/// operator overrides.
///
/// Never persisted directly — the show file stores the family and the quirk
/// overrides, and this is rebuilt from them. That keeps a stored show from
/// pinning stale defaults when a hardware probe corrects a hypothesis.
#[derive(Clone, Debug, PartialEq)]
pub struct ConsoleProfile {
    pub family: ConsoleFamily,
    /// Every surface this console exposes, most preferred first. The first
    /// entry is what a connection uses unless the operator overrides it.
    pub surfaces: &'static [ConsoleSurface],
    pub pad_quirks: PadQuirks,
    pub fader_law: FaderLaw,
    pub heartbeat: HeartbeatStrategy,
    pub enumeration: EnumerationStrategy,
    /// Conventional port the console listens on for Pad traffic.
    pub default_pad_send_port: u16,
    /// Conventional port the console sends Pad traffic to.
    pub default_pad_receive_port: u16,
    /// Valid send-level range in dB, or `None` to leave levels unclamped.
    /// `None` preserves the S-series behaviour of trusting the desk's own
    /// range (see `ParameterPath::clamp_value`).
    pub send_level_db_range: Option<(f32, f32)>,
}

impl ConsoleProfile {
    /// The stock profile for a family, before operator overrides.
    pub fn for_family(family: ConsoleFamily) -> Self {
        match family {
            ConsoleFamily::SSeries => Self {
                family,
                // GP OSC first: it is the documented, feedback-capable surface
                // and every S-series feature has been built on it.
                surfaces: &[ConsoleSurface::SSeriesGp, ConsoleSurface::Pad],
                pad_quirks: PadQuirks::S21,
                fader_law: FaderLaw::RawDb,
                heartbeat: HeartbeatStrategy::GpPingPong,
                enumeration: EnumerationStrategy::GpResendDump,
                // The S21's iPad port is operator-supplied (no fixed default
                // on the desk); 8001 matches the app's CLI default.
                default_pad_send_port: 8001,
                default_pad_receive_port: 8001,
                send_level_db_range: None,
            },
            // SD and Quantum share a profile shape today; they are separate
            // arms so a hardware probe can split them without restructuring.
            ConsoleFamily::SdRange | ConsoleFamily::Quantum => Self {
                family,
                // Pad first, for now: it is the surface with positive evidence
                // of push feedback, which the live state mirror requires.
                // `SdOther` is DiGiCo's documented general-purpose interface
                // and would be preferable — no one-device limit, no reverse
                // engineering — but whether it reports the desk's own surface
                // moves is unverified. Hardware session #1 may reorder these.
                surfaces: &[ConsoleSurface::Pad, ConsoleSurface::SdOther],
                pad_quirks: PadQuirks::SD_HYPOTHESIS,
                fader_law: FaderLaw::RawDb,
                heartbeat: HeartbeatStrategy::ConsoleNameQuery,
                enumeration: EnumerationStrategy::PacedPadQueries,
                // Conventional External Control ports: the console receives on
                // 8000 and sends to 9000. HYPOTHESIS — operator-configurable
                // on the desk, so these are only first-run defaults.
                default_pad_send_port: 8000,
                default_pad_receive_port: 9000,
                // HYPOTHESIS: community implementations report roughly
                // -90..+10 dB on send levels; the low end is widened to the
                // app's -inf sentinel region so a legitimate "off" survives.
                send_level_db_range: Some((-140.0, 10.0)),
            },
        }
    }

    /// Replace the quirks **wholesale** with an operator override, if present.
    ///
    /// Note this is a replacement, not a field-wise merge onto the family
    /// stock. A hand-written override in a show file names its fields against
    /// [`PadQuirks`]'s own serde defaults, which are the S21 values — so a
    /// partial override on an SD/Quantum show inherits S21 values for the
    /// fields it omits, not [`PadQuirks::SD_HYPOTHESIS`] ones. Build overrides
    /// from the family's constant (`PadQuirks { eq_bands_reversed: true,
    /// ..PadQuirks::SD_HYPOTHESIS }`) rather than naming one field alone.
    pub fn with_overrides(mut self, overrides: Option<PadQuirks>) -> Self {
        if let Some(q) = overrides {
            self.pad_quirks = q;
        }
        self
    }

    pub fn supports(&self, feature: AppFeature) -> bool {
        match feature {
            AppFeature::RecallScopeUi => self.family == ConsoleFamily::SSeries,
        }
    }

    /// Whether this console exposes `surface` at all.
    pub fn has_surface(&self, surface: ConsoleSurface) -> bool {
        self.surfaces.contains(&surface)
    }

    /// The surface a connection uses by default.
    ///
    /// Every family declares at least one, so this cannot fail in practice;
    /// it falls back to [`ConsoleSurface::Pad`] rather than panicking, since
    /// the Pad tree is the one all families share.
    pub fn primary_surface(&self) -> ConsoleSurface {
        self.surfaces
            .first()
            .copied()
            .unwrap_or(ConsoleSurface::Pad)
    }
}

impl Default for ConsoleProfile {
    fn default() -> Self {
        Self::for_family(ConsoleFamily::default())
    }
}

/// What a connection actually establishes, derived from the console family and
/// the operator's chosen [`OperatingMode`](crate::model::operating_mode::OperatingMode).
///
/// The mode enum predates multi-family support and is phrased in S-series
/// terms, so this translates it: on a Pad-only console "Mode 1" (GP OSC only)
/// is meaningless, and every mode resolves to a Pad connection instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionScheme {
    /// S-series, GP OSC alone.
    GpOnly,
    /// S-series, GP OSC plus a direct Pad link (Mode 2).
    GpPlusPadDirect,
    /// S-series, GP OSC plus the Pad proxy sitting in front of a real iPad
    /// (Mode 3).
    GpPlusPadProxy,
    /// No GP OSC dialect exists — a single Pad connection carries everything.
    PadDirect,
}

impl ConnectionScheme {
    /// Resolve the scheme for a family and mode.
    pub fn resolve(
        family: ConsoleFamily,
        mode: crate::model::operating_mode::OperatingMode,
    ) -> Self {
        use crate::model::operating_mode::OperatingMode as M;
        match family {
            // SD/Quantum have no `/channel/{n}/…` dialect at all, so the mode
            // selector simply does not apply; they always connect over Pad.
            ConsoleFamily::SdRange | ConsoleFamily::Quantum => ConnectionScheme::PadDirect,
            ConsoleFamily::SSeries => match mode {
                M::Mode1 => ConnectionScheme::GpOnly,
                M::Mode2 => ConnectionScheme::GpPlusPadDirect,
                M::Mode3 => ConnectionScheme::GpPlusPadProxy,
            },
        }
    }

    /// Whether this scheme binds a GP OSC socket and runs the GP loop.
    pub fn uses_gp(self) -> bool {
        !matches!(self, ConnectionScheme::PadDirect)
    }

    /// Whether the operator's iPad-port settings are needed.
    pub fn uses_pad(self) -> bool {
        !matches!(self, ConnectionScheme::GpOnly)
    }
}

/// How well a parameter is known to work on a given console family.
///
/// `Assumed` is the backlog for the hardware-verification probe: reachable in
/// principle, not yet confirmed on a real desk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamSupport {
    /// Confirmed against real hardware.
    Verified,
    /// Believed reachable; unconfirmed.
    Assumed,
    /// Not reachable on this family — never queried, never sent, hidden in UI.
    Unsupported,
}

impl ParamSupport {
    /// Whether the parameter may be used at all on this family.
    pub fn is_usable(self) -> bool {
        !matches!(self, ParamSupport::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s21_quirks_are_the_default() {
        assert_eq!(PadQuirks::default(), PadQuirks::S21);
        assert_eq!(ConsoleFamily::default(), ConsoleFamily::SSeries);
        assert_eq!(ConsoleProfile::default().pad_quirks, PadQuirks::S21);
    }

    #[test]
    fn s_series_profile_matches_verified_hardware_behaviour() {
        let p = ConsoleProfile::for_family(ConsoleFamily::SSeries);
        assert!(p.has_surface(ConsoleSurface::SSeriesGp));
        assert_eq!(p.primary_surface(), ConsoleSurface::SSeriesGp);
        assert_eq!(p.pad_quirks, PadQuirks::S21);
        assert_eq!(p.heartbeat, HeartbeatStrategy::GpPingPong);
        assert_eq!(p.enumeration, EnumerationStrategy::GpResendDump);
        // Levels stay unclamped on S-series — the desk's range is trusted.
        assert_eq!(p.send_level_db_range, None);
    }

    #[test]
    fn sd_families_offer_pad_and_other_osc_but_not_s_series_gp() {
        for family in [ConsoleFamily::SdRange, ConsoleFamily::Quantum] {
            let p = ConsoleProfile::for_family(family);
            assert!(
                !p.has_surface(ConsoleSurface::SSeriesGp),
                "{family:?} must not claim the S-series GP OSC dialect"
            );
            assert!(p.has_surface(ConsoleSurface::SdOther));
            assert!(p.has_surface(ConsoleSurface::Pad));
            // Pad leads until the probe shows /sd/ pushes surface moves.
            assert_eq!(p.primary_surface(), ConsoleSurface::Pad);
            assert_eq!(p.heartbeat, HeartbeatStrategy::ConsoleNameQuery);
            assert_eq!(p.enumeration, EnumerationStrategy::PacedPadQueries);
            assert_eq!(p.pad_quirks, PadQuirks::SD_HYPOTHESIS);
        }
    }

    #[test]
    fn overrides_replace_quirks_and_nothing_else() {
        let base = ConsoleProfile::for_family(ConsoleFamily::Quantum);
        let custom = PadQuirks {
            eq_bands_reversed: true,
            ..PadQuirks::SD_HYPOTHESIS
        };
        let overridden = base.clone().with_overrides(Some(custom));
        assert_eq!(overridden.pad_quirks, custom);
        assert_eq!(overridden.heartbeat, base.heartbeat);
        assert_eq!(overridden.family, base.family);
        // No override → untouched.
        assert_eq!(base.clone().with_overrides(None), base);
    }

    #[test]
    fn feature_support_by_family() {
        let s = ConsoleProfile::for_family(ConsoleFamily::SSeries);
        assert!(s.supports(AppFeature::RecallScopeUi));
        for family in [ConsoleFamily::SdRange, ConsoleFamily::Quantum] {
            let p = ConsoleProfile::for_family(family);
            assert!(!p.supports(AppFeature::RecallScopeUi));
        }
    }

    #[test]
    fn connection_scheme_resolution() {
        use crate::model::operating_mode::OperatingMode as M;
        // S-series keeps every mode meaning exactly what it always meant.
        assert_eq!(
            ConnectionScheme::resolve(ConsoleFamily::SSeries, M::Mode1),
            ConnectionScheme::GpOnly
        );
        assert_eq!(
            ConnectionScheme::resolve(ConsoleFamily::SSeries, M::Mode2),
            ConnectionScheme::GpPlusPadDirect
        );
        assert_eq!(
            ConnectionScheme::resolve(ConsoleFamily::SSeries, M::Mode3),
            ConnectionScheme::GpPlusPadProxy
        );
        // Pad-only families ignore the mode entirely — including Mode 1,
        // which would otherwise mean "GP OSC only" on a desk with no GP OSC.
        for family in [ConsoleFamily::SdRange, ConsoleFamily::Quantum] {
            for mode in [M::Mode1, M::Mode2, M::Mode3] {
                assert_eq!(
                    ConnectionScheme::resolve(family, mode),
                    ConnectionScheme::PadDirect,
                    "{family:?} + {mode:?}"
                );
            }
        }
    }

    #[test]
    fn connection_scheme_capabilities() {
        assert!(ConnectionScheme::GpOnly.uses_gp());
        assert!(!ConnectionScheme::GpOnly.uses_pad());
        assert!(ConnectionScheme::GpPlusPadDirect.uses_gp());
        assert!(ConnectionScheme::GpPlusPadDirect.uses_pad());
        // The one that matters: a Pad-only console binds no GP socket.
        assert!(!ConnectionScheme::PadDirect.uses_gp());
        assert!(ConnectionScheme::PadDirect.uses_pad());
    }

    #[test]
    fn surface_wire_conventions() {
        // The /sd/ surface is the only prefixed one, and the only one whose
        // levels are normalized rather than dB.
        assert_eq!(ConsoleSurface::SdOther.path_prefix(), "/sd");
        assert_eq!(ConsoleSurface::Pad.path_prefix(), "");
        assert_eq!(ConsoleSurface::SSeriesGp.path_prefix(), "");
        assert_eq!(
            ConsoleSurface::SdOther.level_wire(),
            LevelWire::Normalized01
        );
        assert_eq!(ConsoleSurface::Pad.level_wire(), LevelWire::Db);
        assert_eq!(ConsoleSurface::SSeriesGp.level_wire(), LevelWire::Db);
        // Both SD surfaces address through the shared TitleCase tree; only the
        // S-series flat /channel/{n}/ numbering does not.
        assert!(ConsoleSurface::SdOther.uses_pad_tree());
        assert!(ConsoleSurface::Pad.uses_pad_tree());
        assert!(!ConsoleSurface::SSeriesGp.uses_pad_tree());
    }

    #[test]
    fn family_round_trips_through_json() {
        for family in ConsoleFamily::ALL {
            let json = serde_json::to_string(&family).unwrap();
            let back: ConsoleFamily = serde_json::from_str(&json).unwrap();
            assert_eq!(back, family);
        }
    }

    #[test]
    fn quirks_round_trip_through_json() {
        for q in [PadQuirks::S21, PadQuirks::SD_HYPOTHESIS] {
            let json = serde_json::to_string(&q).unwrap();
            let back: PadQuirks = serde_json::from_str(&json).unwrap();
            assert_eq!(back, q);
        }
    }

    #[test]
    fn partial_quirk_json_fills_missing_fields_with_s21_values() {
        // A hand-edited override naming one flag must leave the rest at the
        // S21 values rather than at Rust's zero-ish defaults.
        let q: PadQuirks = serde_json::from_str(r#"{"eq_bands_reversed": false}"#).unwrap();
        assert!(!q.eq_bands_reversed);
        assert!(q.dyn1_mid_high_swapped);
        assert_eq!(q.bool_wire, BoolWire::Float01);
        assert_eq!(q.pan_wire, PanWire::ZeroToOne);
        assert!(q.control_groups_zero_based);
    }

    #[test]
    fn param_support_usability() {
        assert!(ParamSupport::Verified.is_usable());
        assert!(ParamSupport::Assumed.is_usable());
        assert!(!ParamSupport::Unsupported.is_usable());
    }
}
