//! Drives the live board paths once each and records their work: the worker's sync tick, the UI's
//! application of its snapshot, frames, keys, mouse input, and the detail, review and settings
//! screens.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use corral_ops::Ctx;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::work_probe::{Sample, measure};
use super::{App, WorkerMsg, sync_step};
use crate::{Action, Hit, Mode};

pub(crate) struct Row {
    pub(crate) path: String,
    pub(crate) work: Sample,
    pub(crate) elapsed: Duration,
}

const FRAMES: usize = 3;

fn pointer(kind: MouseEventKind, (column, row): (u16, u16)) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// With `CORRAL_WORK_BUDGET_MARKS` set, brackets a sync tick with a stat of a path that cannot
/// exist, so `strace -e trace=%file` of the harness can window its syscalls per tick and store.
fn mark(edge: &str, tick: &str, root: &str) {
    if std::env::var_os("CORRAL_WORK_BUDGET_MARKS").is_some() {
        let _ = std::fs::metadata(format!("/nonexistent/work-budget-mark/{edge}/{tick}{root}"));
    }
}

/// The background worker's loop body, without its 10-second herdr availability probe.
fn worker_tick(
    ctx: &mut Ctx,
    messages: &std::sync::mpsc::Sender<corral_ops::Message>,
) -> WorkerMsg {
    if let Err(error) = ctx.reload_config() {
        panic!("config reload failed: {error:#}");
    }
    WorkerMsg::synced_with_attention(ctx, messages)
}

pub(crate) struct Paths {
    pub(crate) rows: Vec<Row>,
    errors: Vec<String>,
    messages: std::sync::mpsc::Receiver<corral_ops::Message>,
    _capture: corral_core::MessageGuard,
    ctx: Ctx,
    app: App,
    terminal: Terminal<TestBackend>,
    /// When set, the last measured tick runs under a severe resource sample.
    pressure: Option<Arc<AtomicBool>>,
}

impl Paths {
    /// Warms the store caches with two ticks (the first may migrate or settle state), as a live
    /// board would have done long before the owner's next keypress.
    pub(crate) fn new(mut ctx: Ctx) -> Paths {
        let (warm_messages, _warm_receiver) = std::sync::mpsc::channel();
        worker_tick(&mut ctx, &warm_messages);
        worker_tick(&mut ctx, &warm_messages);
        let mut rows = Vec::new();
        let mut app = None;
        let (work, elapsed) = measure(|| app = Some(App::new(ctx.clone(), Mode::Board).unwrap()));
        rows.push(Row {
            path: "board_open".into(),
            work,
            elapsed,
        });
        let (sender, messages) = std::sync::mpsc::channel();
        let mut paths = Paths {
            rows,
            errors: Vec::new(),
            messages,
            _capture: corral_ops::capture_messages(sender),
            ctx,
            app: app.unwrap(),
            terminal: Terminal::new(TestBackend::new(160, 45)).unwrap(),
            pressure: None,
        };
        paths.draw("board_first_frame");
        paths
    }

    /// Ends [`Paths::run`] with a sync tick after raising `flag`, the machine pressure the
    /// context's resource probe reports.
    pub(crate) fn with_pressure(mut self, flag: Arc<AtomicBool>) -> Paths {
        self.pressure = Some(flag);
        self
    }

    fn record(&mut self, path: &str, work: Sample, elapsed: Duration) {
        self.rows.push(Row {
            path: path.into(),
            work,
            elapsed,
        });
    }

    fn draw(&mut self, path: &str) {
        let (app, terminal) = (&mut self.app, &mut self.terminal);
        let (work, elapsed) = measure(|| {
            terminal.draw(|frame| app.draw(frame)).unwrap();
        });
        self.record(path, work, elapsed);
    }

    /// Repeated frames; each must do the same work, so the worst one is recorded.
    fn frames(&mut self, path: &str) {
        let mut worst: Option<Row> = None;
        for _ in 0..FRAMES {
            self.draw(path);
            let row = self.rows.pop().unwrap();
            if worst.as_ref().is_none_or(|w| row.work.work > w.work.work) {
                worst = Some(row);
            }
        }
        self.rows.extend(worst);
    }

    fn key(&mut self, path: &str, code: KeyCode) {
        let app = &mut self.app;
        let (work, elapsed) = measure(|| app.handle_key(KeyEvent::new(code, KeyModifiers::NONE)));
        self.record(&format!("{path}_handle"), work, elapsed);
        self.draw(&format!("{path}_redraw"));
    }

    /// Mouse events through the application's handler, resolved against the last frame's hits.
    fn mouse(&mut self, path: &str, events: &[MouseEvent]) {
        let app = &mut self.app;
        let (work, elapsed) = measure(|| events.iter().for_each(|ev| app.handle_mouse(*ev)));
        self.record(&format!("{path}_handle"), work, elapsed);
        self.draw(&format!("{path}_redraw"));
    }

    /// The cell just inside each card the last frame drew, with its task.
    fn cards(&self) -> Vec<((u16, u16), String)> {
        (self.app.hits.regions().iter())
            .filter_map(|(rect, hit)| match hit {
                Hit::Card(id) => Some(((rect.x + 2, rect.y), id.clone())),
                _ => None,
            })
            .collect()
    }

    /// Wheel over a column, click a card in another column, then click it again to open it.
    fn board_mouse(&mut self) {
        let left =
            |kind: fn(MouseButton) -> MouseEventKind, at| pointer(kind(MouseButton::Left), at);
        let (first, _) = self
            .cards()
            .first()
            .cloned()
            .expect("the board drew no card");
        let wheel = [
            pointer(MouseEventKind::ScrollDown, first),
            pointer(MouseEventKind::ScrollUp, first),
        ];
        self.mouse("board_mouse_wheel", &wheel);
        let (_, id) = (self.cards().into_iter())
            .find(|(at, _)| at.0 != first.0)
            .expect("the board drew cards in only one column");
        let at = |paths: &Paths| paths.cards().into_iter().find(|(_, c)| *c == id).unwrap().0;
        let target = at(self);
        let click = [
            left(MouseEventKind::Down, target),
            left(MouseEventKind::Up, target),
        ];
        self.mouse("board_mouse_select", &click);
        let target = at(self);
        let click = [
            left(MouseEventKind::Down, target),
            left(MouseEventKind::Up, target),
        ];
        let app = &mut self.app;
        let (work, elapsed) = measure(|| click.iter().for_each(|ev| app.handle_mouse(*ev)));
        assert_eq!(
            self.app.stack.len(),
            2,
            "clicking the selected card did not open it"
        );
        self.record("board_mouse_open_handle", work, elapsed);
        self.draw("board_mouse_open_first_frame");
        self.mouse(
            "board_mouse_open_wheel",
            &[pointer(MouseEventKind::ScrollDown, (80, 20))],
        );
        let app = &mut self.app;
        let (work, elapsed) = measure(|| app.apply(Action::Back));
        self.record("board_mouse_back_handle", work, elapsed);
        self.draw("board_mouse_back_redraw");
        assert_eq!(self.app.stack.len(), 1, "the opened card did not close");
    }

    fn tick(&mut self, screen: &str) {
        let root = self.ctx.store.dir().display().to_string();
        let ctx = &mut self.ctx;
        let mut msg = None;
        mark("start", screen, &root);
        let (work, elapsed) = measure(|| msg = Some(worker_tick(ctx, &self.app.message_tx)));
        mark("end", screen, &root);
        self.record(&format!("sync_tick_{screen}"), work, elapsed);
        if let Some(WorkerMsg::Synced(Err(error))) = &msg {
            self.errors.push(format!("sync_tick_{screen}: {error}"));
        }
        let app = &mut self.app;
        let (work, elapsed) = measure(|| app.on_worker(msg.unwrap()));
        self.record(&format!("tick_apply_{screen}"), work, elapsed);
    }

    fn screen(&mut self, name: &str, open: Action) {
        let app = &mut self.app;
        let (work, elapsed) = measure(|| app.apply(open));
        assert_eq!(self.app.stack.len(), 2, "{name} did not open");
        self.record(&format!("{name}_open"), work, elapsed);
        self.draw(&format!("{name}_first_frame"));
        self.frames(&format!("{name}_frame"));
        // The worker's tick does not depend on the open screen; only applying it does.
        let msg = WorkerMsg::synced(&self.ctx, Ok(Vec::new()));
        let app = &mut self.app;
        let (work, elapsed) = measure(|| app.on_worker(msg));
        self.record(&format!("tick_apply_{name}"), work, elapsed);
        self.key(&format!("{name}_key_down"), KeyCode::Char('j'));
        let app = &mut self.app;
        let (work, elapsed) = measure(|| app.apply(Action::Back));
        self.record(&format!("{name}_back_handle"), work, elapsed);
        self.draw(&format!("{name}_back_redraw"));
        assert_eq!(self.app.stack.len(), 1, "{name} did not close");
    }

    /// Errors and board messages (toasts) raised while measuring; a fixture must raise none.
    pub(crate) fn errors(&self) -> Vec<String> {
        let mut errors = self.errors.clone();
        errors.extend(
            self.messages
                .try_iter()
                .map(|m| format!("{}: {}", m.source, m.text)),
        );
        errors
    }

    pub(crate) fn run(&mut self, detail: &str, review: &str) {
        self.frames("board_frame");
        self.tick("board");
        // A live board's log grows with agent activity and its task files change.
        let ctx = self.ctx.clone();
        ctx.store
            .log_event(None, "work_probe", serde_json::json!({"probe": true}))
            .unwrap();
        self.tick("board_after_event");
        ctx.store
            .update(detail, |task| {
                task.labels.push("work-probe".into());
                Ok(())
            })
            .unwrap();
        self.tick("board_after_task_edit");
        self.key("board_key_down", KeyCode::Char('j'));
        self.board_mouse();
        self.screen("detail", Action::OpenDetail(detail.into()));
        self.screen("review", Action::OpenReview(review.into()));
        self.screen("settings", Action::OpenSettings);
        if let Some(flag) = self.pressure.clone() {
            // The first severe sample escalates once, which is when finished work could be sized.
            flag.store(true, Ordering::SeqCst);
            self.tick("board_severe");
        }
    }
}

pub(crate) fn report(rows: &[Row]) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{:<26} ms={:>9.3} {}\n",
                row.path,
                row.elapsed.as_secs_f64() * 1000.0,
                row.work.render()
            )
        })
        .collect()
}

#[test]
#[ignore = "profiles a copied store; run through scripts/qa/board-work-profile.sh"]
fn profile_live_paths() {
    let root = std::path::PathBuf::from(std::env::var_os("CORRAL_PROFILE_STORE").unwrap());
    let ctx = Ctx::from_store(corral_core::Store::open(&root).unwrap());
    let tasks = ctx.store.list().unwrap();
    let pick = |var: &str, fallback: &dyn Fn() -> Option<String>| {
        std::env::var(var).ok().or_else(fallback).unwrap()
    };
    let largest = || {
        tasks
            .iter()
            .max_by_key(|t| std::fs::metadata(&t.path).map_or(0, |m| m.len()))
            .map(|t| t.id.clone())
    };
    let detail = pick("CORRAL_PROFILE_DETAIL", &largest);
    let review = pick("CORRAL_PROFILE_REVIEW", &|| {
        tasks
            .iter()
            .filter(|t| t.status == corral_core::Status::Review && !crate::plan::owns(t))
            .max_by_key(|t| t.runs.len())
            .map(|t| t.id.clone())
            .or_else(largest)
    });
    println!(
        "store={} tasks={} events_bytes={} detail={detail} review={review}",
        root.display(),
        tasks.len(),
        std::fs::metadata(ctx.store.dir().join("events.jsonl")).map_or(0, |m| m.len())
    );
    let mut paths = Paths::new(ctx);
    paths.run(&detail, &review);
    print!("{}", report(&paths.rows));
    for error in paths.errors() {
        println!("message: {error}");
    }
}
