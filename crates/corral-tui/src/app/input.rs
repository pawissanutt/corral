//! Input routing and view-stack actions.
use super::*;

impl App {
    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        let mut key = key;
        if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && self.stack.last().is_some_and(|view| view.board_shortcuts())
            && let KeyCode::Char(physical) = key.code
        {
            let Some(canonical) = self.ctx.store.config.ui.canonical_key(physical) else {
                return;
            };
            key.code = KeyCode::Char(canonical);
        }
        self.handle_canonical_key(key);
    }

    pub(super) fn handle_canonical_key(&mut self, key: KeyEvent) {
        if let KeyCode::F(n @ 8..=10) = key.code {
            if let Some(i) = self.toasts.len().checked_sub(usize::from(n - 8) + 1) {
                self.click_toast(i);
            }
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        let at_board_root = self.mode == Mode::Board && self.stack.len() == 1;
        let Some(top) = self.stack.last_mut() else {
            self.quit = true;
            return;
        };
        if at_board_root && !top.captures_text() && key.code == KeyCode::Char('M') {
            self.stack.push(Box::new(self.messages.view()));
            return;
        }
        if at_board_root && !top.captures_text() && key.code == KeyCode::Char('q') {
            let leave = LeaveView::new(self.counts.used, self.nav.clone());
            self.stack.push(Box::new(leave));
            return;
        }
        let action = top.on_key(key, &self.ctx);
        self.apply(action);
    }

    /// Resolves the hit under the pointer from the last frame. Key hits go through `handle_key`, so a click and
    /// its key are one code path; everything else goes to the topmost view (overlays are modal).
    pub(crate) fn handle_mouse(&mut self, ev: MouseEvent) {
        if ev.kind == MouseEventKind::Moved {
            return;
        }
        let hit = self.hits.at(ev.column, ev.row).cloned();
        let left = ev.kind == MouseEventKind::Down(MouseButton::Left);
        let double = left && self.clicks.press(self.clock.now(), ev.column, ev.row);
        match (&hit, left) {
            (Some(Hit::Key(k)), true) => return self.handle_canonical_key(*k),
            (Some(Hit::Behind(k)), true) => {
                self.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                return self.handle_canonical_key(*k);
            }
            (Some(Hit::Toast(i)), true) => return self.click_toast(*i),
            (Some(Hit::Dismiss), true) => {
                if !self.stack.last().is_some_and(|v| v.has_unsaved_text()) {
                    self.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                }
                return;
            }
            (Some(Hit::Dismiss), false) | (Some(Hit::Inert), true) => return,
            _ => {}
        }
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        let m = MouseCtx {
            ctx: &self.ctx,
            hit,
            double,
        };
        let action = top.on_mouse(ev, &m);
        self.apply(action);
    }

    /// A click on a toast dismisses it; a toast about a task also opens that task (review for a Review card).
    pub(super) fn click_toast(&mut self, i: usize) {
        if i >= self.toasts.len() {
            return;
        }
        let (toast, _) = self.toasts.remove(i);
        if let Some(id) = toast.task {
            self.apply(Action::OpenDetail(id));
        }
    }

    pub(super) fn push(&mut self, view: Result<Box<dyn View>>, what: &str) {
        match view {
            Ok(v) => self.stack.push(v),
            Err(e) => self.toast(Toast::new(
                Tone::Bad,
                format!("Could not open {what}"),
                corral_herdr::friendly_error(&e),
            )),
        }
    }

    pub(super) fn pop(&mut self) {
        self.stack.pop();
        if self.stack.is_empty() {
            self.quit = true;
        } else {
            self.reload();
        }
    }

    pub(crate) fn apply(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Quit => self.quit = true,
            Action::Back => self.pop(),
            Action::BackWithToast(t) => {
                self.toast(t);
                self.pop();
            }
            Action::Toast(t) => {
                self.toast(t);
                self.reload();
            }
            Action::Reload => {
                if let Some(tx) = &self.trigger {
                    let _ = tx.send(());
                }
                self.reload();
            }
            Action::OpenDetail(id) => {
                let v = DetailView::new(&self.ctx, &id).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, &id);
            }
            Action::OpenAcceptance(id) => {
                let v =
                    DetailView::acceptance(&self.ctx, &id).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, &format!("acceptance of {id}"));
            }
            Action::OpenPlan(id) => {
                let v = DetailView::plan(&self.ctx, &id).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, &format!("plan of {id}"));
            }
            // Before implementation, a plan-first task's review is its plan decision.
            Action::OpenReview(id)
                if self.ctx.store.get(&id).is_ok_and(|t| crate::plan::owns(&t)) =>
            {
                self.apply(Action::OpenPlan(id));
            }
            Action::OpenStage(id, stage) => {
                let v = crate::stages::StageView::new(&self.ctx, &id, stage)
                    .map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, &format!("stages of {id}"));
            }
            Action::OpenReview(id) => {
                let v = ReviewView::new(&self.ctx, &id).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, &format!("review {id}"));
            }
            Action::OpenArtifacts => {
                let v = ArtifactsView::new(&self.ctx, None).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, "artifacts");
            }
            Action::OpenGallery => {
                let v = crate::gallery::GalleryView::new(&self.ctx)
                    .map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, "gallery");
            }
            Action::OpenHistory => {
                let v = HistoryView::new(&self.ctx).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, "history");
            }
            Action::OpenSearch => {
                let v = SearchView::new(&self.ctx).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, "search");
            }
            Action::OpenSettings => self.stack.push(Box::new(SettingsView::new(&self.ctx))),
            Action::ToggleAutopilot => self.toggle_autopilot(),
            Action::OpenTour => self.stack.push(Box::new(TourView::new(self.nav.clone()))),
            Action::OpenPin(i) => match crate::shelf::open(&self.ctx, i) {
                Ok(crate::shelf::Opening::Action(a)) => self.apply(a),
                Ok(crate::shelf::Opening::Preview(v)) => self.stack.push(v),
                Err(e) => self.toast(Toast::new(
                    Tone::Warn,
                    format!("Could not open pin #{}", i + 1),
                    format!("{e:#}"),
                )),
            },
            Action::Jump(id) => {
                if let Err(e) = corral_ops::jump(&self.ctx, &id) {
                    self.toast(Toast::new(
                        Tone::Bad,
                        format!("Jump to {id} failed"),
                        corral_herdr::friendly_error(&e),
                    ));
                }
            }
        }
    }
}
