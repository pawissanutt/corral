use super::*;

const OK: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n";

#[test]
fn recognizes_single_closed_response_with_bounded_framing() {
    for (headers, wire) in [
        ("Content-Length: 3\r\n", "abc"),
        ("Connection: close\r\n", "abc"),
        (
            "Transfer-Encoding: Chunked\r\n",
            "1\r\na\r\n2\r\nbc\r\n0\r\n\r\n",
        ),
    ] {
        assert_eq!(
            body(format!("{OK}{headers}\r\n{wire}").as_bytes())
                .unwrap()
                .as_ref(),
            b"abc"
        );
    }
}

#[test]
fn refuses_conflicting_partial_and_trailing_http() {
    for (headers, wire) in [
        ("Content-Length: 2\r\n", "abc"),
        ("Content-Length: 4\r\n", "abc"),
        ("Content-Length: 3\r\ncontent-length: 3\r\n", "abc"),
        ("Content-Length: 3\r\nTransfer-Encoding: chunked\r\n", "abc"),
        ("Content-Length: +3\r\n", "abc"),
        ("Content-Length: 3\n", "abc"),
        ("Content-Encoding: gzip\r\n", "abc"),
        ("Transfer-Encoding: chunked\r\n", "1\r\na\r\n0\r\n\r\nextra"),
        ("Transfer-Encoding: chunked\r\n", "3\r\na\r\n0\r\n\r\n"),
        (
            "Transfer-Encoding: chunked\r\n",
            "1\r\na\r\n0\r\nX: 1\r\n\r\n",
        ),
        (
            "Transfer-Encoding: chunked\r\n",
            "1;hidden=x\r\na\r\n0\r\n\r\n",
        ),
    ] {
        assert!(
            body(format!("{OK}{headers}\r\n{wire}").as_bytes()).is_err(),
            "accepted framing {headers:?} {wire:?}"
        );
    }
    assert!(body(b"HTTP/1.1 302 Found\r\nContent-Type: text/event-stream\r\n\r\n").is_err());
    assert!(body(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n").is_err());
    assert!(body(b"\r\nHTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n").is_err());
    assert!(body(b"HTTP/1.1 200 OK\r\nX: ok\r\n Content-Type: text/event-stream\r\n\r\n").is_err());
}
