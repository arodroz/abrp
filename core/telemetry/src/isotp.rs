//! ISO 15765-2 (ISO-TP) framing (wayfinder #76): encodes outgoing UDS
//! requests as Single Frames and reassembles incoming Single/First/
//! Consecutive Frames into complete UDS payloads. Operates on raw CAN data
//! bytes (PCI byte included) -- the engine runs with ATCAF0 (auto-formatting
//! off), so this is the only place frame structure is parsed. See
//! `docs/research/ioniq5-obd-telemetry.md` §3.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsoTpError {
    /// `encode_single_frame`: payload was empty or over 7 bytes.
    InvalidLength { len: usize },
    /// A frame was shorter than its own PCI declared (missing PCI byte,
    /// or a Single/First Frame truncated before its stated length).
    FrameTooShort,
    /// First Frame announced a total length past `MAX_REASSEMBLY_LEN`.
    LengthOverflow { len: usize },
    /// Consecutive Frame arrived with no First Frame open (never started,
    /// or a prior error already closed it).
    ConsecutiveWithoutFirst,
    /// Consecutive Frame sequence nibble didn't match the expected next
    /// value (mod 16).
    SequenceGap { expected: u8, got: u8 },
}

impl fmt::Display for IsoTpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IsoTpError::InvalidLength { len } => {
                write!(
                    f,
                    "UDS payload of {len} bytes does not fit a single frame (need 1..=7)"
                )
            }
            IsoTpError::FrameTooShort => write!(f, "CAN frame shorter than its PCI declared"),
            IsoTpError::LengthOverflow { len } => {
                write!(
                    f,
                    "First Frame announced length {len}, too large to reassemble"
                )
            }
            IsoTpError::ConsecutiveWithoutFirst => {
                write!(f, "Consecutive Frame received without an open First Frame")
            }
            IsoTpError::SequenceGap { expected, got } => {
                write!(
                    f,
                    "Consecutive Frame sequence gap: expected {expected}, got {got}"
                )
            }
        }
    }
}

impl std::error::Error for IsoTpError {}

/// Encodes `uds` as an ISO-TP Single Frame: PCI byte `0x0N` (N = `uds.len()`,
/// must be 1..=7), then `uds`, zero-padded to 8 bytes.
pub fn encode_single_frame(uds: &[u8]) -> Result<[u8; 8], IsoTpError> {
    if uds.is_empty() || uds.len() > 7 {
        return Err(IsoTpError::InvalidLength { len: uds.len() });
    }
    let mut frame = [0u8; 8];
    frame[0] = uds.len() as u8;
    frame[1..1 + uds.len()].copy_from_slice(uds);
    Ok(frame)
}

/// Defensive cap on a First Frame's 12-bit announced length. Well past any
/// UDS response this engine expects (the two DIDs in
/// `docs/research/ioniq5-obd-telemetry.md` §2 top out around 62 bytes); it
/// only guards against a garbled length driving an unbounded allocation.
const MAX_REASSEMBLY_LEN: usize = 4095;

struct Pending {
    total_len: usize,
    data: Vec<u8>,
    next_seq: u8,
}

/// Reassembles ISO-TP frames into complete UDS payloads. Tracks at most one
/// in-flight multi-frame message: a new Single or First Frame fed while one
/// is pending discards it and starts fresh (the ECU restarting its response
/// takes precedence over a stalled one).
#[derive(Default)]
pub struct Reassembler {
    pending: Option<Pending>,
}

impl Reassembler {
    pub fn new() -> Self {
        Reassembler::default()
    }

    /// Feeds one CAN frame's data bytes, PCI byte included (exactly what
    /// followed the 11-bit id on the wire). Returns the reassembled payload
    /// once the announced length is reached -- trailing zero padding past
    /// that length is truncated -- or `Ok(None)` while still incomplete.
    pub fn feed_frame(&mut self, data: &[u8]) -> Result<Option<Vec<u8>>, IsoTpError> {
        let Some(&pci) = data.first() else {
            return Err(IsoTpError::FrameTooShort);
        };
        match pci >> 4 {
            0 => self.feed_single(pci, data),
            1 => self.feed_first(pci, data),
            2 => self.feed_consecutive(pci, data),
            // Flow Control (3) or a reserved nibble: never expected on the
            // frames this engine (the requester) reads back; ignore.
            _ => Ok(None),
        }
    }

    fn feed_single(&mut self, pci: u8, data: &[u8]) -> Result<Option<Vec<u8>>, IsoTpError> {
        let len = (pci & 0x0F) as usize;
        if data.len() < 1 + len {
            return Err(IsoTpError::FrameTooShort);
        }
        self.pending = None;
        Ok(Some(data[1..1 + len].to_vec()))
    }

    fn feed_first(&mut self, pci: u8, data: &[u8]) -> Result<Option<Vec<u8>>, IsoTpError> {
        if data.len() < 2 {
            return Err(IsoTpError::FrameTooShort);
        }
        let len = (((pci & 0x0F) as usize) << 8) | data[1] as usize;
        if len > MAX_REASSEMBLY_LEN {
            return Err(IsoTpError::LengthOverflow { len });
        }
        let mut buf = data[2..].to_vec();
        buf.truncate(len.min(buf.len()));
        if buf.len() >= len {
            self.pending = None;
            return Ok(Some(buf));
        }
        self.pending = Some(Pending {
            total_len: len,
            data: buf,
            next_seq: 1,
        });
        Ok(None)
    }

    fn feed_consecutive(&mut self, pci: u8, data: &[u8]) -> Result<Option<Vec<u8>>, IsoTpError> {
        let seq = pci & 0x0F;
        let Some(pending) = self.pending.as_mut() else {
            return Err(IsoTpError::ConsecutiveWithoutFirst);
        };
        if seq != pending.next_seq {
            let expected = pending.next_seq;
            self.pending = None;
            return Err(IsoTpError::SequenceGap { expected, got: seq });
        }
        let remaining = pending.total_len - pending.data.len();
        let take = remaining.min(data.len().saturating_sub(1));
        pending.data.extend_from_slice(&data[1..1 + take]);
        pending.next_seq = (pending.next_seq + 1) % 16;
        if pending.data.len() >= pending.total_len {
            let out = std::mem::take(&mut pending.data);
            self.pending = None;
            return Ok(Some(out));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_single_frame_pads_and_sets_pci() {
        assert_eq!(
            encode_single_frame(&[0x22, 0x01, 0x01]).unwrap(),
            [0x03, 0x22, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn encode_single_frame_rejects_empty_and_oversized() {
        assert_eq!(
            encode_single_frame(&[]),
            Err(IsoTpError::InvalidLength { len: 0 })
        );
        assert_eq!(
            encode_single_frame(&[0; 8]),
            Err(IsoTpError::InvalidLength { len: 8 })
        );
        assert!(encode_single_frame(&[0; 7]).is_ok());
    }

    #[test]
    fn reassembler_single_frame_returns_payload_immediately() {
        let mut r = Reassembler::new();
        let out = r
            .feed_frame(&[0x03, 0x62, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00])
            .unwrap();
        assert_eq!(out, Some(vec![0x62, 0x01, 0x01]));
    }

    /// First Frame (6 bytes) + one Consecutive Frame (7 bytes) landing on
    /// exactly the announced length, with no trailing padding to truncate.
    #[test]
    fn reassembler_exact_fit_final_consecutive_frame() {
        let mut r = Reassembler::new();
        let expected: Vec<u8> = (0..13).collect();
        assert_eq!(
            r.feed_frame(&[
                0x10,
                13,
                expected[0],
                expected[1],
                expected[2],
                expected[3],
                expected[4],
                expected[5]
            ])
            .unwrap(),
            None
        );
        let mut cf = vec![0x21];
        cf.extend_from_slice(&expected[6..13]);
        assert_eq!(r.feed_frame(&cf).unwrap(), Some(expected));
    }

    /// Final Consecutive Frame carries fewer than 7 payload bytes and is
    /// not padded to 8 -- the short frame itself, not just the payload
    /// inside it, is short.
    #[test]
    fn reassembler_short_final_consecutive_frame() {
        let mut r = Reassembler::new();
        let expected: Vec<u8> = (0..9).collect();
        r.feed_frame(&[
            0x10,
            9,
            expected[0],
            expected[1],
            expected[2],
            expected[3],
            expected[4],
            expected[5],
        ])
        .unwrap();
        let cf = vec![0x21, expected[6], expected[7], expected[8]];
        assert_eq!(r.feed_frame(&cf).unwrap(), Some(expected));
    }

    /// A padded final CF (full 8 bytes) that reaches the announced length
    /// partway through: the trailing pad bytes must not leak into the
    /// payload.
    #[test]
    fn reassembler_truncates_padding_past_announced_length() {
        let mut r = Reassembler::new();
        let expected: Vec<u8> = (0..9).collect();
        r.feed_frame(&[
            0x10,
            9,
            expected[0],
            expected[1],
            expected[2],
            expected[3],
            expected[4],
            expected[5],
        ])
        .unwrap();
        let cf = [
            0x21,
            expected[6],
            expected[7],
            expected[8],
            0xAA,
            0xAA,
            0xAA,
            0xAA,
        ];
        assert_eq!(r.feed_frame(&cf).unwrap(), Some(expected));
    }

    #[test]
    fn reassembler_consecutive_without_first_errors() {
        let mut r = Reassembler::new();
        assert_eq!(
            r.feed_frame(&[0x21, 1, 2, 3, 4, 5, 6, 7]),
            Err(IsoTpError::ConsecutiveWithoutFirst)
        );
    }

    #[test]
    fn reassembler_sequence_gap_errors() {
        let mut r = Reassembler::new();
        r.feed_frame(&[0x10, 20, 0, 1, 2, 3, 4, 5]).unwrap();
        assert_eq!(r.feed_frame(&[0x21, 6, 7, 8, 9, 10, 11, 12]).unwrap(), None);
        assert_eq!(
            r.feed_frame(&[0x23, 13, 14, 15, 16, 17, 18, 19]),
            Err(IsoTpError::SequenceGap {
                expected: 2,
                got: 3
            })
        );
    }

    /// 16 Consecutive Frames (6 + 7*16 = 118 bytes, past the 111-byte point
    /// where a 15th CF would hit sequence 15) exercise the nibble wrapping
    /// 15 -> 0 rather than erroring.
    #[test]
    fn reassembler_sequence_wraps_past_15_to_0() {
        let mut r = Reassembler::new();
        let expected: Vec<u8> = (0..118u16).map(|b| b as u8).collect();
        let len = expected.len();
        r.feed_frame(&[
            0x10 | (((len >> 8) & 0x0F) as u8),
            (len & 0xFF) as u8,
            expected[0],
            expected[1],
            expected[2],
            expected[3],
            expected[4],
            expected[5],
        ])
        .unwrap();

        let mut offset = 6;
        let mut result = None;
        for i in 0..16u8 {
            let seq = (i + 1) % 16;
            let mut cf = vec![0x20 | seq];
            cf.extend_from_slice(&expected[offset..offset + 7]);
            offset += 7;
            result = r.feed_frame(&cf).unwrap();
        }
        assert_eq!(result, Some(expected));
    }
}
