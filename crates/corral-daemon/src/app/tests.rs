use super::*;
use std::net::TcpStream;
use std::os::unix::{
    fs::PermissionsExt,
    net::{UnixListener, UnixStream},
};
use std::time::Duration;

use corral_core::Status;
use corral_protocol::{
    self as wire,
    device::*,
    frame::{Frame, FrameKind, decode_message, encode_message, read_frame, write_frame},
    rpc,
};
use serde::Serialize;
use serde_json::{Value, json};
use snow::{HandshakeState, TransportState};
use tungstenite::{
    Message, WebSocket, client::IntoClientRequest, protocol::frame::coding::CloseCode,
};

const ORIGIN: &str = "http://127.0.0.1:8790";

fn hello() -> wire::Hello {
    wire::Hello {
        proto: wire::Proto {
            major: wire::PROTO_MAJOR,
            minor: wire::PROTO_MINOR,
        },
        client: wire::ClientInfo {
            kind: "app".into(),
            version: "test".into(),
        },
        caps: vec![
            wire::BOARD_V1.into(),
            DEVICE_V1.into(),
            wire::cmd::CMD_V1.into(),
        ],
    }
}

fn frame(id: u64, method: &str, params: impl Serialize) -> Frame {
    Frame {
        kind: FrameKind::Control,
        stream: 0,
        payload: serde_json::to_vec(&rpc::Request {
            jsonrpc: rpc::JSONRPC.into(),
            id: json!(id),
            method: method.into(),
            params,
        })
        .unwrap(),
    }
}

fn unix_call(stream: &mut UnixStream, method: &str, params: impl Serialize) -> Value {
    write_frame(stream, &frame(1, method, params)).unwrap();
    let response = read_frame(stream).unwrap().unwrap();
    let response: rpc::Response = serde_json::from_slice(&response.payload).unwrap();
    match response.outcome {
        rpc::ResponseOutcome::Result { result } => result,
        rpc::ResponseOutcome::Error { error } => panic!("{method}: {error:?}"),
    }
}

fn upgrade(
    port: u16,
    path: &str,
    origin: &str,
) -> Result<WebSocket<TcpStream>, tungstenite::Error> {
    let mut request = format!("ws://127.0.0.1:{port}{path}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Origin", origin.parse().unwrap());
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    tungstenite::client(request, stream)
        .map(|(socket, _)| socket)
        .map_err(|error| match error {
            tungstenite::HandshakeError::Failure(error) => error,
            other => panic!("unexpected handshake interruption: {other:?}"),
        })
}

fn noise_start(
    code: &PairCode,
    private: &[u8],
    pair: bool,
) -> (WebSocket<TcpStream>, HandshakeState) {
    let path = if pair {
        format!("{PAIR_PATH_PREFIX}{}", code.pair_id_hex())
    } else {
        SESSION_PATH.into()
    };
    let mut socket = upgrade(code.port, &path, ORIGIN).unwrap();
    let params = if pair { NOISE_PAIR } else { NOISE_SESSION }
        .parse()
        .unwrap();
    let builder = snow::Builder::new(params)
        .local_private_key(private)
        .unwrap()
        .remote_public_key(&code.daemon_public)
        .unwrap();
    let builder = if pair {
        builder.psk(2, &code.token).unwrap()
    } else {
        builder
    };
    let mut noise = builder.build_initiator().unwrap();
    let mut bytes = [0; 128];
    let size = noise.write_message(&[], &mut bytes).unwrap();
    assert_eq!(size, HANDSHAKE_FIRST_LEN);
    socket
        .send(Message::Binary(bytes[..size].to_vec().into()))
        .unwrap();
    (socket, noise)
}

struct Client {
    socket: WebSocket<TcpStream>,
    noise: TransportState,
    next: u64,
}
impl Client {
    fn finish(mut socket: WebSocket<TcpStream>, mut noise: HandshakeState) -> (Self, String) {
        let Message::Binary(response) = socket.read().unwrap() else {
            panic!("Noise response is not binary");
        };
        assert_eq!(noise.read_message(&response, &mut [0; 128]).unwrap(), 0);
        let sas = sas(noise.get_handshake_hash());
        (
            Self {
                socket,
                noise: noise.into_transport_mode().unwrap(),
                next: 0,
            },
            sas,
        )
    }
    fn recv(&mut self) -> Value {
        loop {
            match self.socket.read().unwrap() {
                Message::Binary(bytes) => {
                    let mut plain = vec![0; bytes.len()];
                    let len = self.noise.read_message(&bytes, &mut plain).unwrap();
                    let frame = decode_message(&plain[..len]).unwrap();
                    assert_eq!((frame.kind, frame.stream), (FrameKind::Control, 0));
                    return serde_json::from_slice(&frame.payload).unwrap();
                }
                Message::Ping(_) | Message::Pong(_) => {}
                other => panic!("expected app frame: {other:?}"),
            }
        }
    }
    fn call(&mut self, method: &str, params: impl Serialize) -> Value {
        self.next += 1;
        let plain = encode_message(&frame(self.next, method, params)).unwrap();
        let mut cipher = vec![0; plain.len() + NOISE_TAG];
        let len = self.noise.write_message(&plain, &mut cipher).unwrap();
        cipher.truncate(len);
        self.socket.send(Message::Binary(cipher.into())).unwrap();
        loop {
            let response = self.recv();
            if response["id"] == self.next {
                return response;
            }
        }
    }
    fn closed(&mut self, expected: &str) {
        loop {
            match self.socket.read().unwrap() {
                Message::Close(Some(close)) => {
                    assert_eq!(close.code, CloseCode::Policy);
                    assert_eq!(close.reason, expected);
                    return;
                }
                Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {}
                other => panic!("expected close: {other:?}"),
            }
        }
    }
}

#[test]
fn paired_browser_reads_board_and_revocation_closes_it() {
    let fixture = crate::tests::Fixture::new();
    let first = fixture
        .ctx
        .store
        .create("fixture A", None, Status::Ready, "")
        .unwrap();
    let second = fixture
        .ctx
        .store
        .create("fixture B", None, Status::Ready, "")
        .unwrap();
    let shared = Arc::new(Shared::new());
    let projector = crate::projector::start(&fixture.ctx, &shared).unwrap();
    crate::devices::start(&fixture.ctx, &shared, Some(fixture.root.join("state"))).unwrap();
    let socket_path = fixture.root.join("d.sock");
    crate::server::start(
        UnixListener::bind(&socket_path).unwrap(),
        Arc::clone(&shared),
        fixture.ctx.store.dir(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    serve_listener(
        listener,
        Arc::clone(&shared),
        vec![ORIGIN.into(), "https://corral.example".into()],
    )
    .unwrap();

    let error = upgrade(port, SESSION_PATH, "http://evil.example").unwrap_err();
    assert!(matches!(error, tungstenite::Error::Http(response) if response.status() == 403));
    let mut local = UnixStream::connect(&socket_path).unwrap();
    local
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    unix_call(&mut local, wire::HELLO, hello());
    let offer: PairOffer =
        serde_json::from_value(unix_call(&mut local, PAIR_BEGIN, PairBegin {})).unwrap();
    let code = PairCode::decode(&offer.code).unwrap();
    assert_eq!(code.port, port);
    let url_code = offer
        .url
        .as_deref()
        .unwrap()
        .strip_prefix(&format!("{ORIGIN}/#{PAIR_LINK_FRAGMENT}"))
        .unwrap();
    assert_eq!(url_code, offer.code);
    assert_eq!(PairCode::decode(url_code).unwrap(), code);
    let key = snow::Builder::new(NOISE_SESSION.parse().unwrap())
        .generate_keypair()
        .unwrap();
    let (socket, noise) = noise_start(&code, &key.private, true);
    let (mut pairing, expected_sas) = Client::finish(socket, noise);
    assert_eq!(
        pairing.call(wire::HELLO, hello())["result"]["caps"],
        json!([])
    );
    assert_eq!(
        pairing.call(
            PAIR_CLAIM,
            PairClaim {
                name: "Browser".into(),
                app_version: "test".into()
            }
        )["result"]["sas"],
        expected_sas
    );
    let status: PairStatus = serde_json::from_value(unix_call(
        &mut local,
        PAIR_STATUS,
        PairStatusQuery {
            pair_id: offer.pair_id.clone(),
            wait_ms: 0,
        },
    ))
    .unwrap();
    assert!(matches!(status.state, PairState::Claimed { sas, .. } if sas == expected_sas));
    let (mut reused, _) = noise_start(&code, &key.private, true);
    assert!(
        matches!(reused.read().unwrap(), Message::Close(Some(close)) if close.reason == CLOSE_PAIRING_UNAVAILABLE)
    );
    drop(reused);
    let device: DeviceInfo = serde_json::from_value(unix_call(
        &mut local,
        PAIR_CONFIRM,
        PairConfirm {
            pair_id: offer.pair_id,
        },
    ))
    .unwrap();
    let done = pairing.recv();
    assert_eq!(done["method"], PAIR_DONE);
    assert_eq!(done["params"]["device"], device.id);
    drop(pairing);

    let (socket, noise) = noise_start(&code, &key.private, false);
    let (mut browser, _) = Client::finish(socket, noise);
    let welcome = browser.call(wire::HELLO, hello());
    assert_eq!(welcome["result"]["caps"], json!([wire::BOARD_V1]));
    assert_eq!(
        browser.call(wire::SUB_OPEN, json!({"topic": "board", "since": null}))["result"],
        Value::Null
    );
    let delta = browser.recv();
    assert_eq!(delta["method"], wire::SUB_DELTA);
    let mut ids: Vec<_> = delta["params"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    assert_eq!(ids, vec![first.id, second.id]);
    for method in [
        wire::cmd::CMD_SUBMIT,
        wire::NUDGE,
        wire::READ,
        wire::DAEMON_STOP,
    ] {
        let refused = browser.call(method, json!({}));
        assert_eq!(refused["error"]["code"], "refused");
        assert_eq!(refused["error"]["message"], "app_method_refused");
    }
    unix_call(
        &mut local,
        DEVICE_REVOKE,
        DeviceRevoke { device: device.id },
    );
    browser.closed(CLOSE_REVOKED);
    let (mut unknown, _) = noise_start(&code, &key.private, false);
    assert!(
        matches!(unknown.read().unwrap(), Message::Close(Some(close)) if close.reason == CLOSE_DEVICE_UNKNOWN)
    );
    let store_id = corral_core::agent::paths::store_id(&fixture.ctx.store);
    let key_path = fixture
        .root
        .join("state/corral")
        .join(store_id)
        .join("identity.key");
    assert_eq!(
        std::fs::metadata(key_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(local);
    drop(browser);
    drop(unknown);
    shared.stop(false);
    shared.close();
    shared.wait_workers(Duration::from_secs(5));
    projector.join().unwrap();
}
