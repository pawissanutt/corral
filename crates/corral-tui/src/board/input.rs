//! Board keyboard actions.
use super::*;
use corral_ops::command::{
    CmdState, Command, Invoker,
    route::{Patience, submit_with},
};
use corral_protocol::cmd::task::TaskCommand;

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
        let outcome = submit_with(
            ctx.store.root(),
            &Invoker::board(),
            Command::Task(TaskCommand::Move {
                id: id.clone(),
                status: target.as_str().to_owned(),
                force: false,
                owner_ok: false,
                reason: None,
            }),
            Patience::Interactive,
        );
        match outcome.state {
            CmdState::Done => {
                self.refresh(ctx);
                Action::None
            }
            _ => Action::Toast(Toast::new(
                Tone::Bad,
                format!("Move {id} failed"),
                outcome.error.map_or(outcome.stderr, |error| error.message),
            )),
        }
    }

    /// Take-back (`W`) or save (`K`) of a transferred card through the SAME owner services
    /// the CLI and assistant use. Binds the card's contract revision as last loaded; a
    /// changed task refuses as stale instead of acting on newer state.
    pub(super) fn transfer_card(&mut self, ctx: &Ctx, take_back: bool) -> Action {
        use corral_ops::assistant::workflow::handoff::{owner, status};
        let Some(t) = self.selected_task().filter(|t| status::transfer_fenced(t)) else {
            return Action::Toast(Toast::new(
                Tone::Info,
                "No handoff on this card",
                "W/K apply only to work handed off from an assistant",
            ));
        };
        let id = t.id.clone();
        let contract = match corral_core::TaskRef::contract_revision(t) {
            Ok(c) => c,
            Err(e) => {
                return Action::Toast(Toast::new(
                    Tone::Bad,
                    "Handoff unavailable",
                    format!("{e:#}"),
                ));
            }
        };
        let outcome = if take_back {
            owner::take_back(ctx, &id, &contract).and_then(|r| Ok(serde_json::to_string(&r)?))
        } else {
            owner::save(ctx, &id, &contract).map(|_| "Saved for later · Held".to_string())
        };
        self.refresh(ctx);
        match outcome {
            Ok(text) => Action::Toast(Toast::new(
                Tone::Info,
                if take_back {
                    format!("{id} returning to assistant")
                } else {
                    format!("{id} saved")
                },
                text,
            )),
            Err(e) => Action::Toast(Toast::new(
                Tone::Bad,
                format!("{id}: nothing changed"),
                format!("{e:#}"),
            )),
        }
    }

    /// Goal-aware continuation of a handed-off card (`N` continue planning, `U` review
    /// this plan, `T` take over and finish) through the SAME preview/start services. Plan
    /// scopes never offer implementation; the preview pins the current revision.
    pub(super) fn continue_card(&mut self, ctx: &Ctx, key: char) -> Action {
        use corral_ops::assistant::workflow::handoff::{Goal, continuation, owner, status};
        let Some(t) = self.selected_task().filter(|t| status::transfer_fenced(t)) else {
            return Action::Toast(Toast::new(
                Tone::Info,
                "No handoff on this card",
                "N/U/T apply only to work handed off from an assistant",
            ));
        };
        let id = t.id.clone();
        // The same cached canonical projection the card and its menu show, with its pin.
        let shown = self.handoffs.get(&id);
        let plan_only = shown.is_some_and(|r| r.plan_only);
        let pin = shown.and_then(|r| r.pin.clone());
        let goal = match (key, plan_only) {
            ('N', true) => Goal::PlanContinue,
            ('U', true) => Goal::PlanReviewOnly,
            ('T', false) => Goal::ImplementationShip,
            _ => {
                return Action::Toast(Toast::new(
                    Tone::Info,
                    format!("{id}: not offered"),
                    if plan_only {
                        "Plan scope: N continue planning or U review this plan; never implementation"
                    } else {
                        "Implementation scope: T take over and finish"
                    },
                ));
            }
        };
        let outcome = continuation::preview_pinned(ctx, pin.as_ref(), &id, goal)
            .and_then(|choice| owner::start(ctx, &choice, &choice.digest()?));
        self.refresh(ctx);
        match outcome {
            Ok(task) => Action::Toast(Toast::new(
                Tone::Info,
                format!("{id} continuing"),
                status::project(ctx, &task).map_or_else(|| "Started".into(), |r| r.label),
            )),
            Err(e) => Action::Toast(Toast::new(
                Tone::Bad,
                format!("{id}: nothing changed"),
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
                        "a jumps to Running and Blocked cards with a live run",
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
                        "r replies to Blocked agents awaiting an answer",
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
            KeyCode::Char('W') => return self.transfer_card(ctx, true),
            KeyCode::Char('K') => return self.transfer_card(ctx, false),
            KeyCode::Char(c @ ('N' | 'U' | 'T')) => return self.continue_card(ctx, c),
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
            KeyCode::Char('s') => return id.map(Action::OpenAssistant).unwrap_or(Action::None),
            KeyCode::Char('S') => return Action::OpenAssistants,
            KeyCode::Char('A') => return Action::OpenArtifacts,
            KeyCode::Char('V') => return Action::OpenGallery,
            KeyCode::Char('Y') => return Action::OpenHistory,
            KeyCode::Char('/') => return Action::OpenSearch,
            KeyCode::Char('.') => self.open_menu(),
            KeyCode::Char(',') => return Action::OpenSettings,
            KeyCode::Char('O') => return Action::OpenOperating,
            KeyCode::Char('?') => {
                self.overlay = Some(Overlay::Help(HelpView::new(self.nav.clone())))
            }
            KeyCode::Char('t') => return Action::OpenTour,
            KeyCode::Char('B') => {
                let Some(task) = self.selected_task().cloned() else {
                    return Action::None;
                };
                return match corral_ops::review_waiver::eligible(&task) {
                    Ok(()) => {
                        self.overlay = Some(Overlay::Waive(waive::ConfirmWaive::new(&task)));
                        Action::None
                    }
                    Err(error) => Action::Toast(Toast::new(
                        Tone::Info,
                        "Review cannot be skipped",
                        format!("{error:#}"),
                    )),
                };
            }
            KeyCode::Char('F') => {
                return match self.selected_task() {
                    Some(t) if menu::recoverable(t) => Action::OpenRecover(t.id.clone()),
                    _ => Action::Toast(Toast::new(
                        Tone::Info,
                        "Nothing to recover",
                        "recovery is offered on Blocked, Review and plan-decision cards",
                    )),
                };
            }
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
