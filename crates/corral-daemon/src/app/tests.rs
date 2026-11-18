use super::*;
use std::net::TcpStream;
use std::time::Duration;

use corral_protocol::{
    self as wire,
    device::*,
    frame::{Frame, FrameKind, decode_message, encode_message},
    rpc,
};
use corral_types::{NewTask, Op};
use serde::Serialize;
use serde_json::{Value, json};
use snow::{HandshakeState, TransportState};
use tungstenite::{
    Message, WebSocket, client::IntoClientRequest, protocol::frame::coding::CloseCode,
};

use crate::client::Client as LocalClient;

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
            wire::TASKS_V1.into(),
            DEVICE_V1.into(),
            wire::ASSISTANT_V1.into(),
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
fn paired_browser_is_owner_until_revocation_closes_it() {
    let fixture = crate::tests::Fixture::new();
    fixture.unix_server();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    serve_listener(
        listener,
        Arc::clone(fixture.shared()),
        vec![ORIGIN.into()],
        None,
    )
    .unwrap();

    let error = upgrade(port, SESSION_PATH, "http://evil.example").unwrap_err();
    assert!(matches!(error, tungstenite::Error::Http(response) if response.status() == 403));
    let mut local = LocalClient::connect(fixture.root()).unwrap().unwrap();
    let wait = Duration::from_secs(5);
    let offer: PairOffer = local.request(PAIR_BEGIN, PairBegin {}, wait).unwrap();
    let code = PairCode::decode(&offer.code).unwrap();
    assert_eq!(code.port, port);
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
                app_version: "test".into(),
            }
        )["result"]["sas"],
        expected_sas
    );
    let status: PairStatus = local
        .request(
            PAIR_STATUS,
            PairStatusQuery {
                pair_id: offer.pair_id.clone(),
                wait_ms: 0,
            },
            wait,
        )
        .unwrap();
    assert!(matches!(status.state, PairState::Claimed { sas, .. } if sas == expected_sas));
    let (mut reused, _) = noise_start(&code, &key.private, true);
    assert!(
        matches!(reused.read().unwrap(), Message::Close(Some(close)) if close.reason == CLOSE_PAIRING_UNAVAILABLE)
    );
    let device: DeviceInfo = local
        .request(
            PAIR_CONFIRM,
            PairConfirm {
                pair_id: offer.pair_id,
            },
            wait,
        )
        .unwrap();
    let done = pairing.recv();
    assert_eq!(done["method"], PAIR_DONE);
    assert_eq!(done["params"]["device"], device.id);
    drop(pairing);

    let (socket, noise) = noise_start(&code, &key.private, false);
    let (mut browser, _) = Client::finish(socket, noise);
    let welcome = browser.call(wire::HELLO, hello());
    assert_eq!(
        welcome["result"]["caps"],
        json!([wire::TASKS_V1, DEVICE_V1])
    );
    let app_session = browser.call(wire::app::APP_SESSION, json!({}));
    assert_eq!(app_session["result"]["device"]["id"], device.id);
    let acted = browser.call(
        wire::op::OP_SUBMIT,
        wire::op::OpSubmit {
            op: Op::New {
                task: NewTask {
                    title: "created by owner on Noise".into(),
                    ..NewTask::default()
                },
            },
            force: false,
        },
    );
    let id = acted["result"]["task"].as_str().unwrap();
    assert_eq!(
        fixture
            .shared()
            .engine()
            .unwrap()
            .store
            .require_task(id)
            .unwrap()
            .title,
        "created by owner on Noise"
    );
    let offer = browser.call(PAIR_BEGIN, PairBegin {});
    let pair_id = offer["result"]["pair_id"].as_str().unwrap().to_owned();
    assert!(
        browser
            .call(PAIR_CANCEL, PairCancel { pair_id })
            .get("error")
            .is_none()
    );

    let revoked = browser.call(DEVICE_REVOKE, DeviceRevoke { device: device.id });
    assert!(revoked.get("error").is_none(), "{revoked}");
    browser.closed(CLOSE_REVOKED);
    let (mut unknown, _) = noise_start(&code, &key.private, false);
    assert!(
        matches!(unknown.read().unwrap(), Message::Close(Some(close)) if close.reason == CLOSE_DEVICE_UNKNOWN)
    );
}
