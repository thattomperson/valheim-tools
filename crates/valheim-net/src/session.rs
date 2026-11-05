//! Sans-IO client session: joins a server over an established Party link,
//! answers pings, and sends chat.
//!
//! Feed every Party message from the server endpoint into
//! [`ClientSession::receive`], call [`ClientSession::poll`] regularly, and
//! send whatever [`ClientSession::take_outgoing`] returns to the server
//! endpoint (guaranteed + sequential).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::chat::{self, ChatMessage, TalkerType};
use crate::frame::{FrameError, ReliableChannel, TYPE_DATA, TYPE_INTERNAL};
use crate::handshake::{
    parse_player_list, platform_id_frame, ClientPeerInfo, ConnectionStatus, PlayerInfo,
    ServerPeerInfo, SimulationDistance,
};
use crate::hash::stable_hash;
use crate::password::hash_password;
use crate::routed::{RoutedRpcData, EVERYBODY};
use crate::rpc;
use crate::zpackage::{ReadError, Vector3, ZPackage};

const PING_INTERVAL: Duration = Duration::from_secs(1);

/// Who we are to the server.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Sent in the internal frame and in chat messages, e.g. `ValheimTools_ab12`.
    pub platform_user_id: String,
    /// The name other players see.
    pub player_name: String,
    /// `PlayFabId` from `LoginWithCustomID`.
    pub playfab_id: String,
    /// Server password, if any.
    pub password: Option<String>,
    /// Our routed-RPC peer ID. Random per session.
    pub uid: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The server accepted our `PeerInfo`. Chat can be sent from now on.
    Connected(Box<ServerPeerInfo>),
    /// The server's player list changed.
    PlayerList(Vec<PlayerInfo>),
    /// A chat line routed to us.
    Chat { sender: i64, message: ChatMessage },
    /// The server refused us (`Error(int)` RPC).
    Rejected(ConnectionStatus),
    /// The server wants a password and none was configured.
    PasswordRequired,
    /// The server speaks a different network version.
    VersionMismatch {
        version: String,
        network_version: u32,
    },
    /// The server told us to go away (`Disconnect` RPC).
    Disconnected,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("malformed message: {0}")]
    Read(#[from] ReadError),
    #[error("not connected yet")]
    NotConnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    /// Sent `ServerHandshake`, waiting for `ClientHandshake`.
    Handshaking,
    /// Sent `PeerInfo`, waiting for the server's.
    Authenticating,
    Connected,
    Closed,
}

#[derive(Debug)]
pub struct ClientSession {
    cfg: ClientConfig,
    channel: ReliableChannel,
    state: State,
    outgoing: Vec<Vec<u8>>,
    events: VecDeque<Event>,
    next_ping: Option<Instant>,
    routed_counter: i64,
}

mod method {
    use std::sync::LazyLock;

    use crate::hash::stable_hash;

    pub static CLIENT_HANDSHAKE: LazyLock<i32> = LazyLock::new(|| stable_hash("ClientHandshake"));
    pub static PEER_INFO: LazyLock<i32> = LazyLock::new(|| stable_hash("PeerInfo"));
    pub static ERROR: LazyLock<i32> = LazyLock::new(|| stable_hash("Error"));
    pub static DISCONNECT: LazyLock<i32> = LazyLock::new(|| stable_hash("Disconnect"));
    pub static PLAYER_LIST: LazyLock<i32> = LazyLock::new(|| stable_hash("PlayerList"));
    pub static ROUTED_RPC: LazyLock<i32> = LazyLock::new(|| stable_hash("RoutedRPC"));
}

impl ClientSession {
    pub fn new(cfg: ClientConfig) -> Self {
        Self {
            cfg,
            channel: ReliableChannel::new(),
            state: State::Idle,
            outgoing: Vec::new(),
            events: VecDeque::new(),
            next_ping: None,
            routed_counter: 0,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn config(&self) -> &ClientConfig {
        &self.cfg
    }

    /// Opens the session: the platform-ID internal frame, then
    /// `ServerHandshake("")`. Call once the server endpoint is known.
    pub fn start(&mut self, now: Instant) {
        if self.state != State::Idle {
            return;
        }
        let id = platform_id_frame(&self.cfg.platform_user_id);
        self.push(TYPE_INTERNAL, id.as_bytes(), now);
        let mut hs = rpc::call("ServerHandshake");
        hs.write_str("");
        self.push(TYPE_DATA, hs.as_bytes(), now);
        self.state = State::Handshaking;
        self.next_ping = Some(now + PING_INTERVAL);
    }

    fn push(&mut self, kind: u8, body: &[u8], now: Instant) {
        let wire = self.channel.send(kind, body, now);
        self.outgoing.push(wire);
    }

    fn call(&mut self, pkg: ZPackage, now: Instant) {
        self.push(TYPE_DATA, pkg.as_bytes(), now);
    }

    /// Party messages to send to the server endpoint, in order.
    pub fn take_outgoing(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.outgoing)
    }

    pub fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// Retransmits and keep-alive pings.
    pub fn poll(&mut self, now: Instant) {
        let resend = self.channel.poll(now);
        self.outgoing.extend(resend);
        if let Some(at) = self.next_ping {
            if now >= at && matches!(self.state, State::Authenticating | State::Connected) {
                self.call(rpc::ping(true), now);
                self.next_ping = Some(now + PING_INTERVAL);
            }
        }
    }

    /// Feeds one Party message received from the server endpoint.
    pub fn receive(&mut self, msg: &[u8], now: Instant) -> Result<(), SessionError> {
        let received = self.channel.receive(msg, now)?;
        let compression_before = self.channel.compression();
        for d in &received.delivered {
            if d.kind == TYPE_DATA {
                self.handle_rpc(&d.body, now)?;
            }
        }
        if compression_before != self.channel.compression() {
            // The acks were encoded before the switch and the server, which
            // switched first, would drop them. Send a compressed one instead.
            // Our data sent in this call was already encoded compressed
            // because the switch happens before we reply.
            let ack = self.channel.ack();
            self.outgoing.insert(0, ack);
        } else {
            // Acks go out before any replies they trigger, as the game does.
            let mut acks = received.outgoing;
            acks.append(&mut self.outgoing);
            self.outgoing = acks;
        }
        Ok(())
    }

    fn handle_rpc(&mut self, body: &[u8], now: Instant) -> Result<(), SessionError> {
        let (hash, mut r) = rpc::parse(body)?;
        if hash == 0 {
            if r.read_bool()? {
                self.call(rpc::ping(false), now);
            }
        } else if hash == *method::CLIENT_HANDSHAKE {
            let need_password = r.read_bool()?;
            let salt = r.read_string()?;
            self.on_client_handshake(need_password, &salt, now);
        } else if hash == *method::PEER_INFO {
            let pkg = r.read_package()?;
            match ServerPeerInfo::deserialize(&mut pkg.reader())? {
                Ok(info) => {
                    self.channel.enable_compression();
                    self.state = State::Connected;
                    self.events.push_back(Event::Connected(Box::new(info)));
                }
                Err((version, network_version)) => {
                    self.state = State::Closed;
                    self.events.push_back(Event::VersionMismatch {
                        version,
                        network_version,
                    });
                }
            }
        } else if hash == *method::ERROR {
            let status = ConnectionStatus::from(r.read_i32()?);
            self.state = State::Closed;
            self.events.push_back(Event::Rejected(status));
        } else if hash == *method::DISCONNECT {
            self.state = State::Closed;
            self.events.push_back(Event::Disconnected);
        } else if hash == *method::PLAYER_LIST {
            let pkg = r.read_package()?;
            let list = parse_player_list(&mut pkg.reader())?;
            self.events.push_back(Event::PlayerList(list));
        } else if hash == *method::ROUTED_RPC {
            let pkg = r.read_package()?;
            let data = RoutedRpcData::deserialize(&mut pkg.reader())?;
            if data.method_hash == stable_hash(chat::METHOD) {
                let message = ChatMessage::deserialize(&mut data.parameters.reader())?;
                self.events.push_back(Event::Chat {
                    sender: data.sender_peer_id,
                    message,
                });
            }
        } else {
            tracing::trace!(hash, "ignoring rpc");
        }
        Ok(())
    }

    fn on_client_handshake(&mut self, need_password: bool, salt: &str, now: Instant) {
        let password_hash = match (need_password, &self.cfg.password) {
            (false, _) => String::new(),
            (true, Some(pw)) => hash_password(pw, salt),
            (true, None) => {
                self.state = State::Closed;
                self.events.push_back(Event::PasswordRequired);
                return;
            }
        };
        let info = ClientPeerInfo {
            uid: self.cfg.uid,
            ref_pos: Vector3::ZERO,
            player_name: self.cfg.player_name.clone(),
            playfab_id: self.cfg.playfab_id.clone(),
            simulation_distance: SimulationDistance::default(),
            password_hash,
            invite_secret_key: String::new(),
        };
        let mut call = rpc::call("PeerInfo");
        call.write_package(&info.serialize());
        self.call(call, now);
        self.state = State::Authenticating;
    }

    /// Sends a chat line to everybody, as a routed `ChatMessage`.
    pub fn send_chat(
        &mut self,
        kind: TalkerType,
        text: &str,
        now: Instant,
    ) -> Result<(), SessionError> {
        if self.state != State::Connected {
            return Err(SessionError::NotConnected);
        }
        self.routed_counter += 1;
        let message = ChatMessage {
            position: Vector3::ZERO,
            kind,
            name: self.cfg.player_name.clone(),
            platform_user_id: self.cfg.platform_user_id.clone(),
            text: text.to_owned(),
        };
        let data = RoutedRpcData::new(
            self.cfg.uid.wrapping_add(self.routed_counter),
            self.cfg.uid,
            EVERYBODY,
            chat::METHOD,
            message.serialize(),
        );
        let mut call = rpc::call("RoutedRPC");
        call.write_package(&data.serialize());
        self.call(call, now);
        Ok(())
    }

    /// Sends a shout, the only chat type a character-less client can send
    /// to everyone.
    pub fn shout(&mut self, text: &str, now: Instant) -> Result<(), SessionError> {
        self.send_chat(TalkerType::Shout, text, now)
    }

    /// Asks the server to drop us, so the slot frees up at once.
    pub fn disconnect(&mut self, now: Instant) {
        if matches!(self.state, State::Authenticating | State::Connected) {
            self.call(rpc::call("Disconnect"), now);
        }
        self.state = State::Closed;
    }

    /// True once everything we sent has been acknowledged.
    pub fn all_acked(&self) -> bool {
        self.channel.in_flight_bytes() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::TYPE_ACK;
    use crate::zpackage::ZReader;

    /// A scripted server built on the same frame layer.
    struct FakeServer {
        ch: ReliableChannel,
        now: Instant,
        got: Vec<(u8, Vec<u8>)>,
    }

    impl FakeServer {
        fn new(now: Instant) -> Self {
            Self {
                ch: ReliableChannel::new(),
                now,
                got: Vec::new(),
            }
        }

        fn feed(&mut self, msgs: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
            let mut acks = Vec::new();
            for m in msgs {
                let r = self.ch.receive(&m, self.now).unwrap();
                self.got
                    .extend(r.delivered.into_iter().map(|d| (d.kind, d.body)));
                acks.extend(r.outgoing);
            }
            acks
        }

        fn rpc(&mut self, pkg: ZPackage) -> Vec<u8> {
            self.ch.send(TYPE_DATA, pkg.as_bytes(), self.now)
        }

        fn take_rpcs(&mut self) -> Vec<(i32, Vec<u8>)> {
            std::mem::take(&mut self.got)
                .into_iter()
                .filter(|(k, _)| *k == TYPE_DATA)
                .map(|(_, b)| {
                    let (h, mut r) = rpc::parse(&b).unwrap();
                    (h, r.rest().to_vec())
                })
                .collect()
        }
    }

    fn cfg(password: Option<&str>) -> ClientConfig {
        ClientConfig {
            platform_user_id: "ValheimTools_test".into(),
            player_name: "Bot".into(),
            playfab_id: "PFID".into(),
            password: password.map(Into::into),
            uid: 1000,
        }
    }

    fn server_peer_info() -> ZPackage {
        let mut info = ZPackage::new();
        info.write_i64(42)
            .write_str("1.0.21")
            .write_u32(41)
            .write_vec3(Vector3::ZERO)
            .write_str("My server")
            .write_str("SERVERPF")
            .write_i32(2)
            .write_i32(2)
            .write_bool(true)
            .write_str("World")
            .write_i32(7)
            .write_str("seed")
            .write_i64(99)
            .write_i32(2)
            .write_f64(123.0);
        let mut call = rpc::call("PeerInfo");
        call.write_package(&info);
        call
    }

    #[test]
    fn full_join_and_shout() {
        let now = Instant::now();
        let mut c = ClientSession::new(cfg(Some("secret")));
        let mut s = FakeServer::new(now);

        c.start(now);
        let acks = s.feed(c.take_outgoing());
        let first = s.got.remove(0);
        assert_eq!(first.0, TYPE_INTERNAL);
        let mut r = ZReader::new(&first.1);
        assert_eq!(r.read_u8().unwrap(), 1);
        assert_eq!(r.read_string().unwrap(), "ValheimTools_test");
        let rpcs = s.take_rpcs();
        assert_eq!(rpcs.len(), 1);
        assert_eq!(rpcs[0].0, stable_hash("ServerHandshake"));
        for a in acks {
            c.receive(&a, now).unwrap();
        }
        assert!(c.all_acked());

        // Server: ClientHandshake(needPassword=true, salt).
        let mut hs = rpc::call("ClientHandshake");
        hs.write_bool(true).write_str("salty");
        let m = s.rpc(hs);
        c.receive(&m, now).unwrap();
        assert_eq!(c.state(), State::Authenticating);
        let out = c.take_outgoing();
        assert_eq!(out[0], vec![1, 0, 0, 0, TYPE_ACK], "ack comes first");
        s.feed(out);
        let rpcs = s.take_rpcs();
        assert_eq!(rpcs[0].0, stable_hash("PeerInfo"));
        let mut r = ZReader::new(&rpcs[0].1);
        let pkg = r.read_package().unwrap();
        let mut r = pkg.reader();
        assert_eq!(r.read_i64().unwrap(), 1000);
        r.read_string().unwrap();
        r.read_u32().unwrap();
        r.read_vec3().unwrap();
        assert_eq!(r.read_string().unwrap(), "Bot");
        assert_eq!(r.read_string().unwrap(), "PFID");
        r.read_i32().unwrap();
        r.read_i32().unwrap();
        r.read_bool().unwrap();
        assert_eq!(r.read_string().unwrap(), hash_password("secret", "salty"));

        // Server: PeerInfo, then switches to compression.
        let m = s.rpc(server_peer_info());
        s.ch.enable_compression();
        c.receive(&m, now).unwrap();
        assert_eq!(c.state(), State::Connected);
        assert!(matches!(c.next_event(), Some(Event::Connected(i)) if i.world_name == "World"));
        let out = c.take_outgoing();
        assert_eq!(&out[0][..1], &[0x78], "post-switch ack is compressed");
        s.feed(out);

        c.shout("hello", now).unwrap();
        s.feed(c.take_outgoing());
        let rpcs = s.take_rpcs();
        assert_eq!(rpcs.len(), 1);
        assert_eq!(rpcs[0].0, stable_hash("RoutedRPC"));
        let mut r = ZReader::new(&rpcs[0].1);
        let pkg = r.read_package().unwrap();
        let data = RoutedRpcData::deserialize(&mut pkg.reader()).unwrap();
        assert_eq!(data.sender_peer_id, 1000);
        assert_eq!(data.target_peer_id, EVERYBODY);
        assert_eq!(data.msg_id, 1001);
        assert_eq!(data.method_hash, stable_hash("ChatMessage"));
        let msg = ChatMessage::deserialize(&mut data.parameters.reader()).unwrap();
        assert_eq!(msg.kind, TalkerType::Shout);
        assert_eq!(msg.platform_user_id, "ValheimTools_test");
        assert_eq!(msg.text, "hello");
    }

    #[test]
    fn password_required_without_one() {
        let now = Instant::now();
        let mut c = ClientSession::new(cfg(None));
        let mut s = FakeServer::new(now);
        c.start(now);
        s.feed(c.take_outgoing());
        let mut hs = rpc::call("ClientHandshake");
        hs.write_bool(true).write_str("salt");
        let m = s.rpc(hs);
        c.receive(&m, now).unwrap();
        assert_eq!(c.next_event(), Some(Event::PasswordRequired));
        assert_eq!(c.state(), State::Closed);
    }

    #[test]
    fn answers_pings_and_reports_errors() {
        let now = Instant::now();
        let mut c = ClientSession::new(cfg(None));
        let mut s = FakeServer::new(now);
        c.start(now);
        s.feed(c.take_outgoing());
        s.take_rpcs();

        let m = s.rpc(rpc::ping(true));
        c.receive(&m, now).unwrap();
        s.feed(c.take_outgoing());
        let rpcs = s.take_rpcs();
        assert_eq!(rpcs, vec![(0, vec![0])], "pong");

        let mut err = rpc::call("Error");
        err.write_i32(6);
        let m = s.rpc(err);
        c.receive(&m, now).unwrap();
        assert_eq!(
            c.next_event(),
            Some(Event::Rejected(ConnectionStatus::ErrorPassword))
        );
    }

    #[test]
    fn shout_before_connect_fails() {
        let mut c = ClientSession::new(cfg(None));
        assert!(matches!(
            c.shout("x", Instant::now()),
            Err(SessionError::NotConnected)
        ));
    }
}
