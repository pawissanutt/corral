use super::*;

use corral_protocol::frame::{Frame, FrameKind};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::{
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_tungstenite::{
    WebSocketStream, accept_hdr_async,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
    },
};

#[test]
fn windows_pairing_requires_matching_address_and_exchanges_noise_over_loopback() {
    let ui = nsh_core::element::Runtime::default();
    let _enter = ui.enter();
    let feed = signal(Feed::default());
    let runtime = socket::runtime().unwrap();
    runtime.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let endpoint = format!("ws://127.0.0.1:{port}");
        let daemon = noise::generate_keypair().unwrap();
        let code = PairCode {
            daemon_public: daemon.public.try_into().unwrap(),
            token: [17; 32],
            port,
            pair_id: [23; 8],
            endpoint: None,
        };
        let (sender, mut receiver) = mpsc::unbounded();
        let mut driver = Driver::new(feed, sender, Clock::now());
        let wrong = format!("ws://localhost:{port}");
        let mismatched = format!("ws://127.0.0.1:{}", if port == 1 { 2 } else { 1 });
        for refused in [None, Some(wrong), Some(mismatched)] {
            driver
                .handle(Input::Request(ClientRequest::Pair {
                    code: code.encode(),
                    endpoint: refused,
                    name: "Windows fixture".into(),
                }))
                .await;
            assert!(driver.connection.is_none());
            assert!(
                feed.with_untracked(|feed| matches!(
                    feed.browser_pairing,
                    BrowserPairing::Failed(_)
                ))
            );
        }

        // The printed link supplies the same code; its page URL is not dialed.
        let link = format!(
            "http://127.0.0.1:{port}/#{PAIR_LINK_FRAGMENT}{}",
            code.encode()
        );
        let fixture = runtime.spawn(serve_pairing(listener, code, daemon.private));
        driver
            .handle(Input::Request(ClientRequest::Pair {
                code: link,
                endpoint: Some(endpoint),
                name: "Windows fixture".into(),
            }))
            .await;
        let observed_sas = timeout(Duration::from_secs(5), async {
            let mut shown = None;
            while let Some(input) = receiver.next().await {
                driver.handle(input).await;
                let done = feed.with_untracked(|feed| match &feed.browser_pairing {
                    BrowserPairing::Claimed { sas } => {
                        shown = Some(sas.clone());
                        false
                    }
                    BrowserPairing::Failed(_) => true,
                    _ => false,
                });
                if done {
                    return shown;
                }
            }
            shown
        })
        .await
        .expect("the loopback driver did not finish pairing");
        let daemon_sas = timeout(Duration::from_secs(5), fixture)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(observed_sas.as_deref(), Some(daemon_sas.as_str()));
        assert!(driver.connection.is_none());
        assert!(driver.retry_at.is_none());
    });
}

async fn serve_pairing(listener: TcpListener, code: PairCode, private: Vec<u8>) -> String {
    let (stream, _) = listener.accept().await.unwrap();
    let path = format!("{PAIR_PATH_PREFIX}{}", code.pair_id_hex());
    let origin = format!("http://127.0.0.1:{}", code.port);
    let mut socket = accept_hdr_async(stream, |request: &Request, response: Response| {
        assert_eq!(request.uri().path(), path);
        let mut origins = request.headers().get_all("Origin").iter();
        assert_eq!(origins.next().unwrap().to_str().unwrap(), origin);
        assert!(origins.next().is_none());
        Ok(response)
    })
    .await
    .unwrap();

    let first = binary(&mut socket).await;
    assert_eq!(first.len(), HANDSHAKE_FIRST_LEN);
    let mut handshake = snow::Builder::new(NOISE_PAIR.parse().unwrap())
        .local_private_key(&private)
        .unwrap()
        .psk(2, &code.token)
        .unwrap()
        .build_responder()
        .unwrap();
    let mut buf = [0; 128];
    assert_eq!(handshake.read_message(&first, &mut buf).unwrap(), 0);
    let len = handshake.write_message(&[], &mut buf).unwrap();
    socket
        .send(Message::binary(buf[..len].to_vec()))
        .await
        .unwrap();
    let expected_sas = sas(handshake.get_handshake_hash());
    let mut noise = handshake.into_transport_mode().unwrap();

    let hello = receive(&mut socket, &mut noise).await;
    assert_eq!(hello["method"], wire::HELLO);
    let welcome = wire::Welcome {
        proto: wire::Proto {
            major: wire::PROTO_MAJOR,
            minor: wire::PROTO_MINOR,
        },
        caps: vec![],
        daemon: wire::DaemonInfo {
            version: "fixture".into(),
            store: "fixture".into(),
            epoch: 1,
        },
        health: wire::Health::down(),
    };
    reply(
        &mut socket,
        &mut noise,
        json!({"jsonrpc":"2.0", "id":hello["id"], "result":welcome}),
    )
    .await;
    let claim = receive(&mut socket, &mut noise).await;
    assert_eq!(claim["method"], PAIR_CLAIM);
    assert_eq!(claim["params"]["name"], "Windows fixture");
    reply(
        &mut socket,
        &mut noise,
        json!({"jsonrpc":"2.0", "id":claim["id"], "result":{"sas":expected_sas}}),
    )
    .await;
    reply(
        &mut socket,
        &mut noise,
        json!({"jsonrpc":"2.0", "method":PAIR_REFUSED, "params":{"reason":"cancelled"}}),
    )
    .await;
    expected_sas
}

async fn binary(socket: &mut WebSocketStream<TcpStream>) -> socket::MessageBytes {
    match socket.next().await.unwrap().unwrap() {
        Message::Binary(bytes) => bytes,
        other => panic!("expected a binary WebSocket message, got {other:?}"),
    }
}

async fn receive(socket: &mut WebSocketStream<TcpStream>, noise: &mut TransportState) -> Value {
    let bytes = binary(socket).await;
    let mut plain = vec![0; bytes.len()];
    let len = noise.read_message(&bytes, &mut plain).unwrap();
    let frame = decode_message(&plain[..len]).unwrap();
    assert_eq!((frame.kind, frame.stream), (FrameKind::Control, 0));
    serde_json::from_slice(&frame.payload).unwrap()
}

async fn reply(socket: &mut WebSocketStream<TcpStream>, noise: &mut TransportState, value: Value) {
    let plain = encode_message(&Frame {
        kind: FrameKind::Control,
        stream: 0,
        payload: serde_json::to_vec(&value).unwrap(),
    })
    .unwrap();
    let mut cipher = vec![0; plain.len() + NOISE_TAG];
    let len = noise.write_message(&plain, &mut cipher).unwrap();
    cipher.truncate(len);
    socket.send(Message::binary(cipher)).await.unwrap();
}
