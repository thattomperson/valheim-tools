//! ZRpc messages: `i32 method hash` followed by positional parameters.
//! Method hash 0 is the keep-alive ping, carrying one bool
//! (`true` = request, `false` = reply).

use crate::hash::stable_hash;
use crate::zpackage::{Result, ZPackage, ZReader};

/// Starts a ZRpc call to `method`; append parameters to the returned package.
pub fn call(method: &str) -> ZPackage {
    let mut p = ZPackage::new();
    p.write_i32(stable_hash(method));
    p
}

/// A ping request (`true`) or reply (`false`).
pub fn ping(request: bool) -> ZPackage {
    let mut p = ZPackage::new();
    p.write_i32(0).write_bool(request);
    p
}

/// Splits a ZRpc message into its method hash and a reader over the
/// parameters.
pub fn parse(body: &[u8]) -> Result<(i32, ZReader<'_>)> {
    let mut r = ZReader::new(body);
    let hash = r.read_i32()?;
    Ok((hash, r))
}
