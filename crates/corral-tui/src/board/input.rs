//! Board keyboard actions.
use super::*;

impl BoardView {
    /// What the board returns after its open overlay handled an input: Back closes the overlay.
    pub(super) fn overlay_outcome(&mut self, action: Action, ctx: &Ctx) -> Action {
        match action {
            Action::Back => {
                self.overlay = None;
                Action::None
            }
            Action::BackWithToast(t) => {
                self.overlay = None;
                self.refresh(ctx);
                Action::Toast(t)
            }
            Action::OpenTour => {
                self.overlay = None;
                Action::OpenTour
            }
            other => other,
        }
    }

    pub(super) fn move_card(&mut self, ctx: &Ctx, right: bool) -> Action {
        let Some(t) = self.selected_task() else {
            return Action::None;
        };
        let target = match (t.status, right) {
            (Status::Idea | Status::Backlog, true) => Status::Ready,
            (Status::Ready, false) => Status::Backlog,
            _ => return Action::Toast(Toast::new(Tone::Warn, "Not a manual move", MOVE_RULE)),
        };
        let id = t.id.clone();
        match corral_ops::move_task(ctx, &id, target, false) {
            Ok(_) => {
                self.refresh(ctx);
                Action::None
            }
            Err(e) => Action::Toast(Toast::new(
                Tone::Bad,
                format!("Move {id} failed"),
                format!("{e:#}"),
            )),
        }
    }

    pub(super) fn board_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        let ncols = self.columns().len();
        let status = self.selected_task().map(|t| t.status);
        let id = self.selected_task().map(|t| t.id.clone());
        self.follow = true;
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                self.col = self.col.saturating_sub(1);
                self.row = 0;
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.col = (self.col + 1).min(ncols.saturating_sub(1));
                self.row = 0;
            }
            KeyCode::Char('j') | KeyCode::Down => self.row += 1,
            KeyCode::Char('k') | KeyCode::Up => self.row = self.row.saturating_sub(1),
            KeyCode::Enter => {
                return match self.selected_task() {
                    Some(t) => Action::OpenDetail(t.id.clone()),
                    None => Action::None,
                };
            }
            KeyCode::Char('a') => {
                return match (status, id) {
                    (Some(s), Some(id)) if s.holds_slot() => Action::Jump(id),
                    _ => Action::Toast(Toast::new(
                        Tone::Info,
                        "No live run",
                        "a jumps to Running and Needs me cards",
                    )),
                };
            }
            KeyCode::Char('r') => match self.selected_task() {
                Some(t) if crate::plan::owns(t) => return Action::OpenPlan(t.id.clone()),
                Some(t) if t.status == Status::NeedsMe && !t.escalated() => {
                    self.reply = Some(ReplyLine::new(t));
                }
                _ => {
                    return Action::Toast(Toast::new(
                        Tone::Info,
                        "Nothing to reply to",
                        "r replies to Needs me cards",
                    ));
                }
            },
            KeyCode::Char('d') => match self.selected_task() {
                Some(t)
                    if !t.tracker
                        && matches!(t.status, Status::Idea | Status::Backlog | Status::Ready) =>
                {
                    self.overlay = Some(Overlay::Dispatch(Box::new(DispatchSheet::new(
                        ctx,
                        t,
                        &self.tasks,
                    ))));
                }
                _ => {
                    return Action::Toast(Toast::new(
                        Tone::Info,
                        "Not dispatchable",
                        "d dispatches non-tracker Backlog, Ready and idea cards",
                    ));
                }
            },
            KeyCode::Char('H') => return self.move_card(ctx, false),
            KeyCode::Char('L') => return self.move_card(ctx, true),
            KeyCode::Char('X') => {
                if let Some(task) = self
                    .selected_task()
                    .filter(|t| !matches!(t.status, Status::Done | Status::Dropped))
                {
                    self.overlay = Some(Overlay::Drop(drop::ConfirmDrop::new(task)));
                }
            }
            KeyCode::Char('n') | KeyCode::Char('c') => {
                let cwd =
                    std::env::current_dir().unwrap_or_else(|_| ctx.store.root().to_path_buf());
                self.overlay = Some(Overlay::Capture(CaptureView::new(ctx, cwd)));
            }
            KeyCode::Char('P') => {
                return match corral_ops::open_pm(ctx) {
                    Ok(pane) => Action::Toast(Toast::new(
                        Tone::Info,
                        "PM pane focused",
                        format!("corral PM · {pane}"),
                    )),
                    Err(e) => Action::Toast(Toast::new(
                        Tone::Bad,
                        "Could not open the PM",
                        corral_herdr::friendly_error(&e),
                    )),
                };
            }
            KeyCode::Char('A') => return Action::OpenArtifacts,
            KeyCode::Char('V') => return Action::OpenGallery,
            KeyCode::Char('Y') => return Action::OpenHistory,
            KeyCode::Char('/') => return Action::OpenSearch,
            KeyCode::Char('.') => self.open_menu(),
            KeyCode::Char(',') => return Action::OpenSettings,
            KeyCode::Char('O') => return Action::ToggleAutopilot,
            KeyCode::Char('?') => {
                self.overlay = Some(Overlay::Help(HelpView::new(self.nav.clone())))
            }
            KeyCode::Char('t') => return Action::OpenTour,
            KeyCode::Char('*')
            | KeyCode::Char('g')
            | KeyCode::Char('G')
            | KeyCode::Char('1'..='9') => {
                return self.pin_key(key, ctx);
            }
            KeyCode::Char('i') => self.ideas_only = !self.ideas_only,
            KeyCode::Char('D') => self.show_done = !self.show_done,
            _ => return Action::None,
        }
        self.select_current();
        Action::None
    }
}
