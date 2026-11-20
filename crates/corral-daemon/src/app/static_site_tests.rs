use std::fs;
use std::os::unix::fs::symlink;

use super::serve;

#[test]
fn only_the_deployment_root_may_be_a_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = tmp.path().join("bundle");
    let outside = tmp.path().join("outside");
    let current = tmp.path().join("current");
    fs::create_dir(&bundle).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(bundle.join("index.html"), b"app shell").unwrap();
    fs::write(outside.join("secret"), b"not in the bundle").unwrap();
    symlink(&bundle, &current).unwrap();
    symlink(outside.join("secret"), bundle.join("file-link")).unwrap();
    symlink(&outside, bundle.join("dir-link")).unwrap();

    for (path, status, body) in [
        ("/", "200", "app shell"),
        ("/file-link", "404", ""),
        ("/dir-link/secret", "404", ""),
    ] {
        let raw = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nAccept: text/html\r\n\r\n");
        let mut headers = [httparse::EMPTY_HEADER; 2];
        let mut request = httparse::Request::new(&mut headers);
        assert!(request.parse(raw.as_bytes()).unwrap().is_complete());
        let mut response = Vec::new();
        serve(&mut response, &current, &request).unwrap();
        let response = std::str::from_utf8(&response).unwrap();
        let (head, actual_body) = response.split_once("\r\n\r\n").unwrap();
        assert!(
            head.starts_with(&format!("HTTP/1.1 {status} ")),
            "{path}: {head}"
        );
        assert_eq!(actual_body, body, "{path}");
    }
}
