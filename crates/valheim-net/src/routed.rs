//! Routed RPCs (`ZRoutedRpc`): calls addressed to a peer UID (0 = everyone)
//! or to an object, relayed by the server. They travel inside the ZRpc
//! method `RoutedRPC(ZPackage)`.

use crate::hash::stable_hash;
use crate::zpackage::{Result, ZPackage, ZReader, ZDOID};

/// Target meaning "every peer".
pub const EVERYBODY: i64 = 0;

#[derive(Debug, Clone, PartialEq)]
pub struct RoutedRpcData {
    pub msg_id: i64,
    pub sender_peer_id: i64,
    pub target_peer_id: i64,
    pub target_zdo: ZDOID,
    pub method_hash: i32,
    pub parameters: ZPackage,
}

impl RoutedRpcData {
    pub fn new(msg_id: i64, sender: i64, target: i64, method: &str, parameters: ZPackage) -> Self {
        Self {
            msg_id,
            sender_peer_id: sender,
            target_peer_id: target,
            target_zdo: ZDOID::NONE,
            method_hash: stable_hash(method),
            parameters,
        }
    }

    pub fn serialize(&self) -> ZPackage {
        let mut p = ZPackage::new();
        p.write_i64(self.msg_id)
            .write_i64(self.sender_peer_id)
            .write_i64(self.target_peer_id)
            .write_zdoid(self.target_zdo)
            .write_i32(self.method_hash)
            .write_package(&self.parameters);
        p
    }

    pub fn deserialize(r: &mut ZReader<'_>) -> Result<Self> {
        Ok(Self {
            msg_id: r.read_i64()?,
            sender_peer_id: r.read_i64()?,
            target_peer_id: r.read_i64()?,
            target_zdo: r.read_zdoid()?,
            method_hash: r.read_i32()?,
            parameters: r.read_package()?,
        })
    }
}
