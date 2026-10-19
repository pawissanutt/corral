//! View lifecycle and board frame layout.
use super::*;

impl View for BoardView {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, hits: &mut HitMap) {
        self.routes.poll();
        let now = (self.now)();
        let (board, reply_area) = match &self.reply {
            Some(_) if area.height > ReplyLine::HEIGHT + 4 => {
                let [b, r] =
                    Layout::vertical([Constraint::Fill(1), Constraint::Length(ReplyLine::HEIGHT)])
                        .areas(area);
                (b, Some(r))
            }
            _ => (area, None),
        };
        let board = if !self.pins.is_empty() && board.height > 4 {
            let [strip, rest] =
                Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(board);
            shelf::draw_strip(
                f,
                strip,
                &self.pins,
                &mut self.shelf_offset,
                self.shelf_cursor,
                hits,
            );
            rest
        } else {
            board
        };
        let cols = self.columns();
        let n = cols.len() as u32;
        let rects = Layout::horizontal(cols.iter().map(|_| Constraint::Ratio(1, n)))
            .spacing(1)
            .split(board);
        self.col_rects.clear();
        self.card_rects.clear();
        for (ci, col) in cols.into_iter().enumerate() {
            self.col_rects.push((col, rects[ci]));
            self.draw_column(f, rects[ci], col, &ctx.store.config, now, hits);
            if self.drop_target(col) == Some(false) {
                overlays::dim_backdrop(f.buffer_mut(), rects[ci]);
            }
        }
        if self.is_empty() && self.overlay.is_none() {
            empty::draw(f, board, hits, &ctx.store.config.ui);
        }
        if let (Some(reply), Some(r)) = (&self.reply, reply_area) {
            reply.draw(f, r, hits);
        }
        if let Some(menu) = &self.menu {
            menu.draw(f, board, hits);
        }
        if let Some(ov) = &mut self.overlay {
            overlays::dim_backdrop(f.buffer_mut(), area);
            ov.view().draw(f, area, ctx, hits);
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if let Some(ov) = &mut self.overlay {
            let action = ov.view().on_key(key, ctx);
            return self.overlay_outcome(action, ctx);
        }
        if let Some(menu) = &mut self.menu {
            let pick = menu.key(key);
            return self.menu_pick(pick, ctx);
        }
        if self.cancel_drag() {
            return Action::None;
        }
        if self.shelf_cursor.is_some() && self.reply.is_none() {
            return self.shelf_key(key, ctx);
        }
        if let Some(reply) = &mut self.reply {
            return match reply.on_key(key, ctx) {
                Some(action) => {
                    self.reply = None;
                    if matches!(action, Action::Toast(_)) {
                        self.refresh(ctx);
                    }
                    action
                }
                None => Action::None,
            };
        }
        self.board_key(key, ctx)
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if let Some(ov) = &self.overlay {
            return ov.view_ref().hints();
        }
        if let Some(menu) = &self.menu {
            return menu.hints();
        }
        if self.dragging() {
            return vec![("esc", "cancel drag")];
        }
        if self.reply.is_some() {
            return vec![("⏎", "send"), ("esc", "cancel")];
        }
        if self.shelf_cursor.is_some() {
            return pins::SHELF_HINTS.to_vec();
        }
        let task = self.selected_task();
        let status = task.map(|t| t.status);
        let awaiting = task.is_some_and(Task::awaiting_owner);
        // Most useful first: narrow terminals cut the tail of the footer.
        let mut h = Vec::new();
        if task.is_some() {
            h.push(("⏎", "overview"));
        }
        h.extend([("/", "search"), ("?", "help"), ("q", "leave")]);
        match status {
            Some(Status::NeedsMe) if awaiting => h.push(("a", "open agent")),
            Some(Status::NeedsMe) => h.extend([("a", "open agent"), ("r", "reply")]),
            Some(Status::Running) => h.push(("a", "open agent")),
            Some(Status::Ready | Status::Backlog | Status::Idea)
                if !task.is_some_and(|t| t.tracker) =>
            {
                h.push(("d", "dispatch"));
            }
            _ => {}
        }
        h.push(("n", "new"));
        if status.is_some_and(|s| !matches!(s, Status::Done | Status::Dropped)) {
            h.push(("X", "drop"));
        }
        let pinned = task.is_some_and(|t| self.task_pinned(&t.id));
        h.push(("*", if pinned { "unpin" } else { "pin" }));
        if !self.pins.is_empty() {
            h.push(("g", "shelf"));
        }
        h.extend([
            (".", "menu"),
            ("H/L", "move"),
            ("A", "artifacts"),
            ("V", "gallery"),
            ("Y", "history"),
            ("P", "PM"),
            ("O", "mode"),
            ("i", "ideas"),
            ("D", "done"),
            (",", "settings"),
        ]);
        h
    }

    fn refresh(&mut self, ctx: &Ctx) {
        match BoardSnapshot::load(ctx) {
            Ok(snapshot) => self.apply_snapshot(snapshot),
            Err(error) => {
                corral_ops::report_message(None, "Board not reloaded", format!("{error:#}"))
            }
        }
    }

    fn apply_board_snapshot(&mut self, snapshot: BoardSnapshot) {
        self.apply_snapshot(snapshot);
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        self.mouse(ev, m)
    }

    fn captures_text(&self) -> bool {
        self.overlay.is_some() || self.reply.is_some() || self.menu.is_some()
    }

    fn board_shortcuts(&self) -> bool {
        // The shelf consumes canonical board keys too; focus must not restore old bindings.
        !self.captures_text() && !self.dragging()
    }

    fn has_unsaved_text(&self) -> bool {
        match &self.overlay {
            Some(ov) => ov.view_ref().has_unsaved_text(),
            None => self
                .reply
                .as_ref()
                .is_some_and(|r| !r.input.text.is_empty()),
        }
    }
}
