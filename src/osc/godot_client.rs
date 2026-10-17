//! OSC client for Go.dot's authoring protocol: declare the app as a device,
//! capture cues, and send the firing verbs.
//!
//! Pairs with [`crate::osc::godot_cue_builder`], the pure half that says
//! what to send. The contract is `docs/godot-authoring-protocol-0.1.md` in
//! the Go.dot repository.
//!
//! Requests leave from an ephemeral UDP socket. Go.dot's answers do **not**
//! come back to it: Go.dot answers the device the app declared, at the
//! declared port, which is the trigger listener's. The listener hands each
//! answer over through [`crate::osc::godot_reply`], keyed by the cue's
//! identifier (or the declare's fixed keys), and the client waits on that.
//! So an answer can only arrive while the app is connected (the listener
//! runs with the connection) and after it has declared itself.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use rosc::{OscBundle, OscMessage, OscPacket};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tracing::{debug, info};

use super::godot_cue_builder::{self, GoDotCue};
use super::godot_reply::{DECLARED_KEY, DESCRIBED_KEY, GoDotAck, GoDotReplyBus};

/// How long to wait for each answer Go.dot owes: one per datagram (contract
/// §4.5).
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);

/// How long to wait for `/godot/described` after the declare's answer:
/// Go.dot gives its fetch of the description 5 s, then answers.
pub const DESCRIBED_TIMEOUT: Duration = Duration::from_secs(8);

/// Operator-facing reminder appended to an unanswered request.
pub const GODOT_ANSWER_HELP: &str = "Go.dot answers on the trigger port while the app is connected, \
     to the device the app declared on connect. Check Go.dot's address and port in Setup, that the \
     app is connected with Go.dot chosen, and that Go.dot's OSC filter lets this computer in.";

/// Errors from the Go.dot client.
#[derive(Debug)]
pub enum GoDotError {
    /// The address did not parse, or the socket failed.
    Io(std::io::Error),
    /// OSC encoding failed.
    Encode(String),
    /// An answer did not come within [`ANSWER_TIMEOUT`]; names what was
    /// asked.
    Timeout(String),
    /// Go.dot refused, with its reason (`locked`, `unknown-id`,
    /// `bad-address`, `bad-value`, `type-mismatch`).
    Refused(String),
    /// A capture with no message, which Go.dot would refuse.
    NothingToCapture,
}

impl std::fmt::Display for GoDotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "Go.dot I/O error: {e}"),
            Self::Encode(e) => write!(f, "Go.dot encode error: {e}"),
            Self::Timeout(what) => write!(
                f,
                "Go.dot did not answer the {what} within {} s. {GODOT_ANSWER_HELP}",
                ANSWER_TIMEOUT.as_secs()
            ),
            Self::Refused(reason) => write!(f, "Go.dot refused: {reason}"),
            Self::NothingToCapture => write!(
                f,
                "nothing to capture: none of the snapshot's parameters has a GP OSC address"
            ),
        }
    }
}

impl std::error::Error for GoDotError {}

impl From<std::io::Error> for GoDotError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Go.dot's address from the Setup fields: the IP trimmed, empty meaning
/// this machine, IPv4 or IPv6.
pub fn godot_addr(ip: &str, port: u16) -> Result<SocketAddr, GoDotError> {
    let ip = ip.trim();
    let ip: IpAddr = if ip.is_empty() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        ip.parse().map_err(|e| {
            GoDotError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid Go.dot address '{ip}': {e}"),
            ))
        })?
    };
    Ok(SocketAddr::new(ip, port))
}

/// A declare Go.dot has accepted, and its description still to come.
pub struct Declaration {
    /// The `/godot/declared` answer (`created` or `updated`).
    pub declared: GoDotAck,
    /// Waits for `/godot/described`; `None` when the declare described
    /// nothing (query port 0).
    described: Option<oneshot::Receiver<GoDotAck>>,
}

impl Declaration {
    /// Wait for `/godot/described`. `None` when nothing was to be described,
    /// or it did not come within `timeout`.
    pub async fn described(self, timeout: Duration) -> Option<GoDotAck> {
        let rx = self.described?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(ack)) => Some(ack),
            Ok(Err(_)) | Err(_) => None,
        }
    }
}

/// Go.dot OSC client: an ephemeral socket aimed at Go.dot. Cheap to make;
/// made for one export, one declare or one trigger.
pub struct GoDotClient {
    socket: UdpSocket,
    dest: SocketAddr,
    /// Where answers are delivered (the trigger listener feeds it). The
    /// process-global bus in production; tests inject their own.
    bus: GoDotReplyBus,
    /// How long to wait for each answer.
    timeout: Duration,
}

impl GoDotClient {
    /// Bind an ephemeral socket aimed at `ip:port` (empty IP: this machine).
    pub async fn new(ip: &str, port: u16) -> Result<Self, GoDotError> {
        let dest = godot_addr(ip, port)?;
        let bind: SocketAddr = if dest.is_ipv6() {
            "[::]:0".parse().expect("a literal address")
        } else {
            "0.0.0.0:0".parse().expect("a literal address")
        };
        Ok(Self {
            socket: UdpSocket::bind(bind).await?,
            dest,
            bus: crate::osc::godot_reply::global(),
            timeout: ANSWER_TIMEOUT,
        })
    }

    /// Where this client sends.
    pub fn dest(&self) -> SocketAddr {
        self.dest
    }

    /// Send one message, fire-and-forget.
    pub async fn send(&self, msg: &OscMessage) -> Result<(), GoDotError> {
        self.send_packet(&OscPacket::Message(msg.clone())).await
    }

    /// Send messages as one bundle, applied in order in one tick: "GO at" a
    /// cue is `standby.set` then `go`.
    pub async fn send_bundle(&self, msgs: &[OscMessage]) -> Result<(), GoDotError> {
        let bundle = OscBundle {
            timetag: (0, 1).into(), // immediately
            content: msgs.iter().cloned().map(OscPacket::Message).collect(),
        };
        self.send_packet(&OscPacket::Bundle(bundle)).await
    }

    /// Send a list of messages: one alone, several as a bundle.
    pub async fn send_all(&self, msgs: &[OscMessage]) -> Result<(), GoDotError> {
        match msgs {
            [] => Ok(()),
            [one] => self.send(one).await,
            several => self.send_bundle(several).await,
        }
    }

    async fn send_packet(&self, packet: &OscPacket) -> Result<(), GoDotError> {
        let buf = rosc::encoder::encode(packet).map_err(|e| GoDotError::Encode(format!("{e}")))?;
        self.socket.send_to(&buf, self.dest).await?;
        Ok(())
    }

    /// Wait on `rx` for the answer to `what`.
    async fn answer(
        &self,
        key: &str,
        rx: oneshot::Receiver<GoDotAck>,
        what: &str,
    ) -> Result<GoDotAck, GoDotError> {
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(ack)) => Ok(ack),
            // Elapsed, or a newer request under the same key replaced this one.
            Ok(Err(_)) | Err(_) => {
                self.bus.forget(key).await;
                Err(GoDotError::Timeout(what.to_string()))
            }
        }
    }

    /// Declare the app as a device (contract §3) and wait for
    /// `/godot/declared`. When `query_port` is not 0 the returned
    /// [`Declaration`] also waits for `/godot/described`, which comes once
    /// Go.dot has fetched the description. A refusal is
    /// [`GoDotError::Refused`].
    pub async fn declare(
        &self,
        prefix: &str,
        port: u16,
        query_port: u16,
        name: &str,
    ) -> Result<Declaration, GoDotError> {
        // Both armed before sending: the description can follow fast.
        let described = if query_port != 0 {
            Some(self.bus.register(DESCRIBED_KEY).await)
        } else {
            None
        };
        let rx = self.bus.register(DECLARED_KEY).await;
        self.send(&godot_cue_builder::declare_message(
            prefix, port, query_port, name,
        ))
        .await?;
        let declared = self.answer(DECLARED_KEY, rx, "declare").await?;
        if let Some(reason) = declared.refusal() {
            if described.is_some() {
                self.bus.forget(DESCRIBED_KEY).await;
            }
            return Err(GoDotError::Refused(reason.to_string()));
        }
        info!(dest = %self.dest, prefix, port, query_port, ?declared, "Declared in Go.dot");
        Ok(Declaration {
            declared,
            described,
        })
    }

    /// Capture `cue` (contract §4): every datagram of
    /// [`capture_messages`](godot_cue_builder::capture_messages) in turn,
    /// each waiting for its `/godot/captured` before the next leaves, so the
    /// chunks arrive in order and a refused first chunk sends no more.
    /// Returns every answer, the first one saying `created` or `updated`.
    ///
    /// The cue needs its identifier: the answers are matched by it. Any
    /// refusal ends the capture with [`GoDotError::Refused`] and its reason.
    pub async fn capture(
        &self,
        where_: &str,
        target: &str,
        cue: &GoDotCue,
    ) -> Result<Vec<GoDotAck>, GoDotError> {
        if cue.messages.is_empty() {
            return Err(GoDotError::NothingToCapture);
        }
        let datagrams = godot_cue_builder::capture_messages(where_, target, cue);
        let mut acks = Vec::with_capacity(datagrams.len());
        for (i, msg) in datagrams.iter().enumerate() {
            let rx = self.bus.register(&cue.id).await;
            self.send(msg).await?;
            let what = if datagrams.len() == 1 {
                "capture".to_string()
            } else {
                format!("capture (part {} of {})", i + 1, datagrams.len())
            };
            let ack = self.answer(&cue.id, rx, &what).await?;
            if let Some(reason) = ack.refusal() {
                return Err(GoDotError::Refused(reason.to_string()));
            }
            acks.push(ack);
        }
        debug!(dest = %self.dest, id = %cue.id, datagrams = datagrams.len(), "Captured into Go.dot");
        Ok(acks)
    }

    /// Test-only: route answers through a caller-supplied bus.
    #[cfg(test)]
    fn with_bus(mut self, bus: GoDotReplyBus) -> Self {
        self.bus = bus;
        self
    }

    /// Test-only: shorten the wait for each answer.
    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc::godot_cue_builder::WHERE_STANDBY;
    use crate::osc::godot_reply::parse_answer;
    use rosc::OscType;

    /// A stand-in Go.dot: reads datagrams, and for each capture answers
    /// through the bus as the trigger listener would. `refuse_with` makes
    /// every answer a refusal. Returns the messages it received.
    fn fake_godot(
        socket: UdpSocket,
        bus: GoDotReplyBus,
        refuse_with: Option<&'static str>,
    ) -> tokio::task::JoinHandle<Vec<OscMessage>> {
        tokio::spawn(async move {
            let mut got = Vec::new();
            let mut buf = vec![0u8; 65536];
            let mut first = true;
            loop {
                let Ok(Ok((n, _))) =
                    tokio::time::timeout(Duration::from_millis(400), socket.recv_from(&mut buf))
                        .await
                else {
                    return got;
                };
                let Ok((_, OscPacket::Message(msg))) = rosc::decoder::decode_udp(&buf[..n]) else {
                    continue;
                };
                got.push(msg.clone());
                let s = |i: usize| match &msg.args[i] {
                    OscType::String(s) => s.clone(),
                    other => panic!("argument {i} is not a string: {other:?}"),
                };
                if msg.addr == "/godot/cmd/cue/capture" {
                    // Every chunk names S21_HiJack first, as Go.dot answers by it.
                    assert_eq!(s(0), "/s21", "a capture names its sender");
                    // The cue's id: the head's for the first chunk, the
                    // target for a `more` chunk.
                    let id = if s(1) == "more" { s(2) } else { s(3) };
                    let outcome = match (refuse_with, first) {
                        (Some(reason), _) => reason,
                        (None, true) => "created",
                        (None, false) => "appended",
                    };
                    first = false;
                    let (number, name) = if refuse_with.is_some() {
                        (String::new(), String::new())
                    } else {
                        ("4".to_string(), "Recall 'A'".to_string())
                    };
                    let args = vec![
                        OscType::String(id),
                        OscType::String(outcome.into()),
                        OscType::String(number),
                        OscType::String(name),
                    ];
                    bus.deliver(parse_answer("/godot/captured", &args).unwrap())
                        .await;
                } else if msg.addr == "/godot/cmd/mount/declare" {
                    bus.deliver(GoDotAck::Declared {
                        id: "DEV00001".into(),
                        outcome: refuse_with.unwrap_or("created").into(),
                    })
                    .await;
                    bus.deliver(GoDotAck::Described {
                        id: "DEV00001".into(),
                        node_count: 9,
                        problem: String::new(),
                    })
                    .await;
                }
            }
        })
    }

    async fn client_and_fake(
        refuse_with: Option<&'static str>,
    ) -> (GoDotClient, tokio::task::JoinHandle<Vec<OscMessage>>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let bus = GoDotReplyBus::default();
        let client = GoDotClient::new("127.0.0.1", port)
            .await
            .unwrap()
            .with_bus(bus.clone());
        (client, fake_godot(socket, bus, refuse_with))
    }

    fn cue(pairs: usize) -> GoDotCue {
        GoDotCue {
            id: "0123ABCD".into(),
            name: "Recall 'A'".into(),
            number: String::new(),
            notes: String::new(),
            messages: (0..pairs)
                .map(|n| (format!("/channel/{n}/fader"), vec![OscType::Float(-3.5)]))
                .collect(),
        }
    }

    #[tokio::test]
    async fn new_rejects_an_invalid_address_and_defaults_to_localhost() {
        assert!(GoDotClient::new("not.an.ip", 8010).await.is_err());
        let client = GoDotClient::new("  ", 8010).await.unwrap();
        assert_eq!(client.dest().to_string(), "127.0.0.1:8010");
    }

    #[tokio::test]
    async fn a_capture_is_one_datagram_answered_created() {
        let (client, fake) = client_and_fake(None).await;
        let acks = client.capture(WHERE_STANDBY, "", &cue(2)).await.unwrap();
        assert_eq!(acks.len(), 1);
        assert_eq!(
            acks[0],
            GoDotAck::Captured {
                id: "0123ABCD".into(),
                outcome: "created".into(),
                number: "4".into(),
                name: "Recall 'A'".into(),
            }
        );
        let got = fake.await.unwrap();
        assert_eq!(got.len(), 1);
        let msg = &got[0];
        assert_eq!(msg.addr, "/godot/cmd/cue/capture");
        assert_eq!(
            msg.args.len(),
            8 + 2 * 2,
            "eight head strings and two pairs"
        );
        assert!(msg.args.iter().all(|a| matches!(a, OscType::String(_))));
        assert_eq!(msg.args[0], OscType::String("/s21".into()));
        assert_eq!(msg.args[1], OscType::String("standby".into()));
        assert_eq!(msg.args[3], OscType::String("0123ABCD".into()));
        assert_eq!(msg.args[9], OscType::String("f:-3.5".into()));
    }

    #[tokio::test]
    async fn a_long_capture_waits_for_every_chunk() {
        let (client, fake) = client_and_fake(None).await;
        let acks = client.capture(WHERE_STANDBY, "", &cue(150)).await.unwrap();
        assert!(acks.len() > 1);
        let outcomes: Vec<String> = acks
            .iter()
            .map(|a| match a {
                GoDotAck::Captured { outcome, .. } => outcome.clone(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(outcomes[0], "created");
        assert!(outcomes[1..].iter().all(|o| o == "appended"));
        let got = fake.await.unwrap();
        assert_eq!(got.len(), acks.len());
        let pairs: usize = got.iter().map(|m| (m.args.len() - 8) / 2).sum();
        assert_eq!(pairs, 150);
        assert!(
            got[1..]
                .iter()
                .all(|m| m.args[0] == OscType::String("/s21".into())
                    && m.args[1] == OscType::String("more".into())
                    && m.args[2] == OscType::String("0123ABCD".into()))
        );
    }

    #[tokio::test]
    async fn a_refusal_carries_the_reason_and_sends_no_more() {
        let (client, fake) = client_and_fake(Some("locked")).await;
        let err = client
            .capture(WHERE_STANDBY, "", &cue(150))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, GoDotError::Refused(r) if r == "locked"),
            "{err}"
        );
        assert!(err.to_string().contains("locked"));
        assert_eq!(fake.await.unwrap().len(), 1, "no chunk after a refused one");
    }

    #[tokio::test]
    async fn an_unanswered_capture_times_out() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let client = GoDotClient::new("127.0.0.1", port)
            .await
            .unwrap()
            .with_bus(GoDotReplyBus::default())
            .with_timeout(Duration::from_millis(50));
        let err = client
            .capture(WHERE_STANDBY, "", &cue(1))
            .await
            .unwrap_err();
        assert!(matches!(err, GoDotError::Timeout(_)), "{err}");
        assert!(err.to_string().contains("trigger port"));
        drop(socket);
    }

    #[tokio::test]
    async fn an_empty_capture_is_refused_before_sending() {
        let (client, fake) = client_and_fake(None).await;
        let err = client
            .capture(WHERE_STANDBY, "", &cue(0))
            .await
            .unwrap_err();
        assert!(matches!(err, GoDotError::NothingToCapture));
        assert!(fake.await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn declare_waits_for_declared_then_described() {
        let (client, fake) = client_and_fake(None).await;
        let declaration = client
            .declare("/s21", 53001, 8080, "S21_HiJack")
            .await
            .unwrap();
        assert_eq!(
            declaration.declared,
            GoDotAck::Declared {
                id: "DEV00001".into(),
                outcome: "created".into()
            }
        );
        let described = declaration.described(Duration::from_secs(1)).await.unwrap();
        assert!(matches!(
            described,
            GoDotAck::Described { node_count: 9, .. }
        ));
        let got = fake.await.unwrap();
        assert_eq!(got[0].addr, "/godot/cmd/mount/declare");
    }

    #[tokio::test]
    async fn a_refused_declare_is_an_error() {
        let (client, _fake) = client_and_fake(Some("locked")).await;
        let err = client.declare("/s21", 53001, 0, "S21_HiJack").await;
        assert!(matches!(err, Err(GoDotError::Refused(r)) if r == "locked"));
    }

    #[tokio::test]
    async fn a_bundle_keeps_its_order() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let client = GoDotClient::new("127.0.0.1", port).await.unwrap();
        client
            .send_all(&godot_cue_builder::go_at_number_bundle("12"))
            .await
            .unwrap();
        let mut buf = [0u8; 1024];
        let (n, _) = socket.recv_from(&mut buf).await.unwrap();
        let (_, packet) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
        let OscPacket::Bundle(bundle) = packet else {
            panic!("expected a bundle");
        };
        let addrs: Vec<String> = bundle
            .content
            .iter()
            .map(|p| match p {
                OscPacket::Message(m) => m.addr.clone(),
                OscPacket::Bundle(_) => panic!("nested"),
            })
            .collect();
        assert_eq!(addrs, vec!["/godot/cmd/standby/setNumber", "/godot/cmd/go"]);
    }
}
