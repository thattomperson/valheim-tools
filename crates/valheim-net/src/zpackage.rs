//! `ZPackage`: Valheim's serialization buffer.
//!
//! It is a thin wrapper around .NET's `BinaryWriter`/`BinaryReader`, so all
//! integers and floats are little-endian, strings are a 7-bit varint byte
//! length followed by UTF-8, and nested byte arrays / packages are an `i32`
//! length followed by the bytes.

use std::io::{Read, Write};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReadError {
    #[error("unexpected end of package (wanted {wanted} bytes at offset {pos}, have {len})")]
    Eof { pos: usize, wanted: usize, len: usize },
    #[error("invalid 7-bit encoded length")]
    BadVarint,
    #[error("negative length {0}")]
    NegativeLength(i32),
    #[error("string is not valid UTF-8")]
    BadUtf8,
    #[error("failed to decompress package")]
    Decompress,
}

pub type Result<T> = std::result::Result<T, ReadError>;

/// Unity `Vector3`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vector3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vector3 {
    pub const ZERO: Vector3 = Vector3 { x: 0.0, y: 0.0, z: 0.0 };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}

/// Valheim object ID: the owning peer's UID plus a per-peer counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ZDOID {
    pub user_id: i64,
    pub id: u32,
}

impl ZDOID {
    pub const NONE: ZDOID = ZDOID { user_id: 0, id: 0 };

    pub fn is_none(&self) -> bool {
        self.user_id == 0 && self.id == 0
    }
}

/// A write-only package builder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZPackage {
    buf: Vec<u8>,
}

impl ZPackage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_bytes(buf: Vec<u8>) -> Self {
        Self { buf }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn reader(&self) -> ZReader<'_> {
        ZReader::new(&self.buf)
    }

    pub fn write_u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn write_bool(&mut self, v: bool) -> &mut Self {
        self.write_u8(v as u8)
    }

    pub fn write_i32(&mut self, v: i32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn write_u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn write_i64(&mut self, v: i64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn write_u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn write_f32(&mut self, v: f32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn write_f64(&mut self, v: f64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// .NET `BinaryWriter.Write(string)`: 7-bit varint byte length + UTF-8.
    pub fn write_str(&mut self, s: &str) -> &mut Self {
        self.write_7bit_len(s.len());
        self.buf.extend_from_slice(s.as_bytes());
        self
    }

    /// Same as [`write_str`](Self::write_str) but for raw bytes. Used where
    /// .NET strings may hold non-UTF-8-safe content (we never need this for
    /// normal text, but the password hash is ASCII-only anyway).
    pub fn write_str_bytes(&mut self, s: &[u8]) -> &mut Self {
        self.write_7bit_len(s.len());
        self.buf.extend_from_slice(s);
        self
    }

    fn write_7bit_len(&mut self, mut n: usize) {
        while n >= 0x80 {
            self.buf.push((n as u8) | 0x80);
            n >>= 7;
        }
        self.buf.push(n as u8);
    }

    /// `ZPackage.Write(byte[])`: `i32` length + bytes.
    pub fn write_bytes(&mut self, b: &[u8]) -> &mut Self {
        self.write_i32(b.len() as i32);
        self.buf.extend_from_slice(b);
        self
    }

    /// `ZPackage.Write(ZPackage)`: nested package, `i32` length + bytes.
    pub fn write_package(&mut self, p: &ZPackage) -> &mut Self {
        self.write_bytes(&p.buf)
    }

    /// `ZPackage.WriteCompressed(ZPackage)`: gzip, then `i32` length + bytes.
    pub fn write_compressed(&mut self, p: &ZPackage) -> &mut Self {
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(&p.buf).expect("writing to a Vec cannot fail");
        let gz = enc.finish().expect("writing to a Vec cannot fail");
        self.write_bytes(&gz)
    }

    pub fn write_vec3(&mut self, v: Vector3) -> &mut Self {
        self.write_f32(v.x).write_f32(v.y).write_f32(v.z)
    }

    pub fn write_zdoid(&mut self, id: ZDOID) -> &mut Self {
        self.write_i64(id.user_id).write_u32(id.id)
    }

    pub fn write_string_list(&mut self, list: &[String]) -> &mut Self {
        self.write_i32(list.len() as i32);
        for s in list {
            self.write_str(s);
        }
        self
    }
}

/// A cursor over package bytes.
#[derive(Debug, Clone)]
pub struct ZReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> ZReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(ReadError::Eof { pos: self.pos, wanted: n, len: self.buf.len() });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("length checked"))
    }

    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn read_bool(&mut self) -> Result<bool> {
        Ok(self.read_u8()? != 0)
    }

    pub fn read_i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn read_i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    pub fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn read_f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    pub fn read_f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.array()?))
    }

    fn read_7bit_len(&mut self) -> Result<usize> {
        let mut result: u32 = 0;
        for shift in (0..35).step_by(7) {
            let b = self.read_u8()?;
            result |= ((b & 0x7f) as u32) << shift;
            if b & 0x80 == 0 {
                return Ok(result as usize);
            }
        }
        Err(ReadError::BadVarint)
    }

    /// Raw bytes of a .NET string.
    pub fn read_str_bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.read_7bit_len()?;
        self.take(n)
    }

    /// A .NET string, decoded as UTF-8. Invalid sequences are replaced, as
    /// .NET's decoder does.
    pub fn read_string(&mut self) -> Result<String> {
        Ok(String::from_utf8_lossy(self.read_str_bytes()?).into_owned())
    }

    fn read_len(&mut self) -> Result<usize> {
        let n = self.read_i32()?;
        if n < 0 {
            return Err(ReadError::NegativeLength(n));
        }
        Ok(n as usize)
    }

    pub fn read_bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.read_len()?;
        self.take(n)
    }

    pub fn read_package(&mut self) -> Result<ZPackage> {
        Ok(ZPackage::from_bytes(self.read_bytes()?.to_vec()))
    }

    pub fn read_compressed_package(&mut self) -> Result<ZPackage> {
        let gz = self.read_bytes()?;
        let mut out = Vec::new();
        GzDecoder::new(gz).read_to_end(&mut out).map_err(|_| ReadError::Decompress)?;
        Ok(ZPackage::from_bytes(out))
    }

    pub fn read_vec3(&mut self) -> Result<Vector3> {
        Ok(Vector3 { x: self.read_f32()?, y: self.read_f32()?, z: self.read_f32()? })
    }

    pub fn read_zdoid(&mut self) -> Result<ZDOID> {
        Ok(ZDOID { user_id: self.read_i64()?, id: self.read_u32()? })
    }

    pub fn read_string_list(&mut self) -> Result<Vec<String>> {
        let n = self.read_len()?;
        (0..n).map(|_| self.read_string()).collect()
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.buf[self.pos..];
        self.pos = self.buf.len();
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_round_trip() {
        let mut p = ZPackage::new();
        p.write_u8(7)
            .write_bool(true)
            .write_i32(-5)
            .write_u32(41)
            .write_i64(i64::MIN)
            .write_f32(1.5)
            .write_f64(-2.25)
            .write_str("héllo")
            .write_bytes(&[1, 2, 3])
            .write_vec3(Vector3::new(1.0, 2.0, 3.0))
            .write_zdoid(ZDOID { user_id: 9, id: 10 });
        let mut r = p.reader();
        assert_eq!(r.read_u8().unwrap(), 7);
        assert!(r.read_bool().unwrap());
        assert_eq!(r.read_i32().unwrap(), -5);
        assert_eq!(r.read_u32().unwrap(), 41);
        assert_eq!(r.read_i64().unwrap(), i64::MIN);
        assert_eq!(r.read_f32().unwrap(), 1.5);
        assert_eq!(r.read_f64().unwrap(), -2.25);
        assert_eq!(r.read_string().unwrap(), "héllo");
        assert_eq!(r.read_bytes().unwrap(), &[1, 2, 3]);
        assert_eq!(r.read_vec3().unwrap(), Vector3::new(1.0, 2.0, 3.0));
        assert_eq!(r.read_zdoid().unwrap(), ZDOID { user_id: 9, id: 10 });
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn dotnet_string_layout() {
        // BinaryWriter.Write("1.0.21") == 06 31 2E 30 2E 32 31
        let mut p = ZPackage::new();
        p.write_str("1.0.21");
        assert_eq!(p.as_bytes(), b"\x061.0.21");

        // 200-byte string uses a two-byte length prefix: C8 01
        let long = "a".repeat(200);
        let mut p = ZPackage::new();
        p.write_str(&long);
        assert_eq!(&p.as_bytes()[..2], &[0xC8, 0x01]);
        assert_eq!(p.reader().read_string().unwrap(), long);
    }

    #[test]
    fn compressed_package_round_trip() {
        let mut inner = ZPackage::new();
        inner.write_str("compressed!");
        let mut outer = ZPackage::new();
        outer.write_compressed(&inner);
        let got = outer.reader().read_compressed_package().unwrap();
        assert_eq!(got, inner);
    }

    #[test]
    fn eof_is_an_error() {
        let p = ZPackage::from_bytes(vec![1, 2]);
        assert!(matches!(p.reader().read_i32(), Err(ReadError::Eof { .. })));
    }
}
