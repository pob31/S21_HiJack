//! The app's standing in Go.dot while connected: it declares itself as the
//! device `/s21` (contract `docs/godot-authoring-protocol-0.1.md` in the
//! Go.dot repository, §3 and §7.2), and declares again whenever its menu
//! changes, so Go.dot fetches the new one.
//!
//! One task per connection, cancelled with it, for the GUI and headless
//! alike. It says what happened through `notify`: the GUI turns that into a
//! status line, headless into the log.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::console::cue_manager::CueManager;
use crate::console::macro_manager::MacroManager;
use crate::osc::S21_ROOT;
use crate::osc::godot_client::{DESCRIBED_TIMEOUT, GoDotClient};
use crate::osc::godot_reply::GoDotAck;
use crate::web::oscquery::MenuSource;

/// The device's name in Go.dot's Network tab, at its creation.
pub const DEVICE_NAME: &str = "S21_HiJack";

/// How often the menu is compared with the one last declared.
pub const MENU_POLL: Duration = Duration::from_secs(1);

/// How long after a failed declare the app tries again, menu change or not:
/// Go.dot may have been started after the app connected.
pub const RETRY_AFTER: Duration = Duration::from_secs(10);

/// Where Go.dot is and what the app tells it about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoDotLink {
    /// Go.dot's IP (empty: this machine) and OSC port.
    pub godot_ip: String,
    pub godot_port: u16,
    /// Where the app receives OSC: Go.dot's cues and answers come here.
    pub trigger_port: u16,
    /// Where the app serves its description, or 0 when the web server is
    /// off (Go.dot then fetches nothing and offers no menus).
    pub query_port: u16,
}

/// What a declare came to.
#[derive(Clone, Debug, PartialEq)]
pub enum GoDotNotice {
    /// Go.dot made (`created`) or refreshed (`updated`) the device.
    Declared { device_id: String, outcome: String },
    /// Go.dot fetched the description: its node count, or the problem.
    Described { node_count: i64, problem: String },
    /// The declare was answered and Go.dot never said it read the menu.
    NotDescribed,
    /// The declare failed: no answer, a refusal, or a bad address.
    Failed(String),
}

impl GoDotNotice {
    /// The status line for this notice.
    pub fn sentence(&self) -> String {
        match self {
            Self::Declared { outcome, .. } => {
                format!("Declared in Go.dot as {S21_ROOT} ({outcome})")
            }
            Self::Described {
                node_count,
                problem,
            } if problem.is_empty() => {
                format!("Declared in Go.dot as {S21_ROOT}: its menu has {node_count} nodes")
            }
            Self::Described { problem, .. } => {
                format!(
                    "Declared in Go.dot as {S21_ROOT}, but it could not read the menu: {problem}"
                )
            }
            Self::NotDescribed => format!(
                "Declared in Go.dot as {S21_ROOT}, but it did not say whether it read the menu"
            ),
            Self::Failed(reason) => format!("Not declared in Go.dot: {reason}"),
        }
    }

    /// Whether this notice reports something wrong.
    pub fn is_problem(&self) -> bool {
        match self {
            Self::Declared { .. } => false,
            Self::Described { problem, .. } => !problem.is_empty(),
            Self::NotDescribed | Self::Failed(_) => true,
        }
    }
}

/// The menu the app describes right now, one lock at a time.
pub async fn current_menu(
    cue_manager: &Arc<RwLock<CueManager>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
) -> MenuSource {
    let (snapshot_names, cue_numbers) = MenuSource::of_cues(&*cue_manager.read().await);
    let macro_names = MenuSource::of_macros(&*macro_manager.read().await);
    MenuSource::new(snapshot_names, cue_numbers, macro_names)
}

/// Declare once and report: the declare's answer, then (when the app
/// describes itself) whether Go.dot read the menu. Returns whether Go.dot
/// accepted the declare.
pub async fn declare_once(link: &GoDotLink, notify: &(dyn Fn(GoDotNotice) + Send + Sync)) -> bool {
    let client = match GoDotClient::new(&link.godot_ip, link.godot_port).await {
        Ok(client) => client,
        Err(e) => {
            notify(GoDotNotice::Failed(e.to_string()));
            return false;
        }
    };
    let declaration = match client
        .declare(S21_ROOT, link.trigger_port, link.query_port, DEVICE_NAME)
        .await
    {
        Ok(declaration) => declaration,
        Err(e) => {
            notify(GoDotNotice::Failed(e.to_string()));
            return false;
        }
    };
    if let GoDotAck::Declared { id, outcome } = &declaration.declared {
        notify(GoDotNotice::Declared {
            device_id: id.clone(),
            outcome: outcome.clone(),
        });
    }
    if link.query_port != 0 {
        match declaration.described(DESCRIBED_TIMEOUT).await {
            Some(GoDotAck::Described {
                node_count,
                problem,
                ..
            }) => notify(GoDotNotice::Described {
                node_count,
                problem,
            }),
            _ => notify(GoDotNotice::NotDescribed),
        }
    }
    true
}

/// Declare now, then again whenever the menu changes (compared every
/// [`MENU_POLL`]), and again [`RETRY_AFTER`] a failed declare, until
/// `cancel`. A failure is reported once until it changes or a declare
/// succeeds, so a Go.dot that is off does not fill the log.
pub async fn run_declare_loop(
    link: GoDotLink,
    cue_manager: Arc<RwLock<CueManager>>,
    macro_manager: Arc<RwLock<MacroManager>>,
    cancel: CancellationToken,
    notify: impl Fn(GoDotNotice) + Send + Sync + 'static,
) {
    info!(?link, "Go.dot: declaring {S21_ROOT}");
    let last_failure = std::sync::Mutex::new(None::<GoDotNotice>);
    let report = |notice: GoDotNotice| {
        let mut last = last_failure.lock().unwrap_or_else(|e| e.into_inner());
        if let GoDotNotice::Failed(_) = &notice {
            if last.as_ref() == Some(&notice) {
                return;
            }
            *last = Some(notice.clone());
        } else {
            *last = None;
        }
        drop(last);
        notify(notice);
    };

    let mut declared_signature: Option<u64> = None;
    let mut retry_at: Option<tokio::time::Instant> = None;
    let mut poll = tokio::time::interval(MENU_POLL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = poll.tick() => {}
        }
        let signature = current_menu(&cue_manager, &macro_manager).await.signature();
        let due = match (declared_signature, retry_at) {
            (None, None) => true, // the first declare
            (_, Some(at)) => tokio::time::Instant::now() >= at,
            (Some(declared), None) => declared != signature,
        };
        if !due {
            continue;
        }
        let ok = tokio::select! {
            _ = cancel.cancelled() => break,
            ok = declare_once(&link, &report) => ok,
        };
        if ok {
            declared_signature = Some(signature);
            retry_at = None;
        } else {
            warn!(
                "Go.dot: declare failed; trying again in {} s",
                RETRY_AFTER.as_secs()
            );
            retry_at = Some(tokio::time::Instant::now() + RETRY_AFTER);
        }
    }
    info!("Go.dot: declare loop stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::snapshot::{CueList, ScopeTemplate, Snapshot, SnapshotData, SnapshotKind};
    use crate::osc::godot_reply;
    use rosc::{OscMessage, OscPacket, OscType};
    use tokio::net::UdpSocket;

    /// A stand-in Go.dot that answers each declare through the global bus
    /// (as the trigger listener would) and counts them.
    fn fake_godot(socket: UdpSocket, declares: Arc<std::sync::atomic::AtomicUsize>) {
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            while let Ok((n, _)) = socket.recv_from(&mut buf).await {
                let Ok((_, OscPacket::Message(msg))) = rosc::decoder::decode_udp(&buf[..n]) else {
                    continue;
                };
                if msg.addr != "/godot/cmd/mount/declare" {
                    continue;
                }
                assert_eq!(
                    msg.args,
                    vec![
                        OscType::String("/s21".into()),
                        OscType::Int(53001),
                        OscType::Int(0),
                        OscType::String("S21_HiJack".into()),
                    ]
                );
                declares.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let answer = OscMessage {
                    addr: "/godot/declared".into(),
                    args: vec![
                        OscType::String("DEV00001".into()),
                        OscType::String("updated".into()),
                    ],
                };
                let ack = godot_reply::parse_answer(&answer.addr, &answer.args).unwrap();
                godot_reply::global().deliver(ack).await;
            }
        });
    }

    #[test]
    fn sentences() {
        assert_eq!(
            GoDotNotice::Declared {
                device_id: "D".into(),
                outcome: "created".into()
            }
            .sentence(),
            "Declared in Go.dot as /s21 (created)"
        );
        assert_eq!(
            GoDotNotice::Described {
                node_count: 9,
                problem: String::new()
            }
            .sentence(),
            "Declared in Go.dot as /s21: its menu has 9 nodes"
        );
        assert!(GoDotNotice::Failed("x".into()).is_problem());
        assert!(
            !GoDotNotice::Declared {
                device_id: String::new(),
                outcome: "updated".into()
            }
            .is_problem()
        );
    }

    /// Declares on start, again when a snapshot is added, and not while the
    /// menu stays the same.
    #[tokio::test]
    async fn declares_again_when_the_menu_changes() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let declares = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        fake_godot(socket, declares.clone());

        let cue_manager = Arc::new(RwLock::new(CueManager::new(CueList::default())));
        let macro_manager = Arc::new(RwLock::new(MacroManager::new()));
        let cancel = CancellationToken::new();
        let notices = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = notices.clone();
        tokio::spawn(run_declare_loop(
            GoDotLink {
                godot_ip: "127.0.0.1".into(),
                godot_port: port,
                trigger_port: 53001,
                query_port: 0,
            },
            cue_manager.clone(),
            macro_manager,
            cancel.clone(),
            move |n| seen.lock().unwrap().push(n),
        ));

        let count = || declares.load(std::sync::atomic::Ordering::SeqCst);
        let wait_for = |n: usize| async move {
            for _ in 0..50 {
                if count() >= n {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        wait_for(1).await;
        assert_eq!(count(), 1, "declared on start");
        tokio::time::sleep(MENU_POLL + Duration::from_millis(300)).await;
        assert_eq!(count(), 1, "the same menu is not declared again");

        let snap = Snapshot::new(
            "Verse 1".into(),
            ScopeTemplate::new("S".into(), vec![]),
            SnapshotData::new(),
            SnapshotKind::ApplyOnSave,
        );
        cue_manager.write().await.snapshots.insert(snap.id, snap);
        tokio::time::sleep(MENU_POLL).await;
        wait_for(2).await;
        assert_eq!(count(), 2, "a new snapshot is a new menu");
        cancel.cancel();

        let notices = notices.lock().unwrap();
        assert!(notices.iter().all(|n| matches!(
            n,
            GoDotNotice::Declared { outcome, .. } if outcome == "updated"
        )));
    }
}
