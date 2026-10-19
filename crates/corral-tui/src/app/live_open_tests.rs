//! HTML artifacts open through the live artifact server from every board surface, by key and by click.

use std::net::UdpSocket;
use std::sync::mpsc;
use std::time::Duration;

use corral_core::ArtifactKind;
use ratatui::crossterm::event::KeyCode;

use super::App;
use super::mouse_tests::{click, find_any, render};
use crate::artifacts::panels_fixture::{Fixture, fixture};
use crate::board::tests::{code, key};
use crate::{Action, Mode};

/// The panels fixture plus `dash.html` on run R1, a live server for its store and a `[viewers] url` that
/// launches `true <url>`. A private socket records Markdown opens without reaching the desktop.
struct Live {
    fx: Fixture,
    done: mpsc::Receiver<anyhow::Result<()>>,
    opened: UdpSocket,
}

impl Live {
    fn new() -> Live {
        let mut fx = fixture();
        fx.ctx
            .store
            .config
            .viewers
            .insert("url".into(), "true {url}".into());
        let opened = UdpSocket::bind("127.0.0.1:0").unwrap();
        opened
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        fx.ctx.store.config.viewers.insert(
            "md".into(),
            format!(
                "python3 -c 'import socket,sys; socket.socket(socket.AF_INET,socket.SOCK_DGRAM).sendto(sys.argv[1].encode(),(\"127.0.0.1\",{}))' {{path}}",
                opened.local_addr().unwrap().port()
            ),
        );
        let page = fx.root.join("dash.html");
        std::fs::write(&page, "<h1>dash</h1>\n").unwrap();
        corral_ops::add_artifact(
            &fx.ctx,
            &fx.id,
            &page,
            Some(ArtifactKind::Evidence),
            Some("Dashboard page"),
        )
        .unwrap();
        let (ready_tx, ready) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let ctx = fx.ctx.clone();
        std::thread::spawn(move || {
            let result = corral_ops::run_server(ctx, corral_ops::DEFAULT_IDLE, |_| {
                ready_tx.send(())?;
                Ok(())
            });
            let _ = done_tx.send(result);
        });
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        Live { fx, done, opened }
    }

    fn app(&self) -> App {
        App::new(self.fx.ctx.clone(), Mode::Board).unwrap()
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        if corral_ops::stop_server(&self.fx.ctx).unwrap_or(false) {
            let _ = self.done.recv_timeout(Duration::from_secs(10));
        }
    }
}

fn assert_served(app: &mut App, how: &str) {
    let text = render(app);
    assert!(
        text.contains("Serving live"),
        "{how}: no live toast\n{text}"
    );
    assert!(
        text.contains("http://127.0.0.1:"),
        "{how}: the toast names the URL\n{text}"
    );
}

#[test]
fn finder_enter_and_click_serve_html_live() {
    let live = Live::new();
    let mut app = live.app();
    app.handle_key(key('A'));
    for c in "dash".chars() {
        app.handle_key(key(c));
    }
    app.handle_key(code(KeyCode::Enter));
    assert_served(&mut app, "finder ⏎");

    let mut app = live.app();
    app.handle_key(key('A'));
    for c in "dash".chars() {
        app.handle_key(key(c));
    }
    let text = render(&mut app);
    let (x, y) = find_any(&text, "Dashboard page");
    click(&mut app, x, y);
    click(&mut app, x, y);
    assert_served(&mut app, "finder click");
}

#[test]
fn detail_click_enter_and_o_serve_html_and_still_preview_markdown() {
    let live = Live::new();
    for how in ["click", "enter", "o"] {
        let mut app = live.app();
        app.apply(Action::OpenDetail(live.fx.id.clone()));
        app.handle_key(key('1'));
        let text = render(&mut app);
        let (x, y) = find_any(&text, "3 Artifacts");
        click(&mut app, x, y);
        let text = render(&mut app);
        let (x, y) = find_any(&text, "Dashboard page");
        click(&mut app, x, y);
        match how {
            "click" => click(&mut app, x, y),
            "enter" => app.handle_key(code(KeyCode::Enter)),
            _ => app.handle_key(key('o')),
        }
        assert_served(&mut app, &format!("detail {how}"));
    }

    let mut app = live.app();
    app.apply(Action::OpenDetail(live.fx.id.clone()));
    app.handle_key(key('1'));
    let text = render(&mut app);
    let (x, y) = find_any(&text, "3 Artifacts");
    click(&mut app, x, y);
    let text = render(&mut app);
    let (x, y) = find_any(&text, "QA checklist");
    click(&mut app, x, y);
    click(&mut app, x, y);
    let text = render(&mut app);
    assert!(
        text.contains("all green") && !text.contains("Serving live"),
        "markdown keeps its preview\n{text}"
    );
    app.handle_key(key('o'));
    assert!(
        render(&mut app).contains("Opened"),
        "o in the preview opens it"
    );
    let task = live.fx.ctx.store.get(&live.fx.id).unwrap();
    let mut path = [0; 4096];
    let (len, _) = live
        .opened
        .recv_from(&mut path)
        .expect("the private Markdown launcher must record the opened snapshot");
    assert_eq!(
        &path[..len],
        corral_ops::artifact_path(&live.fx.ctx, &task.artifacts[0])
            .unwrap()
            .to_str()
            .unwrap()
            .as_bytes(),
        "the configured Markdown launcher must receive the previewed snapshot"
    );
}

#[test]
fn review_click_enter_and_o_serve_html_live() {
    let live = Live::new();
    for how in ["click", "enter", "o"] {
        let mut app = live.app();
        app.apply(Action::OpenReview(live.fx.id.clone()));
        app.handle_key(code(KeyCode::Esc));
        let text = render(&mut app);
        let (x, y) = find_any(&text, "Dashboard page");
        click(&mut app, x, y);
        match how {
            "click" => click(&mut app, x, y),
            "enter" => app.handle_key(code(KeyCode::Enter)),
            _ => app.handle_key(key('o')),
        }
        assert_served(&mut app, &format!("review {how}"));
    }
}

#[test]
fn shelf_number_key_serves_a_pinned_html_artifact_live() {
    let live = Live::new();
    live.fx
        .ctx
        .store
        .pin(&format!("{}:R1:dash.html", live.fx.id), None, false)
        .unwrap();
    let mut app = live.app();
    render(&mut app);
    app.handle_key(key('1'));
    assert_served(&mut app, "shelf 1");
}
