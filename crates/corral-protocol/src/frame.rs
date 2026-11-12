//! Unix transport framing. Limits apply to payload bytes, excluding the five-byte header.
use std::io::{self, Read, Write};

pub const CONTROL_MAX: usize = 60 * 1024;
pub const DATA_MAX: usize = 32 * 1024;
const HEADER_LEN: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Control = 0,
    Data = 1,
    Credit = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    pub stream: u32,
    pub payload: Vec<u8>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn validate(kind: FrameKind, len: usize) -> io::Result<()> {
    match kind {
        FrameKind::Control if len <= CONTROL_MAX => Ok(()),
        FrameKind::Data if len <= DATA_MAX => Ok(()),
        FrameKind::Credit if len == 4 => Ok(()),
        _ => Err(invalid("invalid frame payload length")),
    }
}

fn read_exact(r: &mut impl Read, bytes: &mut [u8]) -> io::Result<()> {
    r.read_exact(bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            invalid("truncated frame")
        } else {
            error
        }
    })
}

pub fn write_frame(w: &mut impl Write, frame: &Frame) -> io::Result<()> {
    validate(frame.kind, frame.payload.len())?;
    let len = (HEADER_LEN + frame.payload.len()) as u32;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&[frame.kind as u8])?;
    w.write_all(&frame.stream.to_le_bytes())?;
    w.write_all(&frame.payload)
}

/// Returns `None` only when EOF precedes every byte of the next frame.
/// The length and kind are checked before allocating any payload storage.
pub fn read_frame(r: &mut impl Read) -> io::Result<Option<Frame>> {
    let mut prefix = [0; 4];
    loop {
        match r.read(&mut prefix[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    read_exact(r, &mut prefix[1..])?;
    let len = u32::from_le_bytes(prefix) as usize;
    if !(HEADER_LEN..=HEADER_LEN + CONTROL_MAX).contains(&len) {
        return Err(invalid("invalid frame length"));
    }
    let mut header = [0; HEADER_LEN];
    read_exact(r, &mut header)?;
    let kind = match header[0] {
        0 => FrameKind::Control,
        1 => FrameKind::Data,
        2 => FrameKind::Credit,
        _ => return Err(invalid("unknown frame kind")),
    };
    let payload_len = len - HEADER_LEN;
    validate(kind, payload_len)?;
    let stream = u32::from_le_bytes([header[1], header[2], header[3], header[4]]);
    let mut payload = vec![0; payload_len];
    read_exact(r, &mut payload)?;
    Ok(Some(Frame {
        kind,
        stream,
        payload,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_rejects_unbounded_and_truncated_inputs() {
        for (kind, payload) in [
            (FrameKind::Control, b"{}".to_vec()),
            (FrameKind::Data, vec![0, 255]),
            (FrameKind::Credit, 42_u32.to_le_bytes().to_vec()),
        ] {
            let frame = Frame {
                kind,
                stream: 7,
                payload,
            };
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &frame).unwrap();
            assert_eq!(
                u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize,
                bytes.len() - 4
            );
            let mut reader = bytes.as_slice();
            assert_eq!(read_frame(&mut reader).unwrap(), Some(frame));
            assert_eq!(read_frame(&mut reader).unwrap(), None);
            bytes.pop();
            assert_eq!(
                read_frame(&mut bytes.as_slice()).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
        // This reader has only a prefix: rejection must precede any header/payload read
        // or allocation based on its claimed four-gigabyte length.
        assert_eq!(
            read_frame(&mut u32::MAX.to_le_bytes().as_slice())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let mut unknown = 5_u32.to_le_bytes().to_vec();
        unknown.extend_from_slice(&[255, 0, 0, 0, 0]);
        assert_eq!(
            read_frame(&mut unknown.as_slice()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
