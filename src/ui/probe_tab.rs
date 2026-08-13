//! Protocol Probe tab — hardware verification of the SD/Quantum OSC surfaces.
//!
//! SD and Quantum consoles expose two candidate surfaces over the same
//! TitleCase parameter tree: the "Pad" surface the live connection already
//! speaks (bare paths, dB values) and the `/sd/` "Other OSC" surface (a `/sd`
//! prefix, normalized 0..1 values) that DiGiCo documents as a one-way command
//! list. Neither is verified, and the choice between them decides the
//! architecture — this app is a live state mirror, so ganging, pan link and
//! personal monitoring all need the desk to PUSH a value when someone moves a
//! control on the console surface itself. Pull-on-demand only solves the first
//! enumeration.
//!
//! Hence three questions per surface, asked in this order:
//!
//! 1. **Write** — we send a value; does the desk act? Only the operator's eyes
//!    can answer, so the row stays unconfirmed until they say so.
//! 2. **Pull** — we send `{path}/?` with no argument; does a value come back,
//!    and with what type and scaling?
//! 3. **Push** — the decisive one. The operator moves the control on the desk
//!    and we record whatever arrives unsolicited, *including nothing*.
//!
//! A never-run test and a confirmed silence are completely different findings,
//! so [`Verdict`] keeps them apart and the saved report lists the untested
//! combinations by name rather than leaving them out.
//!
//! Pad probes ride the live connection's [`IpadSender`] and are observed by
//! re-scanning the OSC log, which already records both directions of that link.
//! `/sd/` probes own an ephemeral UDP socket in a spawned task — the console's
//! Other-OSC device is configured separately on the desk, with its own ports —
//! and report back through a `std::sync::Mutex` slot polled once per frame, so
//! the UI thread never blocks on the runtime.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use eframe::egui;
use rosc::{OscMessage, OscPacket, OscType};
use tokio::sync::RwLock;

use super::help::{HelpKey, help};
use super::theme;
use crate::model::family::ConsoleSurface;
use crate::model::osc_log::{OscDirection, OscLog, OscProtocol};
use crate::model::state::ConsoleState;
use crate::osc::client::format_osc_args;
use crate::osc::ipad_client::IpadSender;

/// How long a Write / Pull run listens before it calls the surface silent.
///
/// A desk that answers a query answers within tens of milliseconds, so 1.5 s is
/// long enough that "nothing came back" is a real finding, and short enough
/// that the operator can work through the whole matrix in one session.
const OBSERVE_WINDOW: Duration = Duration::from_millis(1500);

/// Inbound Pad paths the app itself solicits (the connection's keep-alive).
/// Excluded from a PUSH window so a heartbeat reply can never be mistaken for
/// the console reporting a surface move.
const PAD_SOLICITED: &[&str] = &["/Console/Name", "/Console/Session/Filename"];

/// Which of the three questions a row answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeTest {
    Write,
    Pull,
    Push,
}

impl ProbeTest {
    fn label(self) -> &'static str {
        match self {
            ProbeTest::Write => "WRITE",
            ProbeTest::Pull => "PULL",
            ProbeTest::Push => "PUSH",
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            ProbeTest::Write => theme::ACCENT_ORANGE,
            ProbeTest::Pull => theme::ACCENT_BLUE,
            ProbeTest::Push => theme::ACCENT_GREEN,
        }
    }
}

/// Outcome of one run.
///
/// The distinction that matters: [`Verdict::Silent`] means the probe ran and
/// the desk said nothing — a real, load-bearing negative — while
/// [`Verdict::NotRun`] means the probe never got off the ground and nothing at
/// all was learned. Conflating those two would make the whole session
/// worthless.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Sent, but whether the desk acted is something only the operator saw.
    Unconfirmed,
    /// Operator confirmed the desk acted on the write.
    Acted,
    /// Operator confirmed the desk did nothing.
    NoEffect,
    /// Something came back.
    Replied,
    /// The window closed with nothing.
    Silent,
    /// The probe could not run at all (bind failure, send error, no link).
    NotRun(String),
}

impl Verdict {
    fn label(&self) -> String {
        match self {
            Verdict::Unconfirmed => "UNCONFIRMED".into(),
            Verdict::Acted => "DESK ACTED".into(),
            Verdict::NoEffect => "NO EFFECT".into(),
            Verdict::Replied => "REPLY".into(),
            Verdict::Silent => "SILENT".into(),
            Verdict::NotRun(why) => format!("NOT RUN — {why}"),
        }
    }

    fn color(&self) -> egui::Color32 {
        match self {
            Verdict::Unconfirmed => theme::ACCENT_AMBER,
            Verdict::Acted | Verdict::Replied => theme::ACCENT_GREEN,
            Verdict::NoEffect | Verdict::Silent => theme::ACCENT_RED,
            Verdict::NotRun(_) => theme::TEXT_SECONDARY,
        }
    }
}

/// One recorded run.
pub struct ProbeResult {
    pub at: DateTime<Local>,
    pub surface: ConsoleSurface,
    pub test: ProbeTest,
    /// Address as sent (a pull carries its `/?` suffix).
    pub path: String,
    pub sent: String,
    pub received: String,
    pub latency_ms: Option<u64>,
    pub verdict: Verdict,
}

/// One message seen during an observation window.
#[derive(Clone)]
struct Hit {
    elapsed_ms: u64,
    path: String,
    args: String,
}

/// Hand-back slot for the spawned probe tasks. `std::sync::Mutex`, not tokio's,
/// so a frame that polls it never blocks on the runtime.
#[derive(Default)]
pub struct ProbeShared {
    /// Messages the `/sd/` listener captured, oldest first.
    hits: Vec<Hit>,
    /// Bind / send failure — the probe never ran, as distinct from silence.
    error: Option<String>,
    /// The listener has stopped and released its socket.
    finished: bool,
    /// UI asked the listener to stop early.
    stop: bool,
    /// Outcome text from the last report write.
    report_status: Option<String>,
}

impl ProbeShared {
    /// Clear the per-run fields, keeping the report status (which belongs to a
    /// different, unrelated task).
    fn reset_run(&mut self) {
        self.hits.clear();
        self.error = None;
        self.finished = false;
        self.stop = false;
    }
}

/// The run currently being observed. Only one at a time — a second listener
/// would fight the first for the local UDP port.
struct InFlight {
    surface: ConsoleSurface,
    test: ProbeTest,
    /// Address as sent.
    path: String,
    /// Address replies are expected on (the pull's `/?` stripped).
    reply_path: String,
    sent: String,
    started: Instant,
    /// Wall clock at arm time — Pad hits are matched by log timestamp.
    started_wall: DateTime<Local>,
    window: Duration,
    hits: Vec<Hit>,
    error: Option<String>,
}

/// How a free-form value is put on the wire. Explicit rather than inferred
/// from the text, because the boolean wire format is one of the things the
/// operator is here to measure: `1`, `1.0` and OSC `T` must be separately
/// sendable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ArgKind {
    #[default]
    Float,
    Int,
    Text,
    OscTrue,
    OscFalse,
    NoArg,
}

impl ArgKind {
    fn label(self) -> &'static str {
        match self {
            ArgKind::Float => "float",
            ArgKind::Int => "int",
            ArgKind::Text => "string",
            ArgKind::OscTrue => "OSC true (T)",
            ArgKind::OscFalse => "OSC false (F)",
            ArgKind::NoArg => "no argument",
        }
    }

    const ALL: [ArgKind; 6] = [
        ArgKind::Float,
        ArgKind::Int,
        ArgKind::Text,
        ArgKind::OscTrue,
        ArgKind::OscFalse,
        ArgKind::NoArg,
    ];

    fn build(self, text: &str) -> Result<Vec<OscType>, String> {
        let t = text.trim();
        match self {
            ArgKind::Float => t
                .parse::<f32>()
                .map(|v| vec![OscType::Float(v)])
                .map_err(|_| format!("\"{t}\" is not a float")),
            ArgKind::Int => t
                .parse::<i32>()
                .map(|v| vec![OscType::Int(v)])
                .map_err(|_| format!("\"{t}\" is not an integer")),
            ArgKind::Text => Ok(vec![OscType::String(t.to_string())]),
            ArgKind::OscTrue => Ok(vec![OscType::Bool(true)]),
            ArgKind::OscFalse => Ok(vec![OscType::Bool(false)]),
            ArgKind::NoArg => Ok(Vec::new()),
        }
    }
}

/// A write value from the built-in catalog. Const-friendly (so the catalog can
/// be a `const`) and expanded to an [`OscType`] at run time.
#[derive(Clone, Copy)]
enum WriteSpec {
    Float(f32),
    Text(&'static str),
}

impl WriteSpec {
    fn args(self) -> Vec<OscType> {
        match self {
            WriteSpec::Float(v) => vec![OscType::Float(v)],
            WriteSpec::Text(s) => vec![OscType::String(s.to_string())],
        }
    }
}

/// One entry in the built-in probe set.
struct Builtin {
    label: &'static str,
    /// `{ch}` is replaced with the probe channel.
    path: &'static str,
    /// Write values differ per surface: the Pad surface carries dB, `/sd/`
    /// carries a normalized 0..1 position.
    pad: WriteSpec,
    sd: WriteSpec,
    /// What to watch for on the desk. Inline rather than a tooltip — the
    /// operator is remote and cannot ask us mid-session.
    watch: &'static str,
}

/// The parameters worth answering the matrix for: one continuous level, one
/// toggle, one string, one bipolar control, one send, one banded parameter.
/// Anything more specific belongs in the free-form row.
const BUILTINS: &[Builtin] = &[
    Builtin {
        label: "Fader",
        path: "/Input_Channels/{ch}/fader",
        pad: WriteSpec::Float(-10.0),
        sd: WriteSpec::Float(0.75),
        watch: "Fader law. Pad should read -10 dB; /sd/ 0.75 should sit near the \
                top of the travel. Note the actual dB the desk shows.",
    },
    Builtin {
        label: "Mute",
        path: "/Input_Channels/{ch}/mute",
        pad: WriteSpec::Float(1.0),
        sd: WriteSpec::Float(1.0),
        watch: "Boolean wire format. If float 1.0 is ignored, retry from the \
                free-form row as int 1 and as OSC true.",
    },
    Builtin {
        label: "Name",
        path: "/Input_Channels/{ch}/Channel_Input/name",
        pad: WriteSpec::Text("PROBE"),
        sd: WriteSpec::Text("PROBE"),
        watch: "String parameter — proves non-numeric arguments survive, and \
                gives PULL something unmistakable to return.",
    },
    Builtin {
        label: "Pan",
        path: "/Input_Channels/{ch}/Panner/pan",
        pad: WriteSpec::Float(0.25),
        sd: WriteSpec::Float(0.25),
        watch: "Pan scaling. 0.25 lands quarter-left if the wire is 0..1, and \
                just right of centre if it is -1..+1.",
    },
    Builtin {
        label: "Aux 1 send level",
        path: "/Input_Channels/{ch}/Aux_Send/1/send_level",
        pad: WriteSpec::Float(-20.0),
        sd: WriteSpec::Float(0.5),
        watch: "Send level, which may use a different law from the main fader. \
                Chase the extremes from the free-form row afterwards.",
    },
    Builtin {
        label: "EQ band 1 gain",
        path: "/Input_Channels/{ch}/EQ/eq_gain_1",
        pad: WriteSpec::Float(6.0),
        sd: WriteSpec::Float(0.75),
        watch: "EQ band identity — note WHICH band moves on screen. S21 firmware \
                numbers them in reverse; whether SD/Quantum does is unknown.",
    },
];

/// A quirk worth chasing that the built-in set deliberately leaves out.
/// Clicking one loads it into the free-form row rather than firing it, so the
/// operator can read the address before it hits the desk.
struct Chase {
    label: &'static str,
    path: &'static str,
    kind: ArgKind,
    value: &'static str,
}

const CHASES: &[Chase] = &[
    Chase {
        label: "EQ band 5 gain on Aux 1 — bus outputs have 8 bands, inputs 4",
        path: "/Aux_Outputs/1/EQ/eq_gain_5",
        kind: ArgKind::Float,
        value: "6.0",
    },
    Chase {
        label: "EQ band 8 gain on Aux 1 — the top of the 8-band range",
        path: "/Aux_Outputs/1/EQ/eq_gain_8",
        kind: ArgKind::Float,
        value: "6.0",
    },
    Chase {
        label: "EQ band 4 gain — run against band 1 to expose a reversed order",
        path: "/Input_Channels/{ch}/EQ/eq_gain_4",
        kind: ArgKind::Float,
        value: "6.0",
    },
    Chase {
        label: "Dynamics band identity — multiband comp threshold, band 2",
        path: "/Input_Channels/{ch}/Dynamics/comp_thresh_2",
        kind: ArgKind::Float,
        value: "-20.0",
    },
    Chase {
        label: "Control Group numbering — fader at index 0",
        path: "/Control_Groups/0/fader",
        kind: ArgKind::Float,
        value: "-10.0",
    },
    Chase {
        label: "Control Group numbering — fader at index 1",
        path: "/Control_Groups/1/fader",
        kind: ArgKind::Float,
        value: "-10.0",
    },
    Chase {
        label: "Boolean as int — mute 1",
        path: "/Input_Channels/{ch}/mute",
        kind: ArgKind::Int,
        value: "1",
    },
    Chase {
        label: "Boolean as OSC true — mute T",
        path: "/Input_Channels/{ch}/mute",
        kind: ArgKind::OscTrue,
        value: "",
    },
    Chase {
        label: "Pan scaling — 0.0 (hard left if 0..1, centre if signed)",
        path: "/Input_Channels/{ch}/Panner/pan",
        kind: ArgKind::Float,
        value: "0.0",
    },
    Chase {
        label: "Pan scaling — -1.0 (hard left only if the wire is signed)",
        path: "/Input_Channels/{ch}/Panner/pan",
        kind: ArgKind::Float,
        value: "-1.0",
    },
    Chase {
        label: "Fader law — 0.0 (unity in dB, bottom of travel if normalized)",
        path: "/Input_Channels/{ch}/fader",
        kind: ArgKind::Float,
        value: "0.0",
    },
    Chase {
        label: "Send level floor — does -140 read as off or clamp?",
        path: "/Input_Channels/{ch}/Aux_Send/1/send_level",
        kind: ArgKind::Float,
        value: "-140.0",
    },
    Chase {
        label: "Send level ceiling — is +10 accepted or clamped?",
        path: "/Input_Channels/{ch}/Aux_Send/1/send_level",
        kind: ArgKind::Float,
        value: "10.0",
    },
];

/// Per-frame state for the Protocol Probe tab. Runtime-only.
pub struct ProbeTabState {
    /// Every run this session, oldest first.
    pub results: Vec<ProbeResult>,
    /// Surface the probe buttons currently target.
    pub surface: ConsoleSurface,
    /// `/sd/` target — the Other-OSC device has its own address and ports,
    /// configured separately on the desk from the Pad device.
    pub sd_host: String,
    pub sd_console_port: String,
    pub sd_local_port: String,
    /// Channel the built-in probe paths address.
    pub channel: String,
    /// Free-form probe row.
    pub free_path: String,
    pub free_value: String,
    pub free_kind: ArgKind,
    /// How long a PUSH arm listens, in seconds.
    pub push_window_secs: String,
    /// Operator's free text, copied into the saved report.
    pub notes: String,
    /// Transient status line.
    pub status: Option<String>,
    shared: Arc<Mutex<ProbeShared>>,
    in_flight: Option<InFlight>,
}

impl Default for ProbeTabState {
    fn default() -> Self {
        Self {
            results: Vec::new(),
            surface: ConsoleSurface::Pad,
            sd_host: String::new(),
            // One off the conventional 8000/9000 pair: those belong to the Pad
            // device, and two devices on one console cannot share a receive
            // port. Both ends are operator-set on the desk, so this is only a
            // first-run default.
            sd_console_port: "8001".into(),
            sd_local_port: "9001".into(),
            channel: "1".into(),
            free_path: String::new(),
            free_value: String::new(),
            free_kind: ArgKind::default(),
            push_window_secs: "20".into(),
            notes: String::new(),
            status: None,
            shared: Arc::new(Mutex::new(ProbeShared::default())),
            in_flight: None,
        }
    }
}

impl ProbeTabState {
    fn probe_channel(&self) -> u16 {
        self.channel.trim().parse().unwrap_or(1)
    }

    /// Resolve a catalog path template against the probe channel and the
    /// selected surface's prefix.
    fn resolve(&self, template: &str) -> String {
        let bare = template.replace("{ch}", &self.probe_channel().to_string());
        format!("{}{bare}", self.surface.path_prefix())
    }

    fn push_window(&self) -> Duration {
        let secs: u64 = self.push_window_secs.trim().parse().unwrap_or(20);
        Duration::from_secs(secs.clamp(2, 300))
    }
}

/// Draw the Protocol Probe tab.
pub fn draw_probe_tab(
    ui: &mut egui::Ui,
    tab: &mut ProbeTabState,
    state: &Arc<RwLock<ConsoleState>>,
    ipad_sender: &Option<IpadSender>,
    osc_log: &OscLog,
    console_ip: &str,
    runtime: &tokio::runtime::Handle,
) {
    if tab.sd_host.is_empty() {
        tab.sd_host = console_ip.to_string();
    }
    poll_run(tab, osc_log);
    if tab.in_flight.is_some() {
        // Nothing here is input-driven — the countdown and the arriving hits
        // both need frames of their own.
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            draw_intro_card(ui);
            ui.add_space(6.0);
            draw_surface_card(ui, tab, ipad_sender);
            ui.add_space(6.0);
            draw_run_card(ui, tab, ipad_sender, runtime);
            ui.add_space(6.0);
            draw_in_flight_card(ui, tab);
            ui.add_space(6.0);
            draw_results_card(ui, tab, state, ipad_sender.is_some(), runtime);
        });
}

// ─── Cards ───────────────────────────────────────────────────────────

fn draw_intro_card(ui: &mut egui::Ui) {
    theme::card_frame().show(ui, |ui| {
        theme::section_heading(ui, "What this tab has to answer");
        ui.label(
            egui::RichText::new(
                "This app is a live state mirror. Ganging, pan link and personal \
                 monitoring all need the console to report a value when someone moves \
                 a control on the desk itself — pulling values on demand only solves \
                 the first enumeration. Run all three tests, in this order, on each \
                 surface. Every row you produce is a finding, including the empty ones.",
            )
            .color(theme::label_weak()),
        );
        ui.add_space(8.0);
        matrix_line(
            ui,
            ProbeTest::Write,
            "We send a value. Watch the DESK: did it act? Only you can see that, so the \
             row stays UNCONFIRMED until you mark it \"desk acted\" or \"no effect\" in \
             the results table below.",
        );
        matrix_line(
            ui,
            ProbeTest::Pull,
            "We send \"{path}/?\" with no argument. A value coming back means the surface \
             answers queries — enough to enumerate the desk at connect time, but not \
             enough for a live mirror.",
        );
        matrix_line(
            ui,
            ProbeTest::Push,
            "The decisive one. Arm it, then move that control ON THE CONSOLE and leave \
             the app alone. Anything arriving unsolicited is recorded — and so is nothing \
             arriving, which is itself the answer: a surface that never pushes cannot \
             drive ganging however well WRITE and PULL work.",
        );
    });
}

fn matrix_line(ui: &mut egui::Ui, test: ProbeTest, prose: &str) {
    ui.horizontal_top(|ui| {
        ui.add_sized(
            [70.0, 18.0],
            egui::Label::new(
                egui::RichText::new(test.label())
                    .strong()
                    .color(test.color()),
            ),
        );
        ui.add(egui::Label::new(egui::RichText::new(prose).color(theme::label_weak())).wrap());
    });
    ui.add_space(4.0);
}

fn draw_surface_card(ui: &mut egui::Ui, tab: &mut ProbeTabState, ipad_sender: &Option<IpadSender>) {
    theme::card_frame().show(ui, |ui| {
        theme::section_heading(ui, "Surface under test");
        ui.label(
            egui::RichText::new(
                "Both surfaces address the same TitleCase parameter tree. Run the whole \
                 matrix on one, then switch and run it again — the report keeps them apart.",
            )
            .color(theme::label_weak())
            .small(),
        );
        ui.add_space(6.0);

        ui.radio_value(
            &mut tab.surface,
            ConsoleSurface::Pad,
            "DiGiCo Pad — bare paths, dB values, rides this app's live connection",
        )
        .on_hover_text(help(HelpKey::ProbeSurfacePad));
        ui.radio_value(
            &mut tab.surface,
            ConsoleSurface::SdOther,
            "Other OSC \"/sd/\" — /sd prefix, normalized 0..1 values, its own socket",
        )
        .on_hover_text(help(HelpKey::ProbeSurfaceSd));

        ui.add_space(8.0);
        match tab.surface {
            ConsoleSurface::SdOther => draw_sd_target(ui, tab),
            _ => draw_pad_target(ui, ipad_sender),
        }
    });
}

fn draw_pad_target(ui: &mut egui::Ui, ipad_sender: &Option<IpadSender>) {
    if ipad_sender.is_some() {
        ui.horizontal(|ui| {
            theme::status_dot(ui, theme::ACCENT_GREEN);
            ui.label(
                egui::RichText::new(
                    "Live Pad link is up — probes are sent raw down that connection, \
                     bypassing the app's own codec, and replies are read back out of the \
                     OSC Log.",
                )
                .color(theme::label_weak()),
            );
        });
    } else {
        ui.horizontal(|ui| {
            theme::status_dot(ui, theme::ACCENT_RED);
            ui.label(
                egui::RichText::new(
                    "No live Pad connection. Connect on the Setup tab first — this surface \
                     has no probe of its own, it borrows the app's connection.",
                )
                .color(theme::ACCENT_AMBER),
            );
        });
    }
}

fn draw_sd_target(ui: &mut egui::Ui, tab: &mut ProbeTabState) {
    egui::Grid::new("probe_sd_target")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Console IP");
            theme::padded_text_edit(ui, &mut tab.sd_host, 150.0, true, "10.0.1.10")
                .on_hover_text(help(HelpKey::ProbeSdHost));
            ui.end_row();

            ui.label("Console port (we send here)");
            theme::padded_text_edit(ui, &mut tab.sd_console_port, 80.0, true, "8001")
                .on_hover_text(help(HelpKey::ProbeSdConsolePort));
            ui.end_row();

            ui.label("Local port (desk replies here)");
            theme::padded_text_edit(ui, &mut tab.sd_local_port, 80.0, true, "9001")
                .on_hover_text(help(HelpKey::ProbeSdLocalPort));
            ui.end_row();
        });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "These must match the Other OSC device on the desk's External Control page, \
             which is configured separately from the Pad device and cannot share its \
             ports — hence the defaults being one off the usual 8000 / 9000 pair. The \
             socket is opened for each run and closed again; if the bind fails, something \
             else on this PC already holds that local port (most likely the live Pad \
             link), so pick another or disconnect on Setup.",
        )
        .color(theme::label_weak())
        .small(),
    );
}

fn draw_run_card(
    ui: &mut egui::Ui,
    tab: &mut ProbeTabState,
    ipad_sender: &Option<IpadSender>,
    runtime: &tokio::runtime::Handle,
) {
    let pad_missing = tab.surface != ConsoleSurface::SdOther && ipad_sender.is_none();

    theme::card_frame().show(ui, |ui| {
        theme::section_heading(ui, "Probes");

        if pad_missing {
            ui.label(
                egui::RichText::new(
                    "The Pad surface needs the app's live connection. Connect on the Setup \
                     tab, or switch to the \"/sd/\" surface above, which brings its own socket.",
                )
                .color(theme::ACCENT_AMBER),
            );
            return;
        }

        ui.label(
            egui::RichText::new(
                "⚠ WRITE changes the desk for real. Use a spare channel or a scratch \
                 session, and set the probe channel accordingly.",
            )
            .color(theme::ACCENT_AMBER),
        );
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label("Probe channel");
            theme::padded_text_edit(ui, &mut tab.channel, 60.0, true, "1")
                .on_hover_text(help(HelpKey::ProbeChannel));
            ui.add_space(16.0);
            ui.label("PUSH listen window (s)");
            theme::padded_text_edit(ui, &mut tab.push_window_secs, 60.0, true, "20")
                .on_hover_text(help(HelpKey::ProbePushWindow));
        });
        ui.add_space(8.0);

        let busy = tab.in_flight.is_some();
        let mut launch: Option<(ProbeTest, String, Vec<OscType>)> = None;

        egui::Grid::new("probe_builtins")
            .num_columns(4)
            .spacing([12.0, 8.0])
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Parameter");
                ui.strong("Address sent");
                ui.strong("Run");
                ui.strong("Done so far");
                ui.end_row();

                for (idx, b) in BUILTINS.iter().enumerate() {
                    ui.vertical(|ui| {
                        ui.set_max_width(230.0);
                        ui.label(egui::RichText::new(b.label).strong());
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(b.watch)
                                    .color(theme::label_weak())
                                    .small(),
                            )
                            .wrap(),
                        );
                    });

                    let path = tab.resolve(b.path);
                    let spec = match tab.surface {
                        ConsoleSurface::SdOther => b.sd,
                        _ => b.pad,
                    };
                    let args = spec.args();
                    ui.vertical(|ui| {
                        ui.set_max_width(280.0);
                        ui.add(egui::Label::new(egui::RichText::new(&path).small()).wrap());
                        ui.label(
                            egui::RichText::new(format!("write value: {}", format_osc_args(&args)))
                                .color(theme::label_weak())
                                .small(),
                        );
                    });

                    if let Some(test) = draw_test_buttons(ui, busy, idx) {
                        launch = Some((test, path.clone(), args));
                    }
                    draw_coverage(ui, tab, &path);
                    ui.end_row();
                }
            });

        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "Done so far — one mark per test, in the order WRITE PULL PUSH:   \
                 –  never run   ●  desk acted / replied   ✖  no effect / silent   \
                 ?  awaiting your answer   !  could not run",
            )
            .color(theme::label_weak())
            .small(),
        );

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        // ── Free-form row ──
        ui.label(egui::RichText::new("Free-form probe").strong());
        ui.label(
            egui::RichText::new(
                "Anything the built-in set doesn't cover. The path is sent exactly as \
                 typed, after the surface prefix; the argument type is explicit so the \
                 boolean wire format can be measured rather than guessed.",
            )
            .color(theme::label_weak())
            .small(),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Path");
            theme::padded_text_edit(
                ui,
                &mut tab.free_path,
                300.0,
                true,
                "/Input_Channels/1/fader",
            )
            .on_hover_text(help(HelpKey::ProbeFreePath));

            ui.label("Type");
            egui::ComboBox::from_id_salt("probe_free_kind")
                .selected_text(tab.free_kind.label())
                .width(120.0)
                .show_ui(ui, |ui| {
                    for kind in ArgKind::ALL {
                        ui.selectable_value(&mut tab.free_kind, kind, kind.label());
                    }
                })
                .response
                .on_hover_text(help(HelpKey::ProbeFreeKind));

            ui.label("Value");
            theme::padded_text_edit(ui, &mut tab.free_value, 100.0, true, "-10.0")
                .on_hover_text(help(HelpKey::ProbeFreeValue));
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if let Some(test) = draw_test_buttons(ui, busy, usize::MAX) {
                let raw = tab.free_path.trim().to_string();
                if raw.is_empty() {
                    tab.status = Some("Type a path first — the free-form row is empty.".into());
                } else {
                    let bare = raw.replace("{ch}", &tab.probe_channel().to_string());
                    let path = format!("{}{bare}", tab.surface.path_prefix());
                    match tab.free_kind.build(&tab.free_value) {
                        Ok(args) => launch = Some((test, path, args)),
                        Err(why) => tab.status = Some(format!("Value rejected: {why}")),
                    }
                }
            }
        });

        ui.add_space(8.0);
        egui::CollapsingHeader::new("Quirks worth chasing — click to load into the row above")
            .default_open(false)
            .show(ui, |ui| {
                for (idx, c) in CHASES.iter().enumerate() {
                    if ui
                        .add(egui::Button::new(c.label).fill(theme::btn_neutral()))
                        .on_hover_text(help(HelpKey::ProbeChase))
                        .clicked()
                    {
                        tab.free_path = c.path.to_string();
                        tab.free_kind = c.kind;
                        tab.free_value = c.value.to_string();
                        tab.status =
                            Some(format!("Loaded chase #{} into the free-form row.", idx + 1));
                    }
                }
            });

        if let Some((test, path, args)) = launch {
            start_run(tab, test, path, args, ipad_sender, runtime);
        }
    });
}

/// The three run buttons. `salt` distinguishes rows; disabled buttons carry a
/// hover saying why, so nothing on this tab is ever a silent no-op.
fn draw_test_buttons(ui: &mut egui::Ui, busy: bool, salt: usize) -> Option<ProbeTest> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        for (test, key) in [
            (ProbeTest::Write, HelpKey::ProbeWrite),
            (ProbeTest::Pull, HelpKey::ProbePull),
            (ProbeTest::Push, HelpKey::ProbePush),
        ] {
            let btn = theme::action_button(test.label(), test.color(), egui::Vec2::new(62.0, 26.0));
            let resp = ui
                .push_id((salt, test.label()), |ui| ui.add_enabled(!busy, btn))
                .inner;
            let resp = if busy {
                resp.on_disabled_hover_text(
                    "A probe is already running — let it finish or stop it below.",
                )
            } else {
                resp.on_hover_text(help(key))
            };
            if resp.clicked() {
                chosen = Some(test);
            }
        }
    });
    chosen
}

/// Which of the three tests this path has already been run for on the current
/// surface. A dash is "never run" — the state the report must never blur into
/// a failure.
fn draw_coverage(ui: &mut egui::Ui, tab: &ProbeTabState, path: &str) {
    ui.horizontal(|ui| {
        for test in [ProbeTest::Write, ProbeTest::Pull, ProbeTest::Push] {
            let last = tab
                .results
                .iter()
                .rev()
                .find(|r| r.surface == tab.surface && r.test == test && r.path_matches(path));
            let (glyph, color, tip) = match last {
                None => ("–", theme::TEXT_SECONDARY, "not run yet".to_string()),
                Some(r) => {
                    let g = match r.verdict {
                        Verdict::Acted | Verdict::Replied => "●",
                        Verdict::NoEffect | Verdict::Silent => "✖",
                        Verdict::Unconfirmed => "?",
                        Verdict::NotRun(_) => "!",
                    };
                    (g, r.verdict.color(), r.verdict.label())
                }
            };
            ui.label(egui::RichText::new(glyph).color(color).strong())
                .on_hover_text(format!("{}: {tip}", test.label()));
        }
    });
}

fn draw_in_flight_card(ui: &mut egui::Ui, tab: &mut ProbeTabState) {
    let Some(run) = tab.in_flight.as_ref() else {
        return;
    };
    let elapsed = run.started.elapsed();
    let remaining = run.window.saturating_sub(elapsed);
    let test = run.test;
    let path = run.path.clone();
    let hits = run.hits.len();
    let mut stop = false;

    theme::elevated_frame()
        .outer_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} running", test.label()))
                        .strong()
                        .color(test.color()),
                );
                ui.label(egui::RichText::new(&path).color(theme::label_weak()));
                ui.add_space(12.0);
                ui.label(format!("{:.1}s left", remaining.as_secs_f32()));
                ui.add_space(12.0);
                ui.label(format!("{hits} message(s) so far"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(theme::action_button(
                            "Stop & record",
                            theme::ACCENT_RED,
                            egui::Vec2::new(120.0, 26.0),
                        ))
                        .on_hover_text(help(HelpKey::ProbeStop))
                        .clicked()
                    {
                        stop = true;
                    }
                });
            });
            if test == ProbeTest::Push {
                ui.label(
                    egui::RichText::new(
                        "Move that control on the console now. Do not touch this app — anything \
                     it sends would make the reply solicited.",
                    )
                    .color(theme::ACCENT_AMBER),
                );
            }
        });

    if stop {
        request_stop(tab);
    }
}

fn draw_results_card(
    ui: &mut egui::Ui,
    tab: &mut ProbeTabState,
    state: &Arc<RwLock<ConsoleState>>,
    pad_live: bool,
    runtime: &tokio::runtime::Handle,
) {
    theme::card_frame().show(ui, |ui| {
        theme::section_heading_with(ui, "Results", |ui| {
            ui.label(
                egui::RichText::new(format!("{} run(s)", tab.results.len()))
                    .color(theme::label_weak())
                    .small(),
            );
        });

        if tab.results.is_empty() {
            ui.label(
                egui::RichText::new("Nothing run yet. Start with WRITE on the fader.")
                    .color(theme::label_weak()),
            );
        } else {
            draw_results_table(ui, tab);
        }

        ui.add_space(10.0);
        ui.label(egui::RichText::new("Notes for the report").strong());
        ui.add(
            egui::TextEdit::multiline(&mut tab.notes)
                .desired_rows(4)
                .desired_width(f32::INFINITY)
                .hint_text(
                    "Console model and firmware, what you saw on the desk, anything odd. \
                     This is copied into the saved report verbatim.",
                ),
        )
        .on_hover_text(help(HelpKey::ProbeNotes));

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add(theme::action_button(
                    "Save report…",
                    theme::ACCENT_GREEN,
                    egui::Vec2::new(130.0, 28.0),
                ))
                .on_hover_text(help(HelpKey::ProbeSaveReport))
                .clicked()
            {
                save_report(tab, state, pad_live, runtime);
            }
            if ui
                .add(theme::action_button(
                    "Clear results",
                    theme::ACCENT_RED,
                    egui::Vec2::new(130.0, 28.0),
                ))
                .on_hover_text(help(HelpKey::ProbeClear))
                .clicked()
            {
                tab.results.clear();
                tab.status = Some("Results cleared.".into());
            }
        });

        if let Some(msg) = &tab.status {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(msg.as_str()).color(theme::label_weak()));
        }
    });
}

fn draw_results_table(ui: &mut egui::Ui, tab: &mut ProbeTabState) {
    use egui_extras::{Column, TableBuilder};

    let text_height = ui.text_style_height(&egui::TextStyle::Body);
    let row_height = text_height * 2.0 + 6.0;
    // Collected while drawing: the verdict cell's two buttons answer the WRITE
    // question the app cannot answer for itself.
    let mut confirm: Option<(usize, Verdict)> = None;

    TableBuilder::new(ui)
        .striped(true)
        .vscroll(false)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(120.0)) // Surface
        .column(Column::exact(56.0)) // Test
        .column(Column::initial(230.0).at_least(120.0).clip(true)) // Path
        .column(Column::initial(90.0).at_least(60.0).clip(true)) // Sent
        .column(Column::initial(220.0).at_least(120.0).clip(true)) // Received
        .column(Column::exact(70.0)) // Latency
        .column(Column::remainder().at_least(200.0)) // Verdict
        .header(text_height + 4.0, |mut header| {
            for title in [
                "Surface", "Test", "Path", "Sent", "Received", "Latency", "Verdict",
            ] {
                header.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, tab.results.len(), |mut row| {
                let idx = row.index();
                let r = &tab.results[idx];
                row.col(|ui| {
                    ui.label(r.surface.label());
                });
                row.col(|ui| {
                    ui.colored_label(r.test.color(), r.test.label());
                });
                row.col(|ui| {
                    ui.label(egui::RichText::new(&r.path).small())
                        .on_hover_text(&r.path);
                });
                row.col(|ui| {
                    ui.label(egui::RichText::new(&r.sent).small());
                });
                row.col(|ui| {
                    ui.label(egui::RichText::new(&r.received).small())
                        .on_hover_text(&r.received);
                });
                row.col(|ui| {
                    match r.latency_ms {
                        Some(ms) => ui.label(format!("{ms} ms")),
                        None => ui.label(egui::RichText::new("—").color(theme::label_weak())),
                    };
                });
                row.col(|ui| {
                    let label = r.verdict.label();
                    ui.colored_label(r.verdict.color(), &label)
                        .on_hover_text(&label);
                    if r.verdict == Verdict::Unconfirmed {
                        if ui
                            .small_button("desk acted")
                            .on_hover_text(help(HelpKey::ProbeConfirmActed))
                            .clicked()
                        {
                            confirm = Some((idx, Verdict::Acted));
                        }
                        if ui
                            .small_button("no effect")
                            .on_hover_text(help(HelpKey::ProbeConfirmNoEffect))
                            .clicked()
                        {
                            confirm = Some((idx, Verdict::NoEffect));
                        }
                    }
                });
            });
        });

    if let Some((idx, verdict)) = confirm
        && let Some(r) = tab.results.get_mut(idx)
    {
        r.verdict = verdict;
    }
}

// ─── Running a probe ─────────────────────────────────────────────────

fn start_run(
    tab: &mut ProbeTabState,
    test: ProbeTest,
    path: String,
    args: Vec<OscType>,
    ipad_sender: &Option<IpadSender>,
    runtime: &tokio::runtime::Handle,
) {
    if tab.in_flight.is_some() {
        return;
    }
    tab.shared.lock().unwrap().reset_run();

    // PUSH sends nothing at all: an unsolicited message is only unsolicited if
    // we stayed quiet.
    let (sent_path, sent_args) = match test {
        ProbeTest::Write => (path.clone(), args),
        ProbeTest::Pull => (format!("{path}/?"), Vec::new()),
        ProbeTest::Push => (path.clone(), Vec::new()),
    };
    let sent_desc = match test {
        ProbeTest::Write => format_osc_args(&sent_args),
        ProbeTest::Pull => "(query, no arg)".to_string(),
        ProbeTest::Push => "(nothing — listening)".to_string(),
    };
    let window = match test {
        ProbeTest::Push => tab.push_window(),
        _ => OBSERVE_WINDOW,
    };

    let mut run = InFlight {
        surface: tab.surface,
        test,
        path: sent_path.clone(),
        reply_path: path,
        sent: sent_desc,
        started: Instant::now(),
        started_wall: Local::now(),
        window,
        hits: Vec::new(),
        error: None,
    };

    match tab.surface {
        ConsoleSurface::SdOther => match sd_target(tab) {
            Ok((console, local_port)) => {
                let send = (test != ProbeTest::Push).then_some((sent_path, sent_args));
                let shared = tab.shared.clone();
                runtime.spawn(run_sd_probe(console, local_port, send, window, shared));
            }
            Err(why) => run.error = Some(why),
        },
        _ => match ipad_sender {
            Some(sender) if test != ProbeTest::Push => {
                let sender = sender.clone();
                let shared = tab.shared.clone();
                runtime.spawn(async move {
                    if let Err(e) = sender.send(&sent_path, sent_args).await {
                        shared.lock().unwrap().error = Some(format!("send failed: {e}"));
                    }
                });
            }
            Some(_) => {}
            None => run.error = Some("no live Pad connection".into()),
        },
    }

    tab.status = None;
    tab.in_flight = Some(run);
}

/// Resolve the `/sd/` console address and our listening port from the
/// operator's fields.
fn sd_target(tab: &ProbeTabState) -> Result<(SocketAddr, u16), String> {
    let ip: IpAddr = tab
        .sd_host
        .trim()
        .parse()
        .map_err(|_| format!("\"{}\" is not an IP address", tab.sd_host.trim()))?;
    let console_port: u16 = tab
        .sd_console_port
        .trim()
        .parse()
        .map_err(|_| format!("\"{}\" is not a port", tab.sd_console_port.trim()))?;
    let local_port: u16 = tab
        .sd_local_port
        .trim()
        .parse()
        .map_err(|_| format!("\"{}\" is not a port", tab.sd_local_port.trim()))?;
    Ok((SocketAddr::new(ip, console_port), local_port))
}

/// The `/sd/` probe task: bind an ephemeral socket, optionally send once, then
/// listen until the window closes or the UI stops it.
async fn run_sd_probe(
    console: SocketAddr,
    local_port: u16,
    send: Option<(String, Vec<OscType>)>,
    window: Duration,
    shared: Arc<Mutex<ProbeShared>>,
) {
    let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), local_port);
    let socket = match crate::ui::net_interfaces::create_bound_udp_socket(bind, None).await {
        Ok(s) => s,
        Err(e) => {
            let mut g = shared.lock().unwrap();
            g.error = Some(format!("cannot bind local port {local_port}: {e}"));
            g.finished = true;
            return;
        }
    };

    let started = Instant::now();
    if let Some((addr, args)) = send {
        let packet = OscPacket::Message(OscMessage { addr, args });
        match rosc::encoder::encode(&packet) {
            Ok(buf) => {
                if let Err(e) = socket.send_to(&buf, console).await {
                    let mut g = shared.lock().unwrap();
                    g.error = Some(format!("send to {console} failed: {e}"));
                    g.finished = true;
                    return;
                }
            }
            Err(e) => {
                let mut g = shared.lock().unwrap();
                g.error = Some(format!("OSC encode failed: {e}"));
                g.finished = true;
                return;
            }
        }
    }

    let mut buf = vec![0u8; 65536];
    loop {
        let remaining = window.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        let stop = shared.lock().unwrap().stop;
        if stop {
            break;
        }
        // Wake regularly even when nothing arrives, so an early stop is honoured.
        let slice = remaining.min(Duration::from_millis(100));
        match tokio::time::timeout(slice, socket.recv_from(&mut buf)).await {
            Ok(Ok((size, _src))) => {
                let elapsed_ms = started.elapsed().as_millis() as u64;
                let hits = decode_hits(&buf[..size], elapsed_ms);
                shared.lock().unwrap().hits.extend(hits);
            }
            Ok(Err(e)) => {
                shared.lock().unwrap().error = Some(format!("receive failed: {e}"));
                break;
            }
            Err(_) => continue,
        }
    }

    shared.lock().unwrap().finished = true;
}

/// Turn one received datagram into hits.
///
/// A packet that is not decodable OSC is still recorded rather than dropped:
/// on this surface "the desk answered, but in DiGiCo's bare-path form" is
/// itself the finding. This is a readable preview, deliberately not a second
/// implementation of the bare-path parser the Pad connection owns.
fn decode_hits(data: &[u8], elapsed_ms: u64) -> Vec<Hit> {
    let mut out = Vec::new();
    match crate::osc::decode_udp_tolerant(data) {
        Some(packet) => flatten_packet(packet, elapsed_ms, &mut out),
        None => {
            let path = match data.first() {
                Some(b'/') => {
                    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
                    String::from_utf8_lossy(&data[..end]).into_owned()
                }
                _ => "<undecodable>".to_string(),
            };
            let hex: String = data
                .iter()
                .take(16)
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            out.push(Hit {
                elapsed_ms,
                path,
                args: format!("(no OSC type tag; {} bytes: {hex})", data.len()),
            });
        }
    }
    out
}

fn flatten_packet(packet: OscPacket, elapsed_ms: u64, out: &mut Vec<Hit>) {
    match packet {
        OscPacket::Message(msg) => out.push(Hit {
            elapsed_ms,
            path: msg.addr,
            args: format_osc_args(&msg.args),
        }),
        OscPacket::Bundle(bundle) => {
            for p in bundle.content {
                flatten_packet(p, elapsed_ms, out);
            }
        }
    }
}

/// Poll the shared slot and the OSC log, and close out a finished run.
fn poll_run(tab: &mut ProbeTabState, osc_log: &OscLog) {
    if let Some(msg) = tab.shared.lock().unwrap().report_status.take() {
        tab.status = Some(msg);
    }

    let Some(run) = tab.in_flight.as_ref() else {
        return;
    };

    let (hits, error, task_done) = match run.surface {
        ConsoleSurface::SdOther => {
            let mut g = tab.shared.lock().unwrap();
            (g.hits.clone(), g.error.take(), g.finished)
        }
        // Pad probes are observed through the log the live connection already
        // writes. Re-derived from scratch each frame so the scan stays
        // idempotent — no cursor to keep in step with a ring buffer that drops
        // its oldest entries.
        _ => {
            let err = tab.shared.lock().unwrap().error.take();
            (collect_pad_hits(osc_log, run), err, false)
        }
    };

    let run = tab.in_flight.as_mut().expect("checked above");
    run.hits = hits;
    if error.is_some() {
        run.error = error;
    }
    let done = task_done || run.error.is_some() || run.started.elapsed() >= run.window;
    if done {
        let run = tab.in_flight.take().expect("checked above");
        tab.results.push(finish_run(run));
    }
}

fn collect_pad_hits(log: &OscLog, run: &InFlight) -> Vec<Hit> {
    log.snapshot()
        .into_iter()
        .filter(|e| e.protocol == OscProtocol::Ipad && e.direction == OscDirection::In)
        .filter(|e| e.timestamp >= run.started_wall)
        .filter(|e| match run.test {
            // A push window accepts anything the desk volunteers, minus the
            // app's own keep-alive replies.
            ProbeTest::Push => !PAD_SOLICITED.contains(&e.path.as_str()),
            _ => e.path == run.reply_path,
        })
        .map(|e| Hit {
            elapsed_ms: (e.timestamp - run.started_wall).num_milliseconds().max(0) as u64,
            path: e.path,
            args: e.args,
        })
        .collect()
}

fn finish_run(run: InFlight) -> ProbeResult {
    let latency_ms = run.hits.first().map(|h| h.elapsed_ms);
    let received = summarize_hits(&run.hits, run.window);
    let verdict = match (&run.error, run.test, run.hits.is_empty()) {
        (Some(why), _, _) => Verdict::NotRun(why.clone()),
        // Only the operator can say whether the desk moved, whatever came back.
        (None, ProbeTest::Write, _) => Verdict::Unconfirmed,
        (None, _, true) => Verdict::Silent,
        (None, _, false) => Verdict::Replied,
    };
    ProbeResult {
        at: Local::now(),
        surface: run.surface,
        test: run.test,
        path: run.path,
        sent: run.sent,
        received,
        latency_ms,
        verdict,
    }
}

/// One line describing what came back — first message in full, then whether it
/// was alone or the head of a burst.
fn summarize_hits(hits: &[Hit], window: Duration) -> String {
    match hits.split_first() {
        None => format!("nothing in {:.1}s", window.as_secs_f32()),
        Some((first, rest)) => {
            let head = if first.args.is_empty() {
                first.path.clone()
            } else {
                format!("{} {}", first.path, first.args)
            };
            if rest.is_empty() {
                format!("{head}  (1 message)")
            } else {
                format!(
                    "{head}  (burst of {}, last at {} ms)",
                    hits.len(),
                    hits.last().map(|h| h.elapsed_ms).unwrap_or_default()
                )
            }
        }
    }
}

/// Close the current window early. Keyed off the *run's* surface, not the
/// selector, which the operator may have moved since arming.
fn request_stop(tab: &mut ProbeTabState) {
    let Some(run) = tab.in_flight.as_mut() else {
        return;
    };
    match run.surface {
        // Let the listener notice, flush and release its socket; the next poll
        // sees `finished` and closes the row out.
        ConsoleSurface::SdOther => tab.shared.lock().unwrap().stop = true,
        _ => run.window = run.started.elapsed(),
    }
}

// ─── Report ──────────────────────────────────────────────────────────

fn save_report(
    tab: &mut ProbeTabState,
    state: &Arc<RwLock<ConsoleState>>,
    pad_live: bool,
    runtime: &tokio::runtime::Handle,
) {
    let header = match state.try_read() {
        Ok(s) => ReportHeader {
            read_ok: true,
            console_name: s.config.console_name.clone(),
            console_serial: s.config.console_serial.clone(),
            session: s.config.session_filename.clone(),
            family: s.config.family.label().to_string(),
        },
        // The mirror is written hard during an enumeration sweep, so a
        // contended lock is normal — say so rather than reporting empty
        // identity fields as if the console had never named itself.
        Err(_) => ReportHeader::default(),
    };
    let markdown = build_report(tab, &header, pad_live);

    let Some(path) = rfd::FileDialog::new()
        .add_filter("Markdown", &["md"])
        .set_file_name("quantum-probe-report.md")
        .save_file()
    else {
        return;
    };

    let shared = tab.shared.clone();
    let display = path.display().to_string();
    runtime.spawn(async move {
        let msg = match tokio::fs::write(&path, markdown).await {
            Ok(()) => format!("Report saved to {display}"),
            Err(e) => format!("Could not write {display}: {e}"),
        };
        shared.lock().unwrap().report_status = Some(msg);
    });
    tab.status = Some("Writing report…".into());
}

#[derive(Default)]
struct ReportHeader {
    /// False when the state mirror was locked at save time, so its blank
    /// fields mean "not read" rather than "not reported".
    read_ok: bool,
    console_name: String,
    console_serial: String,
    session: Option<String>,
    family: String,
}

impl ReportHeader {
    fn field<'a>(&self, value: &'a str) -> &'a str {
        if !self.read_ok {
            "(app state was busy at save time — save again to capture it)"
        } else if value.trim().is_empty() {
            "(not reported by the console)"
        } else {
            value
        }
    }
}

/// Markdown, so the whole thing can be pasted straight into
/// `Documentation/OSC_FIELD_NOTES.md`.
fn build_report(tab: &ProbeTabState, header: &ReportHeader, pad_live: bool) -> String {
    let mut out = String::new();
    out.push_str("# DiGiCo SD / Quantum OSC surface probe report\n\n");
    out.push_str(&format!(
        "- Generated: {}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S %z")
    ));
    out.push_str(&format!("- App version: {}\n", crate::version::APP_VERSION));
    out.push_str(&format!(
        "- Console family (as configured in the app): {}\n",
        header.field(&header.family)
    ));
    out.push_str(&format!(
        "- Console name: {}\n",
        header.field(&header.console_name)
    ));
    out.push_str(&format!(
        "- Console serial: {}\n",
        header.field(&header.console_serial)
    ));
    out.push_str(&format!(
        "- Session file: {}\n",
        header.field(header.session.as_deref().unwrap_or_default())
    ));
    out.push_str(&format!(
        "- Pad link live while probing: {}\n",
        if pad_live { "yes" } else { "no" }
    ));
    out.push_str(&format!(
        "- `/sd/` target: {}:{} , listening on local port {}\n",
        tab.sd_host.trim(),
        tab.sd_console_port.trim(),
        tab.sd_local_port.trim()
    ));
    out.push_str(&format!("- Probe channel: {}\n\n", tab.probe_channel()));

    out.push_str("## Results\n\n");
    if tab.results.is_empty() {
        out.push_str("_No probes were run._\n\n");
    } else {
        out.push_str(
            "| # | Time | Surface | Test | Path | Sent | Received | Latency | Verdict |\n\
             | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
        );
        for (i, r) in tab.results.iter().enumerate() {
            out.push_str(&format!(
                "| {} | {} | {} | {} | `{}` | {} | {} | {} | **{}** |\n",
                i + 1,
                r.at.format("%H:%M:%S"),
                r.surface.label(),
                r.test.label(),
                md_cell(&r.path),
                md_cell(&r.sent),
                md_cell(&r.received),
                r.latency_ms
                    .map(|ms| format!("{ms} ms"))
                    .unwrap_or_else(|| "—".into()),
                md_cell(&r.verdict.label()),
            ));
        }
        out.push('\n');
    }

    out.push_str("## Not tested\n\n");
    let untested = untested_combinations(tab);
    if untested.is_empty() {
        out.push_str("_Every built-in probe was run on both surfaces._\n\n");
    } else {
        out.push_str(
            "These combinations were never run. They are **not** failures — nothing was \
             learned about them either way:\n\n",
        );
        for line in untested {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }

    out.push_str("## Verdict legend\n\n");
    out.push_str(
        "| Verdict | Meaning |\n| --- | --- |\n\
         | DESK ACTED | The operator watched the console and saw the parameter change. |\n\
         | NO EFFECT | The operator watched the console and nothing changed. |\n\
         | UNCONFIRMED | The message was sent, but nobody recorded what the desk did. |\n\
         | REPLY | Something came back inside the window. |\n\
         | SILENT | The window closed with nothing. A real negative result. |\n\
         | NOT RUN | The probe could not start at all (see the reason). Nothing was learned. |\n\n",
    );

    out.push_str("## Operator notes\n\n");
    if tab.notes.trim().is_empty() {
        out.push_str("_None._\n");
    } else {
        out.push_str(tab.notes.trim());
        out.push('\n');
    }
    out
}

/// Every built-in probe × surface × test with no row in this session.
fn untested_combinations(tab: &ProbeTabState) -> Vec<String> {
    let mut out = Vec::new();
    for surface in [ConsoleSurface::Pad, ConsoleSurface::SdOther] {
        for b in BUILTINS {
            let bare = b.path.replace("{ch}", &tab.probe_channel().to_string());
            let path = format!("{}{bare}", surface.path_prefix());
            for test in [ProbeTest::Write, ProbeTest::Pull, ProbeTest::Push] {
                let ran = tab
                    .results
                    .iter()
                    .any(|r| r.surface == surface && r.test == test && r.path_matches(&path));
                if !ran {
                    out.push(format!(
                        "{} · {} · {} (`{path}`)",
                        surface.label(),
                        b.label,
                        test.label()
                    ));
                }
            }
        }
    }
    out
}

impl ProbeResult {
    /// Whether this row is about `path`. A pull row records the `/?` form, so
    /// the suffix is ignored when matching back to the probe it came from.
    fn path_matches(&self, path: &str) -> bool {
        self.path == path || self.path.trim_end_matches("/?") == path
    }
}

/// Markdown table cells cannot carry a raw pipe or a newline.
fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(test: ProbeTest, hits: Vec<Hit>, error: Option<String>) -> ProbeResult {
        finish_run(InFlight {
            surface: ConsoleSurface::SdOther,
            test,
            path: "/sd/Input_Channels/1/fader".into(),
            reply_path: "/sd/Input_Channels/1/fader".into(),
            sent: "1.000".into(),
            started: Instant::now(),
            started_wall: Local::now(),
            window: Duration::from_millis(1500),
            hits,
            error,
        })
    }

    fn hit(ms: u64, path: &str) -> Hit {
        Hit {
            elapsed_ms: ms,
            path: path.into(),
            args: "0.500".into(),
        }
    }

    #[test]
    fn silence_and_failure_are_different_verdicts() {
        // The whole exercise depends on these two never collapsing together:
        // one is a measurement, the other is a missing measurement.
        let silent = run(ProbeTest::Push, vec![], None);
        assert_eq!(silent.verdict, Verdict::Silent);
        assert!(silent.received.contains("nothing"));

        let failed = run(ProbeTest::Push, vec![], Some("cannot bind".into()));
        assert_eq!(failed.verdict, Verdict::NotRun("cannot bind".into()));
        assert!(failed.latency_ms.is_none());
    }

    #[test]
    fn a_write_is_never_self_certified() {
        // Even a chatty echo does not prove the desk acted — only the operator
        // can close a WRITE row.
        let echoed = run(
            ProbeTest::Write,
            vec![hit(4, "/sd/Input_Channels/1/fader")],
            None,
        );
        assert_eq!(echoed.verdict, Verdict::Unconfirmed);
        assert_eq!(echoed.latency_ms, Some(4));
    }

    #[test]
    fn pull_reply_records_latency_and_burst_shape() {
        let single = run(
            ProbeTest::Pull,
            vec![hit(12, "/sd/Input_Channels/1/fader")],
            None,
        );
        assert_eq!(single.verdict, Verdict::Replied);
        assert!(single.received.contains("(1 message)"));

        let burst = run(
            ProbeTest::Push,
            vec![
                hit(5, "/sd/Input_Channels/1/fader"),
                hit(6, "/sd/Input_Channels/1/fader"),
                hit(40, "/sd/Input_Channels/1/mute"),
            ],
            None,
        );
        assert_eq!(burst.latency_ms, Some(5));
        assert!(burst.received.contains("burst of 3"));
        assert!(burst.received.contains("40 ms"));
    }

    #[test]
    fn pad_push_ignores_the_apps_own_keepalive_replies() {
        let log = OscLog::new();
        let run = InFlight {
            surface: ConsoleSurface::Pad,
            test: ProbeTest::Push,
            path: "/Input_Channels/1/fader".into(),
            reply_path: "/Input_Channels/1/fader".into(),
            sent: "(nothing — listening)".into(),
            started: Instant::now(),
            started_wall: Local::now(),
            window: Duration::from_secs(20),
            hits: Vec::new(),
            error: None,
        };
        log.log_ipad_in("/Console/Name", "\"SD12\"");
        log.log_ipad_in("/Input_Channels/1/fader", "-8.000");
        // Outbound traffic is ours, never a push.
        log.log_ipad_out("/Input_Channels/1/fader/?", "");

        let hits = collect_pad_hits(&log, &run);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "/Input_Channels/1/fader");
    }

    #[test]
    fn surface_prefix_applies_to_probe_paths() {
        let mut tab = ProbeTabState {
            channel: "7".into(),
            ..Default::default()
        };
        assert_eq!(
            tab.resolve("/Input_Channels/{ch}/fader"),
            "/Input_Channels/7/fader"
        );
        tab.surface = ConsoleSurface::SdOther;
        assert_eq!(
            tab.resolve("/Input_Channels/{ch}/fader"),
            "/sd/Input_Channels/7/fader"
        );
    }

    #[test]
    fn arg_kinds_keep_the_boolean_encodings_distinct() {
        assert_eq!(
            ArgKind::Float.build("1").unwrap(),
            vec![OscType::Float(1.0)]
        );
        assert_eq!(ArgKind::Int.build("1").unwrap(), vec![OscType::Int(1)]);
        assert_eq!(
            ArgKind::OscTrue.build("").unwrap(),
            vec![OscType::Bool(true)]
        );
        assert!(ArgKind::NoArg.build("").unwrap().is_empty());
        assert!(ArgKind::Float.build("banana").is_err());
    }

    #[test]
    fn report_lists_untested_combinations_by_name() {
        let mut tab = ProbeTabState::default();
        let all = untested_combinations(&tab);
        // Six built-ins × three tests × two surfaces, none run yet.
        assert_eq!(all.len(), BUILTINS.len() * 3 * 2);

        tab.results.push(ProbeResult {
            at: Local::now(),
            surface: ConsoleSurface::Pad,
            test: ProbeTest::Pull,
            // Pull rows record the queried form; the match strips it.
            path: "/Input_Channels/1/fader/?".into(),
            sent: "(query, no arg)".into(),
            received: "nothing in 1.5s".into(),
            latency_ms: None,
            verdict: Verdict::Silent,
        });
        let after = untested_combinations(&tab);
        assert_eq!(after.len(), all.len() - 1);

        let md = build_report(&tab, &ReportHeader::default(), true);
        assert!(md.contains("SILENT"));
        assert!(md.contains("## Not tested"));
        assert!(md.contains("DiGiCo Pad · Fader · PUSH"));
    }

    #[test]
    fn report_header_separates_unread_state_from_unreported_fields() {
        let tab = ProbeTabState::default();
        // Mirror was locked: the blanks are ours, not the console's.
        let busy = build_report(&tab, &ReportHeader::default(), false);
        assert!(busy.contains("app state was busy"));

        let read = ReportHeader {
            read_ok: true,
            console_name: "Quantum 338".into(),
            ..ReportHeader::default()
        };
        let md = build_report(&tab, &read, false);
        assert!(md.contains("Console name: Quantum 338"));
        assert!(md.contains("Console serial: (not reported by the console)"));
        assert!(!md.contains("app state was busy"));
    }

    #[test]
    fn report_cells_survive_pipes_in_paths() {
        assert_eq!(md_cell("a|b\nc"), "a\\|b c");
    }
}
