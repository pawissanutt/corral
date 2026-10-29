//! Store reloads and sync notifications.
use super::*;

impl App {
    pub(super) fn on_worker(&mut self, msg: WorkerMsg) {
        match msg {
            WorkerMsg::Herdr(online) => {
                if self.herdr_online && !online {
                    self.on_message(Message {
                        task: None,
                        file: None,
                        source: "Terminal engine unavailable",
                        text: corral_herdr::friendly_error(
                            &corral_herdr::EngineProblem::Down.into(),
                        ),
                    });
                }
                self.herdr_online = online;
            }
            WorkerMsg::Synced(Ok((changes, snapshot))) => {
                self.last_sync_error = None;
                for c in &changes {
                    if let Some(t) = change_toast(c, snapshot.stage_reason(c)) {
                        self.messages.record(Message {
                            task: Some(c.task.clone()),
                            file: None,
                            source: if c.stage.is_some() {
                                "autopilot"
                            } else {
                                "sync"
                            },
                            text: format!("{}\n{}", t.title, t.body),
                        });
                        // These are real state transitions, including later runs entering the same
                        // stage. Only recurring diagnostics suppress their toast.
                        self.toast(t);
                    }
                }
                if self.ctx.store.config != snapshot.ctx.store.config {
                    self.toast(Toast::new(
                        Tone::Info,
                        "Config reloaded",
                        snapshot.ctx.store.config_path().display().to_string(),
                    ));
                }
                self.counts = Counts::from_tasks(&snapshot.tasks, &snapshot.ctx.store.config)
                    .with_attention(&snapshot.tasks, &snapshot.attention);
                self.resources = snapshot.resources.clone();
                self.operating_header = snapshot.operating_header.clone();
                self.ctx = snapshot.ctx.clone();
                // Hidden screens refresh when exposed by pop(), not on every periodic sync.
                if self.stack.len() > 1
                    && let Some(top) = self.stack.last_mut()
                {
                    top.refresh_board_snapshot(&self.ctx, &snapshot);
                }
                if let Some(board) = self.stack.first_mut() {
                    board.apply_board_snapshot(*snapshot);
                }
            }
            WorkerMsg::Synced(Err(e)) => {
                if self.last_sync_error.as_deref() != Some(e.as_str()) {
                    self.on_message(Message {
                        task: None,
                        file: None,
                        source: "Sync failed",
                        text: e.clone(),
                    });
                    self.last_sync_error = Some(e);
                }
            }
        }
    }

    pub(super) fn drain_messages(&mut self) {
        while let Ok(message) = self.message_rx.try_recv() {
            self.on_message(message);
        }
    }

    pub(super) fn on_message(&mut self, message: Message) {
        if !self.messages.record(message.clone()) {
            return;
        }
        let mut toast = Toast::new(Tone::Warn, message.source, message.text);
        if let Some(task) = message.task {
            toast = toast.about(task);
        }
        self.toast(toast);
    }

    pub(crate) fn toast(&mut self, t: Toast) {
        self.toasts.push((t, Instant::now()));
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
    }

    pub(super) fn load_counts(&mut self) {
        self.resources = crate::resources::ResourceDisplay::sample(&self.ctx);
        self.operating_header = operating::readback(&self.ctx);
        let Ok(tasks) = self.ctx.store.list() else {
            return;
        };
        self.counts = Counts::from_tasks(&tasks, &self.ctx.store.config);
        if let Some(columns) = self.stack.first().and_then(|view| view.column_counts()) {
            for (status, count) in Status::COLUMNS.into_iter().zip(columns) {
                match status {
                    Status::NeedsMe => self.counts.needs = count,
                    Status::Review => self.counts.review = count,
                    Status::Ready => self.counts.ready = count,
                    _ => {}
                }
            }
        } else {
            let attention = crate::attention::load(&self.ctx, jiff::Timestamp::now());
            self.counts = self.counts.with_attention(&tasks, &attention);
        }
    }

    pub(super) fn reload_config(&mut self) {
        match self.ctx.reload_config() {
            Ok(false) => {}
            Ok(true) => {
                self.toast(Toast::new(
                    Tone::Info,
                    "Config reloaded",
                    self.ctx.store.config_path().display().to_string(),
                ));
            }
            Err(e) => self.toast(Toast::new(
                Tone::Bad,
                "Config not reloaded",
                corral_herdr::friendly_error(&e),
            )),
        }
    }

    /// Explicit actions remain synchronous, including hidden views, to preserve immediate input semantics.
    pub(super) fn reload(&mut self) {
        self.reload_config();
        for v in &mut self.stack {
            v.refresh(&self.ctx);
        }
        self.load_counts();
    }
}

impl Counts {
    fn with_attention(
        mut self,
        tasks: &[corral_core::Task],
        attention: &crate::attention::Snapshot,
    ) -> Self {
        let count = |status| {
            tasks
                .iter()
                .filter(|task| crate::attention::column(task, attention) == Some(status))
                .count()
        };
        self.needs = count(Status::NeedsMe);
        self.review = count(Status::Review);
        self.ready = count(Status::Ready);
        self
    }

    fn from_tasks(tasks: &[corral_core::Task], config: &corral_core::Config) -> Self {
        let count = |status: Status| tasks.iter().filter(|task| task.status == status).count();
        let slots = corral_ops::SlotUsage::from_tasks(tasks, &config.limits);
        Self {
            needs: count(Status::NeedsMe),
            used: slots.total(),
            limit: config.limits.running,
            review: count(Status::Review),
            ready: count(Status::Ready),
            tasks: slots.tasks,
            reviews: slots.reviews,
            gates: slots.gates,
            reservations: slots.reservations,
        }
    }
}

/// The toast for one sync change. Autopilot toasts are about their task (a click opens it); blocked and escalated
/// ones carry the stored `reason` of the stage.
pub(crate) fn change_toast(c: &Change, reason: Option<String>) -> Option<Toast> {
    if let Some(stage) = c.stage {
        let with_reason = |r: Option<String>| r.filter(|r| !r.trim().is_empty());
        let (tone, title, body) = match stage {
            AutoStage::Changes => (Tone::Info, format!("↺ {} sent back", c.task), None),
            AutoStage::Blocked => (
                Tone::Warn,
                format!("⛔ {} blocked", c.task),
                with_reason(reason),
            ),
            AutoStage::Reviewed => (Tone::Good, format!("✓ {} reviewed", c.task), None),
            AutoStage::Done => (
                Tone::Good,
                format!("✓ {} landed automatically", c.task),
                None,
            ),
            AutoStage::Escalated => (
                Tone::Attention,
                format!("▲ {} escalated to you", c.task),
                with_reason(reason),
            ),
            AutoStage::Gating | AutoStage::Reviewing | AutoStage::Landing => return None,
        };
        let body = body.unwrap_or_else(|| c.note.clone());
        return Some(Toast::new(tone, title, body).about(&c.task));
    }
    match c.to {
        Status::NeedsMe => Some(
            Toast::new(
                Tone::Attention,
                format!("▲ {} blocked", c.task),
                c.note.clone(),
            )
            .about(&c.task),
        ),
        Status::Review => Some(
            Toast::new(
                Tone::Good,
                format!("◆ {} ready for review", c.task),
                c.note.clone(),
            )
            .about(&c.task),
        ),
        Status::Running if !c.from.holds_slot() => Some(Toast::new(
            Tone::Info,
            format!("⇢ {} dispatched", c.task),
            c.note.clone(),
        )),
        _ => None,
    }
}
