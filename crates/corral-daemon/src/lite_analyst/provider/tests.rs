use super::*;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

#[test]
fn first_send_includes_body_and_partial_send_continues_on_same_stream() {
    // A tiny send buffer forces partial progress without a TCP listener or live route.
    let (socket, peer) = net::socketpair(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )
    .unwrap();
    sockopt::set_socket_send_buffer_size(&socket, 4_096).unwrap();
    rustix::io::ioctl_fionbio(&socket, true).unwrap();
    let mut peer = UnixStream::from(peer);
    let timeout = Duration::from_secs(2);
    peer.set_read_timeout(Some(timeout)).unwrap();
    peer.set_write_timeout(Some(timeout)).unwrap();
    let bearer = [b'a'; 64];
    let mut provider = Provider {
        socket,
        credential: Some(bearer),
        request_sent: false,
        remainder: Vec::new(),
    };
    // Synthetic padding makes the request large enough to force a short send. The body
    // gate is independently tested; here we verify the transport never changes a byte.
    let body = format!("{{\"text\":\"{}\"}}", "x".repeat(65_536)).into_bytes();
    let len = body.len().to_string();
    let head = [PREFIX, &bearer, HEADERS, len.as_bytes(), END_HEADERS].concat();
    provider.send(&body).unwrap();
    let initial_count = head.len() + body.len() - provider.remainder.len();
    assert!(initial_count > head.len() && initial_count < head.len() + body.len());
    let mut initial = vec![0; initial_count];
    peer.read_exact(&mut initial).unwrap();
    assert_eq!(initial[..head.len()], head);
    assert_eq!(initial[head.len()..], body[..initial_count - head.len()]);
    assert!(provider.send(&body).is_err());

    let server = std::thread::spawn(move || {
        let mut rest = vec![0; head.len() + body.len() - initial_count];
        peer.read_exact(&mut rest).unwrap();
        assert_eq!(rest, body[initial_count - head.len()..]);
        peer.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        for (delta, reason) in [
            (r#"{"role":"assistant"}"#, "null"),
            (r#"{"content":"done"}"#, "null"),
            ("{}", r#""stop""#),
        ] {
            write!(peer, "data: {{\"id\":\"test\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"fixture-v1\",\"choices\":[{{\"index\":0,\"delta\":{delta},\"finish_reason\":{reason}}}]}}\n\n").unwrap();
        }
        peer.write_all(b"data: [DONE]\n\n").unwrap();
    });
    let response = provider
        .receive(Instant::now() + timeout, &mut || Ok(()))
        .unwrap();
    assert_eq!(
        response,
        serde_json::json!({"role":"assistant","content":"done"})
    );
    server.join().unwrap();
}
