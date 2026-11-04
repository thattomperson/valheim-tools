//! Messages exchanged while joining (`ZNet`): the platform-ID internal
//! frame, `ServerHandshake`, `ClientHandshake`, `PeerInfo`, plus the
//! `PlayerList` the server sends once we are in.

use crate::zpackage::{Result, Vector3, ZPackage, ZReader, ZDOID};

/// Game version string the server compares loosely (`Version.CurrentVersion`).
pub const GAME_VERSION: &str = "1.0.21";
/// Network protocol version; must match exactly.
pub const NETWORK_VERSION: u32 = 41;

/// `ZNet.ConnectionStatus`, as sent in the `Error(int)` RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    None,
    Connecting,
    Connected,
    ErrorVersion,
    ErrorDisconnected,
    ErrorConnectFailed,
    ErrorPassword,
    ErrorAlreadyConnected,
    ErrorBanned,
    ErrorFull,
    ErrorPlatformExcluded,
    ErrorCrossplayPrivilege,
    ErrorKicked,
    Unknown(i32),
}

impl From<i32> for ConnectionStatus {
    fn from(v: i32) -> Self {
        use ConnectionStatus::*;
        match v {
            0 => None,
            1 => Connecting,
            2 => Connected,
            3 => ErrorVersion,
            4 => ErrorDisconnected,
            5 => ErrorConnectFailed,
            6 => ErrorPassword,
            7 => ErrorAlreadyConnected,
            8 => ErrorBanned,
            9 => ErrorFull,
            10 => ErrorPlatformExcluded,
            11 => ErrorCrossplayPrivilege,
            12 => ErrorKicked,
            n => Unknown(n),
        }
    }
}

impl std::fmt::Display for ConnectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ConnectionStatus::*;
        let s = match self {
            ErrorVersion => "incompatible game version",
            ErrorDisconnected => "disconnected",
            ErrorConnectFailed => "connection failed (or not authenticated)",
            ErrorPassword => "wrong password",
            ErrorAlreadyConnected => "already connected",
            ErrorBanned => "banned or not on the permitted list",
            ErrorFull => "server is full",
            ErrorPlatformExcluded => "platform excluded",
            ErrorCrossplayPrivilege => "crossplay not permitted",
            ErrorKicked => "kicked",
            Unknown(n) => return write!(f, "unknown error {n}"),
            other => return write!(f, "{other:?}"),
        };
        f.write_str(s)
    }
}

/// The socket-internal frame (type 64) announcing our platform user ID.
pub fn platform_id_frame(platform_user_id: &str) -> ZPackage {
    let mut p = ZPackage::new();
    p.write_u8(1).write_str(platform_user_id);
    p
}

/// `SimulationDistance`; the game's "original" setting is 2/2/classic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimulationDistance {
    pub near: i32,
    pub far: i32,
    pub classic: bool,
}

impl Default for SimulationDistance {
    fn default() -> Self {
        Self {
            near: 2,
            far: 2,
            classic: true,
        }
    }
}

/// The client's `PeerInfo` package.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientPeerInfo {
    pub uid: i64,
    pub ref_pos: Vector3,
    pub player_name: String,
    pub playfab_id: String,
    pub simulation_distance: SimulationDistance,
    /// Output of [`crate::password::hash_password`], or empty.
    pub password_hash: String,
    pub invite_secret_key: String,
}

impl ClientPeerInfo {
    pub fn serialize(&self) -> ZPackage {
        let mut p = ZPackage::new();
        p.write_i64(self.uid)
            .write_str(GAME_VERSION)
            .write_u32(NETWORK_VERSION)
            .write_vec3(self.ref_pos)
            .write_str(&self.player_name)
            .write_str(&self.playfab_id)
            .write_i32(self.simulation_distance.near)
            .write_i32(self.simulation_distance.far)
            .write_bool(self.simulation_distance.classic)
            .write_str(&self.password_hash)
            .write_str(&self.invite_secret_key)
            // Steam session ticket: only read by Steam-backend servers.
            .write_bytes(&[]);
        p
    }
}

/// The world details in the server's `PeerInfo` reply.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerPeerInfo {
    pub uid: i64,
    pub version: String,
    pub network_version: u32,
    pub ref_pos: Vector3,
    pub server_name: String,
    pub playfab_id: String,
    pub world_name: String,
    pub seed: i32,
    pub seed_name: String,
    pub world_uid: i64,
    pub world_gen_version: i32,
    pub net_time: f64,
}

impl ServerPeerInfo {
    /// Parses the reply. Returns the network version separately on mismatch,
    /// because the rest of the layout may differ.
    pub fn deserialize(r: &mut ZReader<'_>) -> Result<std::result::Result<Self, (String, u32)>> {
        let uid = r.read_i64()?;
        let version = r.read_string()?;
        let network_version = r.read_u32()?;
        if network_version != NETWORK_VERSION {
            return Ok(Err((version, network_version)));
        }
        let ref_pos = r.read_vec3()?;
        let server_name = r.read_string()?;
        let playfab_id = r.read_string()?;
        let _near = r.read_i32()?;
        let _far = r.read_i32()?;
        let _classic = r.read_bool()?;
        Ok(Ok(Self {
            uid,
            version,
            network_version,
            ref_pos,
            server_name,
            playfab_id,
            world_name: r.read_string()?,
            seed: r.read_i32()?,
            seed_name: r.read_string()?,
            world_uid: r.read_i64()?,
            world_gen_version: r.read_i32()?,
            net_time: r.read_f64()?,
        }))
    }
}

/// One entry of the server's `PlayerList`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInfo {
    pub name: String,
    pub character_id: ZDOID,
    pub platform_user_id: String,
    pub display_name: String,
    pub server_assigned_display_name: String,
    pub playfab_id: String,
    pub position: Option<Vector3>,
}

pub fn parse_player_list(r: &mut ZReader<'_>) -> Result<Vec<PlayerInfo>> {
    let n = r.read_i32()?.max(0);
    (0..n)
        .map(|_| {
            Ok(PlayerInfo {
                name: r.read_string()?,
                character_id: r.read_zdoid()?,
                platform_user_id: r.read_string()?,
                display_name: r.read_string()?,
                server_assigned_display_name: r.read_string()?,
                playfab_id: r.read_string()?,
                position: if r.read_bool()? {
                    Some(r.read_vec3()?)
                } else {
                    None
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_peer_info_layout() {
        let info = ClientPeerInfo {
            uid: 1,
            ref_pos: Vector3::ZERO,
            player_name: "Bot".into(),
            playfab_id: "ABC".into(),
            simulation_distance: SimulationDistance::default(),
            password_hash: String::new(),
            invite_secret_key: String::new(),
        };
        let p = info.serialize();
        let mut r = p.reader();
        assert_eq!(r.read_i64().unwrap(), 1);
        assert_eq!(r.read_string().unwrap(), GAME_VERSION);
        assert_eq!(r.read_u32().unwrap(), 41);
        assert_eq!(r.read_vec3().unwrap(), Vector3::ZERO);
        assert_eq!(r.read_string().unwrap(), "Bot");
        assert_eq!(r.read_string().unwrap(), "ABC");
        assert_eq!(
            (
                r.read_i32().unwrap(),
                r.read_i32().unwrap(),
                r.read_bool().unwrap()
            ),
            (2, 2, true)
        );
        assert_eq!(r.read_string().unwrap(), "");
        assert_eq!(r.read_string().unwrap(), "");
        assert_eq!(r.read_bytes().unwrap(), &[] as &[u8]);
        assert_eq!(r.remaining(), 0);
    }
}
