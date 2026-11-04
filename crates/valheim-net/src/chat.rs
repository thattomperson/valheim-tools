//! The `ChatMessage` routed RPC (`Chat.RPC_ChatMessage`).

use crate::zpackage::{Result, Vector3, ZPackage, ZReader};

pub const METHOD: &str = "ChatMessage";

/// `Talker.Type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TalkerType {
    Whisper,
    Normal,
    Shout,
    Ping,
    Other(i32),
}

impl From<i32> for TalkerType {
    fn from(v: i32) -> Self {
        match v {
            0 => Self::Whisper,
            1 => Self::Normal,
            2 => Self::Shout,
            3 => Self::Ping,
            n => Self::Other(n),
        }
    }
}

impl From<TalkerType> for i32 {
    fn from(t: TalkerType) -> i32 {
        match t {
            TalkerType::Whisper => 0,
            TalkerType::Normal => 1,
            TalkerType::Shout => 2,
            TalkerType::Ping => 3,
            TalkerType::Other(n) => n,
        }
    }
}

/// Parameters of `ChatMessage(Vector3 position, int type, UserInfo user, string text)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub position: Vector3,
    pub kind: TalkerType,
    /// `UserInfo.Name`. Receivers display the name from the player list instead.
    pub name: String,
    /// `UserInfo.UserId`, e.g. `Steam_7656...`. Receivers drop the message
    /// unless this ID is in the server's player list.
    pub platform_user_id: String,
    pub text: String,
}

impl ChatMessage {
    pub fn serialize(&self) -> ZPackage {
        let mut p = ZPackage::new();
        p.write_vec3(self.position)
            .write_i32(self.kind.into())
            .write_str(&self.name)
            .write_str(&self.platform_user_id)
            .write_str(&self.text);
        p
    }

    pub fn deserialize(r: &mut ZReader<'_>) -> Result<Self> {
        Ok(Self {
            position: r.read_vec3()?,
            kind: r.read_i32()?.into(),
            name: r.read_string()?,
            platform_user_id: r.read_string()?,
            text: r.read_string()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let m = ChatMessage {
            position: Vector3::new(1.0, 2.0, 3.0),
            kind: TalkerType::Shout,
            name: "Bot".into(),
            platform_user_id: "ValheimTools_abc".into(),
            text: "hello".into(),
        };
        let p = m.serialize();
        assert_eq!(ChatMessage::deserialize(&mut p.reader()).unwrap(), m);
    }
}
