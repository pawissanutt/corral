//! Optional HTTP entry point on the app listener. WebSockets keep their existing
//! upgrade parser, exact Origin check and Noise authentication.
use std::{
    io::{self, Read},
    time::Duration,
};

use tungstenite::handshake::headers::MAX_HEADERS;

use super::{
    HttpPermit, StaticHost, report, static_site,
    ws::{Bounded, UPGRADE_MAX},
};

// The uncompressed app WASM is about 14 MiB. A streaming TLS proxy can
// propagate a remote browser's backpressure all the way to this socket.
const STATIC_TRANSFER_TIMEOUT: Duration = Duration::from_secs(180);

/// Sniff under the upgrade deadline, replaying every byte to tungstenite if
/// this is an upgrade. Classification alone does not admit an app session.
pub(super) fn route(
    mut stream: Bounded,
    host: &StaticHost,
    pending: HttpPermit,
    upgrade: impl FnOnce(Bounded, HttpPermit),
) {
    let mut bytes = Vec::with_capacity(1024);
    match read_head(&mut stream, &mut bytes) {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            return send_error(&mut stream, 431);
        }
        Err(error) => return report(Err(error.into())),
    }
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut request = httparse::Request::new(&mut headers);
    match request.parse(&bytes) {
        Ok(httparse::Status::Complete(_)) => {}
        Err(httparse::Error::TooManyHeaders) => return send_error(&mut stream, 431),
        _ => return send_error(&mut stream, 400),
    }
    if request.headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("upgrade")
            && header
                .value
                .split(|&byte| byte == b',')
                .any(|value| value.trim_ascii().eq_ignore_ascii_case(b"websocket"))
    }) {
        stream.replay(bytes);
        return upgrade(stream, pending);
    }
    let Some(_permit) = host.acquire_transfer() else {
        return send_error(&mut stream, 503);
    };
    drop(pending);
    stream.static_transfer(STATIC_TRANSFER_TIMEOUT);
    report(static_site::serve(&mut stream, &host.root, &request).map_err(Into::into));
}

fn send_error(stream: &mut Bounded, status: u16) {
    report(static_site::error(stream, status).map_err(Into::into));
}

fn read_head(stream: &mut impl Read, bytes: &mut Vec<u8>) -> io::Result<bool> {
    let mut part = [0; 1024];
    while bytes.len() < UPGRADE_MAX {
        let limit = part.len().min(UPGRADE_MAX - bytes.len());
        let count = stream.read(&mut part[..limit])?;
        if count == 0 {
            return Ok(false);
        }
        let boundary = bytes.len().saturating_sub(3);
        bytes.extend_from_slice(&part[..count]);
        let tail = &bytes[boundary..];
        if tail.windows(4).any(|w| w == b"\r\n\r\n") || tail.windows(2).any(|w| w == b"\n\n") {
            return Ok(true);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "request headers too large",
    ))
}
