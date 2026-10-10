//! External cue-trigger dispatcher (v0.1.2).
//!
//! Fires a cue's [`CueTrigger`]s when it is recalled — fire-and-forget OSC to
//! QLab / LiveProfessor / custom targets, and MIDI out the active port. It is
//! attached to the [`SnapshotEngine`](crate::console::snapshot_engine) via
//! `set_trigger_dispatcher` and invoked at the very end of `recall_cue`, after
//! the console memory row + snapshot overlay have been applied — external I/O
//! must never delay or interfere with the cue's primary effect.
//!
//! Each send is independent and best-effort: a failed OSC/MIDI send is logged
//! and skipped, never propagated, so one dead target can't abort the recall or
//! the remaining triggers. This mirrors the console `OscSender`'s "drop on
//! timeout, never wedge" philosophy.
//!
//! OSC does **not** go through the console sender (which is pinned to the desk
//! address). The dispatcher binds its own ephemeral UDP socket per send — the
//! same fire-and-forget pattern as [`crate::osc::qlab_client`].

use std::sync::Arc;

use rosc::{OscMessage, OscPacket};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::console::cue_manager::CueManager;
use crate::console::midi_engine::MidiEngine;
use crate::model::cue_trigger::{CueTrigger, TriggerAction, format_osc_args};
use crate::model::osc_log::OscLog;
use crate::osc::godot_cue_builder;

/// Fires external cue triggers. Cheap to clone via `Arc`; constructed once at
/// app/daemon startup and shared with the snapshot engine.
pub struct TriggerDispatcher {
    midi: Arc<MidiEngine>,
    /// Resolves `TriggerAction::Osc { target_id }` to a host:port. The same
    /// process-wide cue manager the show file loads into.
    cue_manager: Arc<RwLock<CueManager>>,
    /// Optional OSC-log handle so external triggers appear in the OSC Log tab.
    osc_log: Option<OscLog>,
    /// Where Go.dot triggers go: the show's Go.dot address, set on connect
    /// when Go.dot is the show's cueing system, `None` otherwise (a Go.dot
    /// trigger then sends nothing and the log says why).
    godot_target: std::sync::RwLock<Option<(String, u16)>>,
}

impl TriggerDispatcher {
    pub fn new(
        midi: Arc<MidiEngine>,
        cue_manager: Arc<RwLock<CueManager>>,
        osc_log: Option<OscLog>,
    ) -> Arc<Self> {
        Arc::new(Self {
            midi,
            cue_manager,
            osc_log,
            godot_target: std::sync::RwLock::new(None),
        })
    }

    /// Point Go.dot triggers at `target` (host, port), or nowhere.
    pub fn set_godot_target(&self, target: Option<(String, u16)>) {
        *self.godot_target.write().unwrap_or_else(|e| e.into_inner()) = target;
    }

    /// Where Go.dot triggers go now.
    pub fn godot_target(&self) -> Option<(String, u16)> {
        self.godot_target
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Fire all enabled triggers of a cue with no snapshot captured into
    /// Go.dot. See [`fire_for`](Self::fire_for).
    pub async fn fire(&self, triggers: &[CueTrigger]) {
        self.fire_for(triggers, None).await;
    }

    /// Fire all enabled triggers. `snapshot_cue_id` is the Go.dot cue
    /// identifier kept on the snapshot the cue recalls, if it has one: what
    /// "this snapshot's cue" means to a Go.dot trigger. Never returns an
    /// error — failures are logged.
    pub async fn fire_for(&self, triggers: &[CueTrigger], snapshot_cue_id: Option<&str>) {
        if triggers.is_empty() {
            return;
        }
        // Snapshot the OSC targets once so we don't hold the cue lock across
        // the (awaiting) sends, and don't re-lock per trigger.
        let targets = self.cue_manager.read().await.osc_targets.clone();

        for trig in triggers.iter().filter(|t| t.enabled) {
            match &trig.action {
                TriggerAction::Osc {
                    target_id,
                    host,
                    port,
                    path,
                    args,
                } => {
                    // Resolve destination: named target wins, else inline.
                    let dest = match target_id {
                        Some(id) => targets.get(id).map(|t| (t.host.clone(), t.port)),
                        None => match (host, port) {
                            (Some(h), Some(p)) => Some((h.clone(), *p)),
                            _ => None,
                        },
                    };
                    let Some((host, port)) = dest else {
                        warn!(
                            label = %trig.label,
                            "External OSC trigger has no resolvable destination — skipped"
                        );
                        continue;
                    };
                    let rosc_args = args.iter().map(|a| a.to_osc()).collect();
                    match send_osc(&host, port, path, rosc_args).await {
                        Ok(()) => {
                            if let Some(log) = &self.osc_log {
                                log.log_external_out(path, &format_osc_args(args));
                            }
                            debug!(label = %trig.label, %host, port, path, "Fired OSC trigger");
                        }
                        Err(e) => warn!(
                            label = %trig.label, %host, port, path,
                            "External OSC trigger send failed: {e}"
                        ),
                    }
                }
                TriggerAction::Midi { message } => {
                    self.midi.send(message.to_bytes());
                    if let Some(log) = &self.osc_log {
                        log.log_external_out("MIDI", &message.describe());
                    }
                    debug!(label = %trig.label, msg = %message.describe(), "Fired MIDI trigger");
                }
                TriggerAction::GoDot { verb } => {
                    let Some((host, port)) = self.godot_target() else {
                        warn!(
                            label = %trig.label,
                            "Go.dot trigger skipped: Go.dot is not this show's cueing system"
                        );
                        continue;
                    };
                    let msgs = match godot_cue_builder::verb_messages(verb, snapshot_cue_id) {
                        Ok(msgs) => msgs,
                        Err(why) => {
                            warn!(label = %trig.label, "Go.dot trigger: {why}");
                            continue;
                        }
                    };
                    match send_godot(&host, port, &msgs).await {
                        Ok(()) => {
                            if let Some(log) = &self.osc_log {
                                for msg in &msgs {
                                    log.log_external_out(
                                        &msg.addr,
                                        &godot_cue_builder::atoms_of(&msg.args).unwrap_or_default(),
                                    );
                                }
                            }
                            debug!(label = %trig.label, %host, port, verb = %verb.describe(), "Fired Go.dot trigger");
                        }
                        Err(e) => warn!(
                            label = %trig.label, %host, port,
                            "Go.dot trigger send failed: {e}"
                        ),
                    }
                }
            }
        }
    }
}

/// Send a Go.dot verb's messages, fire-and-forget: one alone, the GO-at
/// pairs as one bundle so Go.dot applies them in order in one tick.
async fn send_godot(
    host: &str,
    port: u16,
    msgs: &[OscMessage],
) -> Result<(), crate::osc::godot_client::GoDotError> {
    crate::osc::godot_client::GoDotClient::new(host, port)
        .await?
        .send_all(msgs)
        .await
}

/// Bind an ephemeral UDP socket and send a single OSC message, fire-and-forget.
async fn send_osc(
    host: &str,
    port: u16,
    path: &str,
    args: Vec<rosc::OscType>,
) -> std::io::Result<()> {
    let dest = format!("{host}:{port}");
    let packet = OscPacket::Message(OscMessage {
        addr: path.to_string(),
        args,
    });
    let buf = rosc::encoder::encode(&packet)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.send_to(&buf, &dest).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::cue_trigger::{CueTrigger, MidiMessage, OscArg, OscTarget, TriggerAction};
    use crate::model::snapshot::CueList;
    use std::time::Duration;
    use uuid::Uuid;

    fn dispatcher_with(
        cue_manager: Arc<RwLock<CueManager>>,
        log: Option<OscLog>,
    ) -> Arc<TriggerDispatcher> {
        TriggerDispatcher::new(MidiEngine::new(), cue_manager, log)
    }

    #[tokio::test]
    async fn fires_osc_to_inline_target() {
        // A stand-in receiver plays the role of QLab.
        let recv = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = recv.local_addr().unwrap().port();

        let cue_mgr = Arc::new(RwLock::new(CueManager::new(CueList::default())));
        let log = OscLog::new();
        let disp = dispatcher_with(cue_mgr, Some(log.clone()));

        let trig = CueTrigger::new(TriggerAction::Osc {
            target_id: None,
            host: Some("127.0.0.1".into()),
            port: Some(port),
            path: "/go".into(),
            args: vec![OscArg::Str("Q3".into())],
        });
        disp.fire(&[trig]).await;

        // Packet arrives + is logged as External.
        let mut buf = [0u8; 1024];
        let got = tokio::time::timeout(Duration::from_millis(500), recv.recv_from(&mut buf)).await;
        assert!(got.is_ok(), "expected an OSC packet at the receiver");
        let (n, _) = got.unwrap().unwrap();
        let (_, packet) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
        match packet {
            OscPacket::Message(m) => assert_eq!(m.addr, "/go"),
            _ => panic!("expected message"),
        }
        assert_eq!(log.len(), 1);
    }

    #[tokio::test]
    async fn fires_osc_to_named_target() {
        let recv = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = recv.local_addr().unwrap().port();

        let target = OscTarget::new("QLab", "127.0.0.1", port);
        let target_id = target.id;
        let mut mgr = CueManager::new(CueList::default());
        mgr.add_osc_target(target);
        let cue_mgr = Arc::new(RwLock::new(mgr));
        let disp = dispatcher_with(cue_mgr, None);

        let trig = CueTrigger::new(TriggerAction::Osc {
            target_id: Some(target_id),
            host: None,
            port: None,
            path: "/stop".into(),
            args: vec![],
        });
        disp.fire(&[trig]).await;

        let mut buf = [0u8; 1024];
        let got = tokio::time::timeout(Duration::from_millis(500), recv.recv_from(&mut buf)).await;
        assert!(got.is_ok(), "expected an OSC packet via the named target");
    }

    #[tokio::test]
    async fn disabled_trigger_is_skipped() {
        let recv = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = recv.local_addr().unwrap().port();

        let cue_mgr = Arc::new(RwLock::new(CueManager::new(CueList::default())));
        let disp = dispatcher_with(cue_mgr, None);

        let mut trig = CueTrigger::new(TriggerAction::Osc {
            target_id: None,
            host: Some("127.0.0.1".into()),
            port: Some(port),
            path: "/go".into(),
            args: vec![],
        });
        trig.enabled = false;
        disp.fire(&[trig]).await;

        let mut buf = [0u8; 1024];
        let got = tokio::time::timeout(Duration::from_millis(150), recv.recv_from(&mut buf)).await;
        assert!(got.is_err(), "disabled trigger must not send");
    }

    #[tokio::test]
    async fn unresolvable_target_does_not_panic() {
        // target_id that isn't in the map → skipped, no panic, other triggers
        // still fire.
        let recv = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = recv.local_addr().unwrap().port();
        let cue_mgr = Arc::new(RwLock::new(CueManager::new(CueList::default())));
        let disp = dispatcher_with(cue_mgr, None);

        let bad = CueTrigger::new(TriggerAction::Osc {
            target_id: Some(Uuid::new_v4()),
            host: None,
            port: None,
            path: "/go".into(),
            args: vec![],
        });
        let good = CueTrigger::new(TriggerAction::Osc {
            target_id: None,
            host: Some("127.0.0.1".into()),
            port: Some(port),
            path: "/go".into(),
            args: vec![],
        });
        disp.fire(&[bad, good]).await;

        let mut buf = [0u8; 1024];
        let got = tokio::time::timeout(Duration::from_millis(500), recv.recv_from(&mut buf)).await;
        assert!(got.is_ok(), "the resolvable trigger should still fire");
    }

    /// What a stand-in Go.dot receives within `wait`: each datagram's
    /// addresses (a bundle's in order).
    async fn received(recv: &UdpSocket, wait: Duration) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        let mut buf = [0u8; 2048];
        while let Ok(Ok((n, _))) = tokio::time::timeout(wait, recv.recv_from(&mut buf)).await {
            let (_, packet) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
            out.push(match packet {
                OscPacket::Message(m) => vec![m.addr],
                OscPacket::Bundle(b) => b
                    .content
                    .into_iter()
                    .map(|p| match p {
                        OscPacket::Message(m) => m.addr,
                        OscPacket::Bundle(_) => panic!("nested bundle"),
                    })
                    .collect(),
            });
        }
        out
    }

    fn godot(verb: crate::model::cue_trigger::GoDotVerb) -> CueTrigger {
        CueTrigger::new(TriggerAction::GoDot { verb })
    }

    #[tokio::test]
    async fn godot_triggers_go_to_the_shows_godot() {
        use crate::model::cue_trigger::GoDotVerb;
        let recv = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = recv.local_addr().unwrap().port();
        let cue_mgr = Arc::new(RwLock::new(CueManager::new(CueList::default())));
        let log = OscLog::new();
        let disp = dispatcher_with(cue_mgr, Some(log.clone()));
        disp.set_godot_target(Some(("127.0.0.1".into(), port)));

        disp.fire_for(
            &[
                godot(GoDotVerb::GoAtSnapshotCue),
                godot(GoDotVerb::FireCueNumber("12".into())),
                godot(GoDotVerb::Esc),
            ],
            Some("0123ABCD"),
        )
        .await;
        let got = received(&recv, Duration::from_millis(300)).await;
        assert_eq!(
            got,
            vec![
                vec!["/godot/cmd/standby/set", "/godot/cmd/go"],
                vec!["/godot/cmd/cue/fireNumber"],
                vec!["/godot/cmd/run/stopAll"],
            ]
        );
        assert_eq!(log.len(), 4, "each message is in the OSC log");
    }

    #[tokio::test]
    async fn a_snapshot_never_captured_sends_nothing_to_godot() {
        use crate::model::cue_trigger::GoDotVerb;
        let recv = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = recv.local_addr().unwrap().port();
        let cue_mgr = Arc::new(RwLock::new(CueManager::new(CueList::default())));
        let disp = dispatcher_with(cue_mgr, None);
        disp.set_godot_target(Some(("127.0.0.1".into(), port)));

        disp.fire_for(
            &[
                godot(GoDotVerb::FireSnapshotCue),
                godot(GoDotVerb::GoAtSnapshotCue),
                godot(GoDotVerb::Go),
            ],
            None,
        )
        .await;
        let got = received(&recv, Duration::from_millis(300)).await;
        assert_eq!(
            got,
            vec![vec!["/godot/cmd/go"]],
            "only GO needs no snapshot"
        );
    }

    #[tokio::test]
    async fn godot_triggers_need_godot_as_the_cueing_system() {
        use crate::model::cue_trigger::GoDotVerb;
        let recv = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let cue_mgr = Arc::new(RwLock::new(CueManager::new(CueList::default())));
        let disp = dispatcher_with(cue_mgr, None);
        // No target: a QLab show.
        disp.fire_for(&[godot(GoDotVerb::Go)], Some("0123ABCD"))
            .await;
        assert!(received(&recv, Duration::from_millis(150)).await.is_empty());
        assert_eq!(disp.godot_target(), None);
    }

    #[tokio::test]
    async fn midi_trigger_logs_without_panicking() {
        // No MIDI port connected → send is a logged no-op; fire must not panic
        // and should still record the External log line.
        let cue_mgr = Arc::new(RwLock::new(CueManager::new(CueList::default())));
        let log = OscLog::new();
        let disp = dispatcher_with(cue_mgr, Some(log.clone()));
        let trig = CueTrigger::new(TriggerAction::Midi {
            message: MidiMessage::ProgramChange {
                channel: 1,
                program: 5,
            },
        });
        disp.fire(&[trig]).await;
        assert_eq!(log.len(), 1);
    }
}
