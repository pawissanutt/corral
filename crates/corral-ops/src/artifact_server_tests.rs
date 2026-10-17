use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use corral_core::{Artifact, ArtifactKind, Status, Store};

use super::pages::{artifact_url_path, markdown_html};
use super::*;

const WAIT: Duration = Duration::from_secs(10);

struct Server {
    ctx: Ctx,
    info: ServerInfo,
    done: mpsc::Receiver<Result<()>>,
    _root: tempfile::TempDir,
}

fn start(idle: Duration) -> Server {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let (info_tx, info_rx) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let server_ctx = ctx.clone();
    std::thread::spawn(move || {
        let result = run_server(server_ctx, idle, |info| {
            info_tx.send(info.clone())?;
            Ok(())
        });
        let _ = done_tx.send(result);
    });
    let info = info_rx.recv_timeout(WAIT).unwrap();
    Server {
        ctx,
        info,
        done,
        _root: root,
    }
}

fn add(s: &Server, task: &str, path: &Path, body: &[u8], kind: Option<ArtifactKind>) -> Artifact {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    crate::add_artifact(&s.ctx, task, path, kind, None).unwrap()
}

fn raw(s: &Server, request: &str) -> (String, String) {
    let mut c = TcpStream::connect(("127.0.0.1", s.info.port)).unwrap();
    c.set_read_timeout(Some(WAIT)).unwrap();
    c.write_all(request.as_bytes()).unwrap();
    let mut out = String::new();
    c.read_to_string(&mut out).unwrap();
    let (status, rest) = out.split_once("\r\n").unwrap_or((&out, ""));
    (status.to_string(), rest.to_string())
}

fn get(s: &Server, path: &str) -> (String, String) {
    raw(
        s,
        &format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            s.info.port
        ),
    )
}

fn url(s: &Server, a: &Artifact) -> String {
    format!("/{}/{}", s.info.token, artifact_url_path(a).unwrap())
}

type Events = std::io::Lines<BufReader<TcpStream>>;

fn attr(body: &str, name: &str) -> String {
    let start = body.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
    body[start..][..body[start..].find('"').unwrap()].to_string()
}

/// Loads `page` and opens its event stream exactly as the injected script does (its data-watch and data-v).
fn subscribe(s: &Server, page: &str) -> Events {
    let (status, body) = get(s, page);
    assert_eq!(status, "HTTP/1.1 200 OK", "{page}");
    let mut c = TcpStream::connect(("127.0.0.1", s.info.port)).unwrap();
    c.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        c,
        "GET /{}/events?watch={}&v={} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        s.info.token,
        super::pages::encode(&attr(&body, "data-watch")),
        attr(&body, "data-v"),
        s.info.port
    )
    .unwrap();
    BufReader::new(c).lines()
}

/// The stream's next verdict: `Some(url)` for a reload, None for the `: live` sent when the page is current.
fn verdict(events: &mut Events) -> Option<String> {
    let line = events
        .by_ref()
        .map(Result::unwrap)
        .find(|l| l == "event: reload" || l == ": live")
        .unwrap();
    (line == "event: reload").then(|| {
        let data = events.next().unwrap().unwrap();
        data.strip_prefix("data: ").unwrap().to_string()
    })
}

/// Rewrites the run of the task's newest artifact (what a later run's recording looks like here).
fn set_run(s: &Server, task: &str, run: &str) -> Artifact {
    s.ctx
        .store
        .update(task, |t| {
            if let Some(a) = t.artifacts.last_mut() {
                a.run = run.to_string();
            }
            Ok(())
        })
        .unwrap();
    s.ctx
        .store
        .get(task)
        .unwrap()
        .artifacts
        .last()
        .unwrap()
        .clone()
}

#[test]
fn html_is_served_live_with_its_relative_assets_from_snapshots() {
    // Given a report and the assets next to and above it, recorded as artifacts.
    let s = start(DEFAULT_IDLE);
    let task = s
        .ctx
        .store
        .create("Report", None, Status::Done, "")
        .unwrap();
    let src = s._root.path().join("work/out");
    let report = add(
        &s,
        &task.id,
        &src.join("report.html"),
        b"<html><body class=x><img src=\"img/pic.png\"><link href=\"../style.css\"></body></html>",
        Some(ArtifactKind::Report),
    );
    add(&s, &task.id, &src.join("img/pic.png"), b"PNGDATA", None);
    add(
        &s,
        &task.id,
        &s._root.path().join("work/style.css"),
        b"body{}",
        None,
    );
    assert!(s.info.base_url().starts_with("http://127.0.0.1:"));

    // When the page and the URLs its relative links resolve to are requested.
    let page = url(&s, &report);
    let (status, body) = get(&s, &page);
    let dir = page.rsplit_once('/').unwrap().0;
    let (img_status, img) = get(&s, &format!("{dir}/img/pic.png"));
    let parent = dir.rsplit_once('/').unwrap().0;
    let (css_status, css) = get(&s, &format!("{parent}/style.css"));

    // Then the document carries the live header after <body> and the assets come from their snapshots.
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(body.contains("Content-Type: text/html"));
    assert!(body.contains("Referrer-Policy: no-referrer"));
    assert!(
        body.contains("<body class=x><div id=\"corral-live\""),
        "{body}"
    );
    assert!(body.contains(&format!("{} \u{b7} R0 \u{b7} updated ", task.id)));
    assert!(body.contains("<img src=\"img/pic.png\">"));
    assert_eq!(img_status, "HTTP/1.1 200 OK");
    assert!(img.contains("Content-Type: image/png") && img.ends_with("PNGDATA"));
    assert_eq!(css_status, "HTTP/1.1 200 OK");
    assert!(css.contains("Content-Type: text/css") && css.ends_with("body{}"));
}

#[test]
fn requests_outside_recorded_artifacts_or_without_the_token_are_refused() {
    // Given a recorded artifact and an unrecorded file in its run directory.
    let s = start(DEFAULT_IDLE);
    let task = s.ctx.store.create("Guard", None, Status::Done, "").unwrap();
    let a = add(
        &s,
        &task.id,
        &s._root.path().join("r.html"),
        b"<p>hi</p>",
        None,
    );
    let run_dir = s.ctx.store.artifacts_dir(&task.id, "R0");
    std::fs::write(run_dir.join("secret.txt"), b"nope").unwrap();
    let tok = &s.info.token;
    let good = artifact_url_path(&a).unwrap();
    let port = s.info.port;

    // When requests miss the token, escape, name unrecorded files or come through another host name.
    let cases = [
        (format!("/{good}"), "404"),
        (format!("/{}x/{good}", &tok[..31]), "404"),
        (format!("/{tok}/a/{}/R0/secret.txt", task.id), "404"),
        (format!("/{tok}/a/{}/R0/etc/passwd", task.id), "404"),
        (
            format!("/{tok}/a/{}/R0/%2E%2E/%2E%2E/tasks", task.id),
            "400",
        ),
        (format!("/{tok}/a/{}/R0/..%2Fstate", task.id), "400"),
        (format!("/{tok}/a/T-999/R0/r.html"), "404"),
    ];
    // Then each one is refused without content.
    for (path, want) in cases {
        let (status, body) = get(&s, &path);
        assert!(status.contains(want), "{path}: {status}");
        assert!(
            !body.contains("nope") && !body.contains("<p>hi</p>"),
            "{path}"
        );
    }
    let (status, _) = raw(
        &s,
        &format!("GET /{tok}/{good} HTTP/1.1\r\nHost: evil.example:{port}\r\n\r\n"),
    );
    assert!(status.contains("403"), "{status}");
    let (status, _) = raw(
        &s,
        &format!("POST /{tok}/{good} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
    );
    assert!(status.contains("405"), "{status}");
    assert_eq!(get(&s, &format!("/{tok}/{good}")).0, "HTTP/1.1 200 OK");
}

#[test]
fn an_open_page_is_told_to_reload_when_its_artifact_is_updated() {
    // Given an open event stream for a served report.
    let s = start(DEFAULT_IDLE);
    let task = s.ctx.store.create("Live", None, Status::Done, "").unwrap();
    let path = s._root.path().join("live/report.html");
    let first = add(&s, &task.id, &path, b"<body>v1</body>", None);
    let mut events = subscribe(&s, &url(&s, &first));
    assert_eq!(verdict(&mut events), None, "a current page is not reloaded");

    // When a later version of the same file is recorded.
    let second = add(&s, &task.id, &path, b"<body>v2</body>", None);

    // Then the stream sends a reload naming the page, which now serves the new version.
    assert_eq!(verdict(&mut events), Some(url(&s, &second)));
    assert!(get(&s, &url(&s, &second)).1.contains("v2"));
}

#[test]
fn the_index_lists_documents_importance_first_and_renders_markdown_safely() {
    // Given a normal-tier HTML page and a keep-tier Markdown report with raw HTML in it.
    let s = start(DEFAULT_IDLE);
    let task = s.ctx.store.create("Docs", None, Status::Done, "").unwrap();
    let root = s._root.path();
    add(
        &s,
        &task.id,
        &root.join("notes.html"),
        b"<p>n</p>",
        Some(ArtifactKind::Evidence),
    );
    let md = add(
        &s,
        &task.id,
        &root.join("summary.md"),
        b"---\nk: v\n---\n# Title\n<script>alert(1)</script>\n[x](javascript:alert(1))\n",
        Some(ArtifactKind::Report),
    );
    add(&s, &task.id, &root.join("run.log"), b"log", None);
    add(
        &s,
        &task.id,
        &root.join("notes.html"),
        b"<p>n2</p>",
        Some(ArtifactKind::Evidence),
    );

    // When the task index and the Markdown page are requested.
    let (_, index) = get(&s, &format!("/{}/t/{}/", s.info.token, task.id));
    let (_, page) = get(&s, &url(&s, &md));
    let (_, raw_md) = get(&s, &format!("{}?raw", url(&s, &md)));

    // Then keep comes before normal, logs are not listed and the Markdown renders without active content.
    let keep = index.find("summary.md").unwrap();
    let normal = index.find("notes.html").unwrap();
    assert!(keep < normal, "{index}");
    assert!(!index.contains("run.log"));
    assert_eq!(
        index.matches(">notes.html<").count(),
        1,
        "one row per document"
    );
    assert!(page.contains("<h1>Title</h1>"));
    assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(page.contains("<a href=\"#\""));
    assert!(!page.contains("k: v"));
    assert!(raw_md.contains("text/plain") && raw_md.contains("<script>alert(1)</script>"));
}

#[test]
fn markdown_images_keep_their_alt_text_as_an_attribute() {
    let html = markdown_html("![a \"pic\"](img/p.png)");
    assert_eq!(
        html,
        "<p><img src=\"img/p.png\" alt=\"a &quot;pic&quot;\"></p>\n"
    );
}

#[test]
fn the_server_stops_when_idle_and_removes_its_state() {
    // Given a server with a short idle timeout and no clients.
    let s = start(Duration::from_millis(100));
    assert!(state_path(&s.ctx).is_file());

    // When nothing connects, then it exits on its own and its state file is gone.
    s.done.recv_timeout(WAIT).unwrap().unwrap();
    assert!(!state_path(&s.ctx).exists());
}

#[test]
fn the_running_server_is_found_reused_and_stopped_through_its_state_file() {
    // Given a running server.
    let s = start(DEFAULT_IDLE);

    // When the workspace looks it up and stops it.
    assert_eq!(running_server(&s.ctx), Some(s.info.clone()));
    assert_eq!(ensure_server(&s.ctx).unwrap(), s.info);
    assert!(stop_server(&s.ctx).unwrap());

    // Then the server exits and nothing is running any more.
    s.done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(running_server(&s.ctx), None);
    assert!(!stop_server(&s.ctx).unwrap());
}

#[test]
fn symlinked_snapshots_and_run_directories_never_leak_files_outside_the_store() {
    // Given one snapshot replaced by a symlink to an outside file, a run directory replaced by a symlink to an
    // outside directory, and a compressed snapshot.
    let s = start(DEFAULT_IDLE);
    let task = s
        .ctx
        .store
        .create("Escape", None, Status::Done, "")
        .unwrap();
    let root = s._root.path();
    std::fs::write(root.join("outside.txt"), "OUTSIDE_MARKER").unwrap();
    let file = add(
        &s,
        &task.id,
        &root.join("a/file.html"),
        b"<p>file</p>",
        None,
    );
    let snapshot = s.ctx.store.dir().join(&file.path);
    std::fs::remove_file(&snapshot).unwrap();
    std::os::unix::fs::symlink(root.join("outside.txt"), &snapshot).unwrap();
    let other = s
        .ctx
        .store
        .create("Escape dir", None, Status::Done, "")
        .unwrap();
    let dir_art = add(&s, &other.id, &root.join("b/dir.html"), b"<p>dir</p>", None);
    let run_dir = s.ctx.store.artifacts_dir(&other.id, "R0");
    let outside_dir = root.join("outside-run");
    std::fs::rename(&run_dir, &outside_dir).unwrap();
    std::os::unix::fs::symlink(&outside_dir, &run_dir).unwrap();
    let packed = s
        .ctx
        .store
        .create("Packed", None, Status::Done, "")
        .unwrap();
    let gz = add(
        &s,
        &packed.id,
        &root.join("c/packed.html"),
        b"<p>packed body</p>",
        None,
    );
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(b"<p>packed body</p>").unwrap();
    std::fs::write(s.ctx.store.dir().join(&gz.path), enc.finish().unwrap()).unwrap();
    s.ctx
        .store
        .update(&packed.id, |t| {
            t.artifacts[0].compressed_bytes = Some(1);
            Ok(())
        })
        .unwrap();

    // When each one is requested.
    let (file_status, file_body) = get(&s, &url(&s, &file));
    let (dir_status, dir_body) = get(&s, &url(&s, &dir_art));
    let (gz_status, gz_body) = get(&s, &url(&s, &gz));

    // Then the symlinks are refused and the compressed snapshot is decoded and served.
    assert!(file_status.contains("404"), "{file_status}");
    assert!(!file_body.contains("OUTSIDE_MARKER"));
    assert!(dir_status.contains("404"), "{dir_status}");
    assert!(!dir_body.contains("<p>dir</p>"));
    assert_eq!(gz_status, "HTTP/1.1 200 OK");
    assert!(gz_body.contains("<p>packed body</p>"), "{gz_body}");
}

#[test]
fn asset_paths_resolve_exactly_with_literal_plus_and_no_basename_guessing() {
    // Given a page with a plus-named sibling and two same-named stylesheets in different directories.
    let s = start(DEFAULT_IDLE);
    let task = s.ctx.store.create("Paths", None, Status::Done, "").unwrap();
    let out = s._root.path().join("out");
    let page = add(
        &s,
        &task.id,
        &out.join("report.html"),
        b"<body>r</body>",
        None,
    );
    add(
        &s,
        &task.id,
        &out.join("release+notes.css"),
        b"PLUS_CSS",
        None,
    );
    add(&s, &task.id, &out.join("a/style.css"), b"A_CSS", None);
    add(&s, &task.id, &out.join("b/style.css"), b"B_CSS", None);
    let dir = url(&s, &page).rsplit_once('/').unwrap().0.to_string();

    // When the browser asks for the relative URLs it would build.
    let plus = get(&s, &format!("{dir}/release+notes.css"));
    let a = get(&s, &format!("{dir}/a/style.css"));
    let b = get(&s, &format!("{dir}/b/style.css"));
    let missing = get(&s, &format!("{dir}/missing/style.css"));

    // Then each path gets exactly its own file and a missing one is a 404.
    assert!(
        plus.0.contains("200") && plus.1.ends_with("PLUS_CSS"),
        "{plus:?}"
    );
    assert!(a.1.ends_with("A_CSS") && b.1.ends_with("B_CSS"));
    assert!(missing.0.contains("404"), "{missing:?}");
    assert!(!missing.1.contains("_CSS"));
}

#[test]
fn an_open_page_moves_to_the_later_run_that_updates_its_document() {
    // Given a page open on run R1's report.
    let s = start(DEFAULT_IDLE);
    let task = s.ctx.store.create("Runs", None, Status::Done, "").unwrap();
    let path = s._root.path().join("w/report.html");
    add(&s, &task.id, &path, b"<body>R1 body</body>", None);
    let first = set_run(&s, &task.id, "R1");
    let mut events = subscribe(&s, &url(&s, &first));
    assert_eq!(verdict(&mut events), None);

    // When run R2 records a new version of the same file.
    add(&s, &task.id, &path, b"<body>R2 body</body>", None);
    let second = set_run(&s, &task.id, "R2");

    // Then the page is sent to R2's URL, which serves the new body.
    let data = verdict(&mut events).unwrap();
    assert!(data.contains("/a/") && data.contains("/R2/"), "{data}");
    assert_eq!(data, url(&s, &second));
    assert!(get(&s, &url(&s, &second)).1.contains("R2 body"));
}

#[test]
fn an_open_page_reloads_when_its_assets_change_but_its_html_does_not() {
    // Given an open report whose stylesheet sits next to it.
    let s = start(DEFAULT_IDLE);
    let task = s
        .ctx
        .store
        .create("Assets", None, Status::Done, "")
        .unwrap();
    let dir = s._root.path().join("gen");
    let html: &[u8] = b"<html><body><link rel=stylesheet href=\"style.css\"></body></html>";
    let report = add(&s, &task.id, &dir.join("report.html"), html, None);
    add(
        &s,
        &task.id,
        &dir.join("style.css"),
        b"body{color:red}",
        None,
    );
    let page = url(&s, &report);
    let mut events = subscribe(&s, &page);
    assert_eq!(verdict(&mut events), None, "a current page is not reloaded");

    // When the stylesheet changes and the byte-identical HTML is re-added.
    add(
        &s,
        &task.id,
        &dir.join("style.css"),
        b"body{color:blue}",
        None,
    );
    let again = add(&s, &task.id, &dir.join("report.html"), html, None);
    assert_eq!(
        again.path, report.path,
        "the HTML snapshot itself is unchanged"
    );

    // Then the page reloads in place and loads the new stylesheet.
    assert_eq!(verdict(&mut events), Some(page.clone()));
    let css = format!("{}/style.css", page.rsplit_once('/').unwrap().0);
    assert!(get(&s, &css).1.ends_with("body{color:blue}"));

    // And the reloaded page is current: no reload loop.
    assert_eq!(verdict(&mut subscribe(&s, &page)), None);
}

#[test]
fn an_open_page_moves_to_a_later_run_that_re_adds_identical_html() {
    // Given a page open on run R1's report.
    let s = start(DEFAULT_IDLE);
    let task = s.ctx.store.create("Same", None, Status::Done, "").unwrap();
    let path = s._root.path().join("w/report.html");
    add(&s, &task.id, &path, b"<body>same body</body>", None);
    let first = set_run(&s, &task.id, "R1");
    let mut events = subscribe(&s, &url(&s, &first));
    assert_eq!(verdict(&mut events), None);

    // When run R2 records the byte-identical document.
    add(&s, &task.id, &path, b"<body>same body</body>", None);
    let second = set_run(&s, &task.id, "R2");
    assert_eq!(second.sha256, first.sha256);

    // Then the page moves to R2's URL, whose header names R2, and stays there.
    let target = verdict(&mut events).unwrap();
    assert_eq!(target, url(&s, &second));
    assert!(target.contains("/R2/"), "{target}");
    let (_, body) = get(&s, &target);
    assert!(
        body.contains(&format!("{} \u{b7} R2 \u{b7} updated ", task.id)),
        "{body}"
    );
    assert_eq!(verdict(&mut subscribe(&s, &target)), None);
}
