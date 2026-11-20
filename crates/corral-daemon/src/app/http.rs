//! Optional HTTP entry point on the app listener. WebSockets keep their existing
//! upgrade parser, exact Origin check and Noise authentication.
use std::io::{self, Read};

use super::{
    HttpPermit, StaticHost, static_site,
    ws::{Bounded, HANDSHAKE_TIMEOUT, UPGRADE_MAX},
};

/// Sniff under the upgrade deadline, replaying every byte to tungstenite if
/// this is an upgrade. Classification alone does not admit an app session.
pub(super) fn route(
    mut stream: Bounded,
    host: &StaticHost,
    pending: HttpPermit,
    upgrade: impl FnOnce(Bounded, HttpPermit) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut bytes = Vec::with_capacity(1024);
    match read_head(&mut stream, &mut bytes) {
        Ok(true) => {}
        Ok(false) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            return Ok(static_site::error(&mut stream, 431)?);
        }
        Err(error) => return Err(error.into()),
    }
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut request = httparse::Request::new(&mut headers);
    match request.parse(&bytes) {
        Ok(httparse::Status::Complete(_)) => {}
        Err(httparse::Error::TooManyHeaders) => return Ok(static_site::error(&mut stream, 431)?),
        _ => return Ok(static_site::error(&mut stream, 400)?),
    }
    if request
        .headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("upgrade"))
    {
        stream.replay(bytes);
        return upgrade(stream, pending);
    }
    let permit = HttpPermit::acquire(&host.transfers, super::MAX_STATIC_TRANSFERS);
    stream.budget(HANDSHAKE_TIMEOUT, HANDSHAKE_TIMEOUT);
    let Some(_permit) = permit else {
        return Ok(static_site::error(&mut stream, 503)?);
    };
    drop(pending);
    Ok(static_site::serve(&mut stream, &host.root, &request)?)
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
