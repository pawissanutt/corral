//! One HTTP/1.1 connection: GET/HEAD only, `Connection: close`, the token as the first path segment (so
//! relative asset URLs carry it), a loopback `Host` (no DNS rebinding) and responses read only from recorded
//! artifact snapshots.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, Result};
use corral_core::Store;

use super::Shared;
use super::pages;

const MAX_HEAD: usize = 16 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const EVENT_TICK: Duration = Duration::from_millis(500);
const PINGS_EVERY: u32 = 30;

struct Response {
    status: &'static str,
    ctype: &'static str,
    body: Vec<u8>,
    location: Option<String>,
}

impl Response {
    fn ok(ctype: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Response {
            status: "200 OK",
            ctype,
            body: body.into(),
            location: None,
        }
    }

    fn error(status: &'static str) -> Self {
        Response {
            status,
            ctype: "text/plain; charset=utf-8",
            body: format!("{status}\n").into_bytes(),
            location: None,
        }
    }

    fn redirect(to: String) -> Self {
        Response {
            status: "302 Found",
            ctype: "text/plain; charset=utf-8",
            body: Vec::new(),
            location: Some(to),
        }
    }
}

pub(super) fn handle(shared: &Shared, mut stream: TcpStream) -> Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let Some(head) = read_head(&mut stream)? else {
        return write(
            &mut stream,
            &Response::error("431 Request Header Fields Too Large"),
            false,
        );
    };
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let (method, target) = (
        first.next().unwrap_or_default(),
        first.next().unwrap_or_default(),
    );
    let head_only = method == "HEAD";
    if method != "GET" && !head_only {
        return write(
            &mut stream,
            &Response::error("405 Method Not Allowed"),
            false,
        );
    }
    let host = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.trim().to_ascii_lowercase());
    let port = shared.info.port;
    if !host.is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}")) {
        return write(&mut stream, &Response::error("403 Forbidden"), head_only);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let Some(segs) = path.strip_prefix('/').and_then(|p| {
        p.split('/')
            .map(pages::decode_segment)
            .collect::<Option<Vec<String>>>()
    }) else {
        return write(&mut stream, &Response::error("400 Bad Request"), head_only);
    };
    if !segs.first().is_some_and(|t| same(t, &shared.info.token)) {
        return write(&mut stream, &Response::error("404 Not Found"), head_only);
    }
    if segs
        .iter()
        .any(|s| s == "." || s == ".." || s.contains(['/', '\\', '\0']))
    {
        return write(&mut stream, &Response::error("400 Bad Request"), head_only);
    }
    let rest: Vec<&str> = segs[1..].iter().map(String::as_str).collect();
    if rest == ["events"] {
        return events(shared, stream, query);
    }
    let response = route(shared, &rest, query)
        .unwrap_or_else(|_| Response::error("500 Internal Server Error"));
    write(&mut stream, &response, head_only)?;
    if rest == ["shutdown"] {
        shared.shut_down();
    }
    Ok(())
}

/// Constant-time token comparison.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn read_head(stream: &mut TcpStream) -> Result<Option<String>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        if buf.len() > MAX_HEAD {
            return Ok(None);
        }
        let n = stream.read(&mut chunk).context("reading request")?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

fn write(stream: &mut TcpStream, r: &Response, head_only: bool) -> Result<()> {
    let mut head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n",
        r.status,
        r.ctype,
        r.body.len()
    );
    if let Some(to) = &r.location {
        head.push_str(&format!("Location: {to}\r\n"));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .context("writing response")?;
    if !head_only {
        stream.write_all(&r.body).context("writing response body")?;
    }
    stream.flush().context("flushing response")
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('=').or(Some((kv, ""))))
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| pages::decode_query(v))
}

fn route(shared: &Shared, rest: &[&str], query: &str) -> Result<Response> {
    let token = &shared.info.token;
    let ctx = &shared.ctx;
    Ok(match rest {
        [] => Response::redirect(format!("/{token}/")),
        [""] => index(shared, None)?,
        ["health"] => Response::ok(
            "text/plain; charset=utf-8",
            format!("corral-artifact-server\n{}\n", ctx.store.dir().display()),
        ),
        ["shutdown"] => Response::ok("text/plain; charset=utf-8", "stopping\n"),
        ["t", task] => Response::redirect(format!("/{token}/t/{}/", pages::encode(task))),
        ["t", task, ""] => match Store::normalize_id(task) {
            Ok(id) => index(shared, Some(&id))?,
            Err(_) => Response::error("404 Not Found"),
        },
        ["a", task, run, rel @ ..] if !rel.is_empty() && !rel.contains(&"") => artifact(
            shared,
            task,
            run,
            &rel.join("/"),
            query_param(query, "raw").is_some(),
        )?,
        _ => Response::error("404 Not Found"),
    })
}

fn index(shared: &Shared, task: Option<&str>) -> Result<Response> {
    let token = &shared.info.token;
    let rows = pages::index_rows(&shared.ctx, task)?;
    let (watch, label) = match task {
        Some(t) => (format!("t/{t}"), format!("{t} \u{b7} documents")),
        None => ("index".to_string(), "corral \u{b7} documents".to_string()),
    };
    let header = pages::live_header(token, &label, "", &watch, &pages::fingerprint(&rows));
    let html = pages::inject(&pages::index_page(token, task, &rows), &header);
    Ok(Response::ok("text/html; charset=utf-8", html))
}

fn artifact(shared: &Shared, task: &str, run: &str, rel: &str, raw: bool) -> Result<Response> {
    let ctx = &shared.ctx;
    let Ok(id) = Store::normalize_id(task) else {
        return Ok(Response::error("404 Not Found"));
    };
    let Ok(t) = ctx.store.get(&id) else {
        return Ok(Response::error("404 Not Found"));
    };
    let Some(a) = pages::resolve(&t, run, rel) else {
        return Ok(Response::error("404 Not Found"));
    };
    let Ok(bytes) = crate::artifact_storage::artifact_read_confined(ctx, a) else {
        return Ok(Response::error("404 Not Found"));
    };
    if a.purged_at.is_some() {
        return Ok(Response::ok("text/plain; charset=utf-8", bytes));
    }
    let ext = pages::ext(a);
    let document = matches!(ext.as_str(), "html" | "htm")
        || (!raw && matches!(ext.as_str(), "md" | "markdown"));
    if !document {
        return Ok(Response::ok(pages::content_type(&ext), bytes));
    }
    let token = &shared.info.token;
    let watch = pages::artifact_url_path(a).unwrap_or_default();
    let label = format!(
        "{id} \u{b7} {} \u{b7} updated {}",
        a.run,
        pages::hhmm(a.added)
    );
    let header = pages::live_header(
        token,
        &label,
        &format!("t/{}/", pages::encode(&id)),
        &watch,
        &a.sha256,
    );
    let text = String::from_utf8_lossy(&bytes);
    let html = if matches!(ext.as_str(), "md" | "markdown") {
        pages::page(&a.title, &pages::markdown_html(&text))
    } else {
        text.into_owned()
    };
    Ok(Response::ok(
        "text/html; charset=utf-8",
        pages::inject(&html, &header),
    ))
}

/// What a page's event stream follows now: the URL to load and its version, None when it is gone.
fn current(shared: &Shared, watch: &str) -> Option<(String, String)> {
    let token = &shared.info.token;
    let ctx = &shared.ctx;
    if watch == "index" {
        let rows = pages::index_rows(ctx, None).ok()?;
        return Some((format!("/{token}/"), pages::fingerprint(&rows)));
    }
    if let Some(task) = watch.strip_prefix("t/") {
        let rows = pages::index_rows(ctx, Some(task)).ok()?;
        return Some((
            format!("/{token}/t/{}/", pages::encode(task)),
            pages::fingerprint(&rows),
        ));
    }
    let segs: Vec<String> = watch
        .strip_prefix("a/")?
        .split('/')
        .map(pages::decode_segment)
        .collect::<Option<_>>()?;
    let [task, run, rel @ ..] = segs.as_slice() else {
        return None;
    };
    let t = ctx.store.get(task).ok()?;
    let served = pages::resolve(&t, run, &rel.join("/"))?;
    let latest = pages::latest(&t, served).unwrap_or(served);
    Some((
        format!("/{token}/{}", pages::artifact_url_path(latest)?),
        latest.sha256.clone(),
    ))
}

/// Server-sent events: `reload` (data: the URL to load) once what the page shows has a newer version. Open
/// streams keep the server alive; each ends when the client goes away or the server stops.
fn events(shared: &Shared, mut stream: TcpStream, query: &str) -> Result<()> {
    let watch = query_param(query, "watch").unwrap_or_default();
    let version = query_param(query, "v").unwrap_or_default();
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: close\r\n\r\nretry: 2000\n\n",
    )?;
    stream.flush()?;
    shared.streams.fetch_add(1, Ordering::SeqCst);
    let result = follow(shared, &mut stream, &watch, &version);
    shared.streams.fetch_sub(1, Ordering::SeqCst);
    shared.touch();
    result
}

fn follow(shared: &Shared, stream: &mut TcpStream, watch: &str, version: &str) -> Result<()> {
    let mut ticks = 0u32;
    while !shared.stopping() {
        if let Some((url, now)) = current(shared, watch)
            && now != version
        {
            stream.write_all(format!("event: reload\ndata: {url}\n\n").as_bytes())?;
            return stream.flush().context("sending reload");
        }
        ticks += 1;
        if ticks.is_multiple_of(PINGS_EVERY) {
            stream.write_all(b": ping\n\n")?;
            stream.flush()?;
        }
        std::thread::sleep(EVENT_TICK);
    }
    Ok(())
}
