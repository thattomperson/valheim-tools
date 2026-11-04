//! Valheim's reliable, ordered framing on top of PlayFab Party
//! (`ZPlayFabSocket`).
//!
//! Each Party message carries one frame: `body ++ seq:u32le ++ type:u8`.
//! Data and internal frames are numbered per direction starting at 0 and are
//! acknowledged with a 5-byte ack frame (empty body) whose `seq` is the next
//! number the receiver expects. After the version handshake both sides
//! zlib-compress every Party message, acks included.

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;

/// Frame type: a ZRpc message.
pub const TYPE_DATA: u8 = 17;
/// Frame type: acknowledgement.
pub const TYPE_ACK: u8 = 42;
/// Frame type: socket-internal message (platform ID exchange).
pub const TYPE_INTERNAL: u8 = 64;

const TRAILER_LEN: usize = 5;
const RETRANSMIT_FIRST: Duration = Duration::from_secs(3);
const RETRANSMIT_AGAIN: Duration = Duration::from_secs(1);

/// A frame delivered in order to the layer above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub kind: u8,
    pub body: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("peer acknowledged a frame we never sent")]
    BogusAck,
}

/// What came out of feeding one Party message into the channel.
#[derive(Debug, Default)]
pub struct Received {
    /// Frames now deliverable in order.
    pub delivered: Vec<Delivered>,
    /// Party messages to send back (acks).
    pub outgoing: Vec<Vec<u8>>,
}

fn trailer_ok(buf: &[u8]) -> bool {
    buf.len() >= TRAILER_LEN && matches!(buf[buf.len() - 1], TYPE_DATA | TYPE_ACK | TYPE_INTERNAL)
}

fn split(buf: &[u8]) -> (&[u8], u32, u8) {
    let n = buf.len() - TRAILER_LEN;
    let seq = u32::from_le_bytes(buf[n..n + 4].try_into().expect("4 bytes"));
    (&buf[..n], seq, buf[buf.len() - 1])
}

fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).expect("writing to a Vec cannot fail");
    enc.finish().expect("writing to a Vec cannot fail")
}

fn zlib_decompress(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    ZlibDecoder::new(data).read_to_end(&mut out).ok()?;
    Some(out)
}

/// One direction-pair of the reliable channel to a single peer.
#[derive(Debug)]
pub struct ReliableChannel {
    compression: bool,
    // Sending side.
    next_send_seq: u32,
    acked_tail: u32,
    in_flight: VecDeque<Vec<u8>>,
    next_resend: Option<Instant>,
    // Receiving side.
    next_recv_seq: u32,
    out_of_order: BTreeMap<u32, Delivered>,
}

impl Default for ReliableChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl ReliableChannel {
    pub fn new() -> Self {
        Self {
            compression: false,
            next_send_seq: 0,
            acked_tail: 0,
            in_flight: VecDeque::new(),
            next_resend: None,
            next_recv_seq: 0,
            out_of_order: BTreeMap::new(),
        }
    }

    /// Turns on zlib for all later Party messages (`VersionMatch`).
    pub fn enable_compression(&mut self) {
        self.compression = true;
    }

    pub fn compression(&self) -> bool {
        self.compression
    }

    /// Bytes sent but not yet acknowledged.
    pub fn in_flight_bytes(&self) -> usize {
        self.in_flight.iter().map(Vec::len).sum()
    }

    fn wire(&self, frame: &[u8]) -> Vec<u8> {
        if self.compression {
            zlib_compress(frame)
        } else {
            frame.to_vec()
        }
    }

    /// Queues `body` as a numbered frame of `kind` and returns the Party
    /// message to send now.
    pub fn send(&mut self, kind: u8, body: &[u8], now: Instant) -> Vec<u8> {
        let mut frame = Vec::with_capacity(body.len() + TRAILER_LEN);
        frame.extend_from_slice(body);
        frame.extend_from_slice(&self.next_send_seq.to_le_bytes());
        frame.push(kind);
        self.next_send_seq = self.next_send_seq.wrapping_add(1);
        if self.in_flight.is_empty() {
            self.next_resend = Some(now + RETRANSMIT_FIRST);
        }
        let wire = self.wire(&frame);
        self.in_flight.push_back(frame);
        wire
    }

    fn ack(&self) -> Vec<u8> {
        let mut frame = Vec::with_capacity(TRAILER_LEN);
        frame.extend_from_slice(&self.next_recv_seq.to_le_bytes());
        frame.push(TYPE_ACK);
        self.wire(&frame)
    }

    /// Decodes a raw Party message into a frame, undoing compression.
    ///
    /// The two sides switch compression on at slightly different moments,
    /// so we accept both forms whatever our own setting. A zlib stream is
    /// tried first: it carries an Adler-32 checksum, so random raw bytes
    /// practically never decompress cleanly, while a raw frame's trailer
    /// check alone would misread about 1 in 85 compressed messages.
    fn decode(&self, msg: &[u8]) -> Option<Vec<u8>> {
        if let Some(f) = zlib_decompress(msg).filter(|f| trailer_ok(f)) {
            return Some(f);
        }
        trailer_ok(msg).then(|| msg.to_vec())
    }

    /// Feeds one Party message from the peer. Any frames that become
    /// deliverable are returned in order, with the acks to send back.
    ///
    /// Callers that switch compression based on a delivered frame (the
    /// `PeerInfo` reply) must do so before feeding the next message.
    pub fn receive(&mut self, msg: &[u8], now: Instant) -> Result<Received, FrameError> {
        let mut out = Received::default();
        let Some(frame) = self.decode(msg) else {
            tracing::debug!(len = msg.len(), "dropping undecodable party message");
            return Ok(out);
        };
        let (body, seq, kind) = split(&frame);
        if kind == TYPE_ACK {
            self.process_ack(seq, now)?;
            return Ok(out);
        }
        if seq != self.next_recv_seq {
            // Ahead of what we expect: buffer it. Behind: a duplicate.
            if seq.wrapping_sub(self.next_recv_seq) < i32::MAX as u32 {
                self.out_of_order.entry(seq).or_insert(Delivered {
                    kind,
                    body: body.to_vec(),
                });
            }
            out.outgoing.push(self.ack());
            return Ok(out);
        }
        out.delivered.push(Delivered {
            kind,
            body: body.to_vec(),
        });
        self.next_recv_seq = self.next_recv_seq.wrapping_add(1);
        while let Some(d) = self.out_of_order.remove(&self.next_recv_seq) {
            out.delivered.push(d);
            self.next_recv_seq = self.next_recv_seq.wrapping_add(1);
        }
        out.outgoing.push(self.ack());
        Ok(out)
    }

    fn process_ack(&mut self, next_expected: u32, now: Instant) -> Result<(), FrameError> {
        let mut progressed = false;
        while self.acked_tail != next_expected {
            if self.in_flight.pop_front().is_none() {
                return Err(FrameError::BogusAck);
            }
            self.acked_tail = self.acked_tail.wrapping_add(1);
            progressed = true;
        }
        if progressed {
            self.next_resend = (!self.in_flight.is_empty()).then(|| now + RETRANSMIT_FIRST);
        }
        Ok(())
    }

    /// Returns Party messages to retransmit if the oldest unacked frame is
    /// overdue. Party already delivers reliably, so this rarely fires.
    pub fn poll(&mut self, now: Instant) -> Vec<Vec<u8>> {
        match (self.next_resend, self.in_flight.front()) {
            (Some(at), Some(frame)) if now >= at => {
                self.next_resend = Some(now + RETRANSMIT_AGAIN);
                vec![self.wire(frame)]
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (ReliableChannel, ReliableChannel, Instant) {
        (
            ReliableChannel::new(),
            ReliableChannel::new(),
            Instant::now(),
        )
    }

    #[test]
    fn frame_layout() {
        let (mut a, _, now) = pair();
        let w0 = a.send(TYPE_INTERNAL, b"hi", now);
        assert_eq!(w0, vec![b'h', b'i', 0, 0, 0, 0, TYPE_INTERNAL]);
        let w1 = a.send(TYPE_DATA, b"x", now);
        assert_eq!(w1, vec![b'x', 1, 0, 0, 0, TYPE_DATA]);
    }

    #[test]
    fn in_order_delivery_and_ack() {
        let (mut a, mut b, now) = pair();
        let w = a.send(TYPE_DATA, b"one", now);
        let r = b.receive(&w, now).unwrap();
        assert_eq!(
            r.delivered,
            vec![Delivered {
                kind: TYPE_DATA,
                body: b"one".to_vec()
            }]
        );
        assert_eq!(r.outgoing, vec![vec![1, 0, 0, 0, TYPE_ACK]]);
        assert_eq!(a.in_flight_bytes(), 3 + TRAILER_LEN);
        a.receive(&r.outgoing[0], now).unwrap();
        assert_eq!(a.in_flight_bytes(), 0);
    }

    #[test]
    fn out_of_order_is_buffered() {
        let (mut a, mut b, now) = pair();
        let w0 = a.send(TYPE_DATA, b"0", now);
        let w1 = a.send(TYPE_DATA, b"1", now);
        let r = b.receive(&w1, now).unwrap();
        assert!(r.delivered.is_empty());
        assert_eq!(r.outgoing, vec![vec![0, 0, 0, 0, TYPE_ACK]]);
        let r = b.receive(&w0, now).unwrap();
        let bodies: Vec<_> = r.delivered.iter().map(|d| d.body.clone()).collect();
        assert_eq!(bodies, vec![b"0".to_vec(), b"1".to_vec()]);
        assert_eq!(r.outgoing, vec![vec![2, 0, 0, 0, TYPE_ACK]]);
    }

    #[test]
    fn duplicates_are_ignored() {
        let (mut a, mut b, now) = pair();
        let w0 = a.send(TYPE_DATA, b"0", now);
        assert_eq!(b.receive(&w0, now).unwrap().delivered.len(), 1);
        assert!(b.receive(&w0, now).unwrap().delivered.is_empty());
    }

    #[test]
    fn compressed_round_trip_and_switchover() {
        let (mut a, mut b, now) = pair();
        a.enable_compression();
        let w = a.send(TYPE_DATA, b"zipped", now);
        assert_eq!(&w[..1], &[0x78], "zlib header");
        // Receiver hasn't switched yet: it still decodes the compressed frame.
        let r = b.receive(&w, now).unwrap();
        assert_eq!(r.delivered[0].body, b"zipped");
        // Receiver switched, sender hasn't: raw frames still decode.
        b.enable_compression();
        let mut c = ReliableChannel::new();
        c.next_send_seq = 1;
        let raw = c.send(TYPE_DATA, b"raw", now);
        assert_eq!(b.receive(&raw, now).unwrap().delivered[0].body, b"raw");
    }

    #[test]
    fn retransmits_oldest_unacked() {
        let (mut a, _, now) = pair();
        let w = a.send(TYPE_DATA, b"x", now);
        assert!(a.poll(now + Duration::from_secs(1)).is_empty());
        assert_eq!(a.poll(now + Duration::from_secs(3)), vec![w.clone()]);
        assert!(a.poll(now + Duration::from_millis(3500)).is_empty());
        assert_eq!(a.poll(now + Duration::from_secs(4)), vec![w]);
    }

    #[test]
    fn bogus_ack_is_an_error() {
        let (mut a, _, now) = pair();
        assert_eq!(
            a.receive(&[5, 0, 0, 0, TYPE_ACK], now).unwrap_err(),
            FrameError::BogusAck
        );
    }
}
