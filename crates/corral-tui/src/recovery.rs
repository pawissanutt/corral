//! Owner recovery sheet: a read-only preview, owner-written reason, then a separate confirmation step.
//!
//! The sheet never applies on preview, on Enter in a text field or on a stale proposal. Confirm re-derives the
//! preview from the same input and, when anything changed, returns to editing with the draft kept.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use corral_core::{
    CheckDisposition, ProfileChoice, RecoveryAction, RecoveryAdmissionStage, RecoveryCheck,
    RecoveryMerge, RecoveryPreview, RecoveryRecord, RecoveryReviewStage, RecoveryReviewer, Task,
};
use corral_ops::Ctx;
use corral_ops::recovery::{self, RecoveryInput};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};

use crate::artifacts::ui;
use crate::hit::{self, Hit};
use crate::{Action, MouseCtx, Toast, Tone};

mod draw;
pub(crate) mod labels;
#[cfg(test)]
mod tests;

/// Mouse zones ([`Hit::Item`]) of the sheet.
const Z_ROW: u8 = 0;
const Z_KIND: u8 = 1;
const Z_PARAM: u8 = 2;
const Z_REVIEWER: u8 = 3;

/// Obtains the exact owner confirmation and applies. Never a PM or autopilot path.
pub(crate) type Authorize =
    fn(&Ctx, &RecoveryPreview, &RecoveryInput, &str) -> Result<RecoveryRecord>;

/// The board's explicit confirmation, bound to the exact current preview, then one apply.
fn confirm_and_apply(
    ctx: &Ctx,
    preview: &RecoveryPreview,
    input: &RecoveryInput,
    reason: &str,
) -> Result<RecoveryRecord> {
    let confirmation = recovery::recover_confirm_tui(ctx, preview, input, reason)?;
    recovery::recover_apply(ctx, preview, input, Some(confirmation))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    UseReviewer,
    AcceptPlan,
    AcceptCode,
    ForceAdmission,
}

pub(crate) const KINDS: [Kind; 4] = [
    Kind::UseReviewer,
    Kind::AcceptPlan,
    Kind::AcceptCode,
    Kind::ForceAdmission,
];

impl Kind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::UseReviewer => "use-reviewer",
            Self::AcceptPlan => "accept-plan",
            Self::AcceptCode => "accept-code",
            Self::ForceAdmission => "force-admission",
        }
    }

    /// The pill options of this action's one parameter (empty when it has none).
    pub(crate) fn params(self) -> &'static [&'static str] {
        match self {
            Self::UseReviewer => &["plan-review", "code-review"],
            Self::AcceptPlan => &[],
            Self::AcceptCode => &["keep", "rebase", "ff"],
            Self::ForceAdmission => &[
                "dispatch",
                "plan-dispatch",
                "gate",
                "code-review",
                "plan-review",
            ],
        }
    }

    fn param_name(self) -> &'static str {
        match self {
            Self::AcceptCode => "merge",
            _ => "stage",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Kind,
    Param,
    Reviewer,
    Check(usize),
    Notes,
    Reason,
    Cancel,
    Continue,
    Edit,
    Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Edit,
    Confirm,
}

pub(crate) struct RecoverySheet {
    pub(crate) task: String,
    pub(crate) kind: usize,
    pub(crate) param: usize,
    pub(crate) reviewers: Vec<String>,
    pub(crate) reviewer: usize,
    /// Checks the current preview proposes to override or waive; each needs its own acknowledgement.
    pub(crate) offered: Vec<RecoveryCheck>,
    pub(crate) acknowledged: BTreeSet<RecoveryCheck>,
    pub(crate) notes: String,
    pub(crate) reason: String,
    pub(crate) preview: Option<std::result::Result<RecoveryPreview, String>>,
    pub(crate) step: Step,
    pub(crate) focus: usize,
    pub(crate) notice: Option<(Tone, String)>,
    pub(crate) scroll: ui::Scroll,
    /// Keep the focused control in view; the wheel and Page keys turn this off until focus moves.
    pub(crate) follow: bool,
    /// The body's page height at the last draw (Page Up/Down step).
    pub(crate) page: usize,
    authorize: Authorize,
}

impl RecoverySheet {
    /// Opens on the action matching the task's state; previews immediately (read-only).
    pub(crate) fn new(ctx: &Ctx, task: &Task) -> RecoverySheet {
        let kind = if crate::plan::owns(task) {
            Kind::AcceptPlan
        } else if task.awaiting_owner() {
            Kind::AcceptCode
        } else {
            Kind::UseReviewer
        };
        Self::with(ctx, &task.id, kind, confirm_and_apply)
    }

    pub(crate) fn with(ctx: &Ctx, task: &str, kind: Kind, authorize: Authorize) -> RecoverySheet {
        let mut sheet = RecoverySheet {
            task: task.to_string(),
            kind: KINDS.iter().position(|k| *k == kind).unwrap_or(0),
            param: 0,
            reviewers: ctx.store.config.profile_names(),
            reviewer: 0,
            offered: Vec::new(),
            acknowledged: BTreeSet::new(),
            notes: String::new(),
            reason: String::new(),
            preview: None,
            step: Step::Edit,
            focus: 0,
            notice: None,
            scroll: ui::Scroll::default(),
            follow: true,
            page: 10,
            authorize,
        };
        sheet.refresh_preview(ctx);
        sheet.focus = sheet.position(Control::Cancel);
        sheet
    }

    pub(crate) fn kind(&self) -> Kind {
        KINDS[self.kind % KINDS.len()]
    }

    pub(crate) fn input(&self, ctx: &Ctx) -> Result<RecoveryInput> {
        let kind = self.kind();
        let param = kind.params().get(self.param).copied().unwrap_or("");
        let exceptions = self.acknowledged.clone();
        let action = match kind {
            Kind::UseReviewer => {
                let name = self
                    .reviewers
                    .get(self.reviewer)
                    .context("no reviewer profile")?;
                let resolved = ctx.store.config.resolve_profile(
                    &ProfileChoice {
                        profile: Some(name.clone()),
                        ..Default::default()
                    },
                    None,
                    None,
                )?;
                RecoveryAction::UseReviewer {
                    stage: if param == "code-review" {
                        RecoveryReviewStage::CodeReview
                    } else {
                        RecoveryReviewStage::PlanReview
                    },
                    reviewer: RecoveryReviewer {
                        profile: resolved.profile,
                        model: resolved
                            .model
                            .context("reviewer effective model is unknown")?,
                        vendor: resolved.vendor.context("reviewer vendor is unknown")?,
                    },
                }
            }
            Kind::AcceptPlan => RecoveryAction::AcceptPlan {
                waive_independent_review: exceptions
                    .contains(&RecoveryCheck::IndependentPlanReview),
                resolution_notes_sha256: corral_core::agent::paths::sha256_hex(
                    self.notes.as_bytes(),
                ),
            },
            Kind::AcceptCode => RecoveryAction::AcceptCode {
                merge: match param {
                    "rebase" => RecoveryMerge::Rebase,
                    "ff" => RecoveryMerge::Ff,
                    _ => RecoveryMerge::Keep,
                },
            },
            Kind::ForceAdmission => {
                let stage = match param {
                    "plan-dispatch" => RecoveryAdmissionStage::PlanDispatch,
                    "gate" => RecoveryAdmissionStage::Gate,
                    "code-review" => RecoveryAdmissionStage::CodeReview,
                    "plan-review" => RecoveryAdmissionStage::PlanReview,
                    _ => RecoveryAdmissionStage::Dispatch,
                };
                return Ok(RecoveryInput {
                    exceptions,
                    ..recovery::recover_resource_input(ctx, stage)?
                });
            }
        };
        Ok(RecoveryInput {
            action,
            exceptions,
            notes: if kind == Kind::AcceptPlan {
                self.notes.clone()
            } else {
                String::new()
            },
        })
    }

    fn evaluate(&self, ctx: &Ctx) -> Result<(RecoveryInput, RecoveryPreview)> {
        let input = self.input(ctx)?;
        let preview = recovery::recover_preview(ctx, &self.task, &input)?;
        Ok((input, preview))
    }

    /// Re-derives the read-only preview. Nothing is persisted.
    pub(crate) fn refresh_preview(&mut self, ctx: &Ctx) {
        let result = self
            .evaluate(ctx)
            .map(|(_, p)| p)
            .map_err(|e| format!("{e:#}"));
        if let Ok(preview) = &result {
            self.offered = preview
                .checks
                .iter()
                .filter(|c| {
                    matches!(
                        c.disposition,
                        CheckDisposition::Overridden | CheckDisposition::Waived
                    )
                })
                .map(|c| c.check)
                .collect();
            let offered = self.offered.clone();
            self.acknowledged.retain(|c| offered.contains(c));
        }
        self.preview = Some(result);
    }

    pub(crate) fn controls(&self) -> Vec<Control> {
        if self.step == Step::Confirm {
            return vec![Control::Edit, Control::Cancel, Control::Confirm];
        }
        let kind = self.kind();
        let mut out = vec![Control::Kind];
        if !kind.params().is_empty() {
            out.push(Control::Param);
        }
        if kind == Kind::UseReviewer {
            out.push(Control::Reviewer);
        }
        out.extend((0..self.offered.len()).map(Control::Check));
        if kind == Kind::AcceptPlan {
            out.push(Control::Notes);
        }
        out.extend([Control::Reason, Control::Cancel, Control::Continue]);
        out
    }

    pub(crate) fn focused(&self) -> Control {
        let controls = self.controls();
        controls[self.focus.min(controls.len() - 1)]
    }

    fn position(&self, control: Control) -> usize {
        self.controls()
            .iter()
            .position(|c| *c == control)
            .unwrap_or(0)
    }

    fn move_focus(&mut self, delta: isize) {
        let n = self.controls().len() as isize;
        self.focus = (self.focus as isize + delta).rem_euclid(n) as usize;
        self.follow = true;
    }

    fn cycle(&mut self, control: Control, delta: isize, ctx: &Ctx) {
        let step = |value: usize, n: usize| {
            if n == 0 {
                0
            } else {
                (value as isize + delta).rem_euclid(n as isize) as usize
            }
        };
        match control {
            Control::Kind => self.set_kind(step(self.kind, KINDS.len()), ctx),
            Control::Param => self.set_param(step(self.param, self.kind().params().len()), ctx),
            Control::Reviewer => self.set_reviewer(step(self.reviewer, self.reviewers.len()), ctx),
            _ => {}
        }
    }

    pub(crate) fn set_kind(&mut self, kind: usize, ctx: &Ctx) {
        if kind >= KINDS.len() {
            return;
        }
        // Each action has its own exception list: acknowledgements never carry across.
        self.kind = kind;
        self.param = 0;
        self.acknowledged.clear();
        self.refresh_preview(ctx);
        self.focus = self.position(Control::Kind);
    }

    fn set_param(&mut self, param: usize, ctx: &Ctx) {
        if param < self.kind().params().len() {
            self.param = param;
            self.acknowledged.clear();
            self.refresh_preview(ctx);
        }
    }

    fn set_reviewer(&mut self, reviewer: usize, ctx: &Ctx) {
        if reviewer < self.reviewers.len() {
            self.reviewer = reviewer;
            self.acknowledged.clear();
            self.refresh_preview(ctx);
        }
    }

    fn toggle(&mut self, i: usize, ctx: &Ctx) {
        if let Some(check) = self.offered.get(i).copied() {
            if !self.acknowledged.remove(&check) {
                self.acknowledged.insert(check);
            }
            self.refresh_preview(ctx);
        }
    }

    /// Continue: only a clean, current preview with an owner-written reason reaches confirmation.
    fn proceed(&mut self, ctx: &Ctx) -> Action {
        self.refresh_preview(ctx);
        let problem = match &self.preview {
            Some(Err(e)) => Some(format!("Preview failed: {e}")),
            None => Some("No preview".into()),
            Some(Ok(p)) if !p.remaining_blockers.is_empty() => Some(format!(
                "{} hard blocker(s) remain; recovery cannot proceed",
                p.remaining_blockers.len()
            )),
            Some(Ok(_)) if self.acknowledged.len() != self.offered.len() => {
                Some("Acknowledge each listed exception (space) first".into())
            }
            Some(Ok(_)) if self.reason.trim().is_empty() => {
                Some("Write the owner reason first".into())
            }
            Some(Ok(_)) if self.kind() == Kind::AcceptPlan && self.notes.trim().is_empty() => {
                Some("Write the owner-decision resolution notes first".into())
            }
            Some(Ok(_)) => None,
        };
        match problem {
            Some(text) => self.notice = Some((Tone::Warn, text)),
            None => {
                self.notice = None;
                self.step = Step::Confirm;
                self.focus = self.position(Control::Cancel);
                self.scroll = ui::Scroll::default();
                self.follow = true;
            }
        }
        Action::None
    }

    /// Re-derives the preview; a changed proposal returns to editing instead of applying.
    fn confirm(&mut self, ctx: &Ctx) -> Action {
        let shown = match &self.preview {
            Some(Ok(p)) => p.clone(),
            _ => return self.back_to_edit("Proposal unavailable; nothing was applied".into()),
        };
        let (input, mut fresh) = match self.evaluate(ctx) {
            Ok(v) => v,
            Err(e) => return self.stale(ctx, format!("Proposal refresh failed: {e:#}")),
        };
        let expires = fresh.expires;
        fresh.expires = shown.expires;
        if fresh != shown {
            return self.stale(
                ctx,
                "Task, policy, checks or effect changed; review the refreshed proposal and confirm again".into(),
            );
        }
        fresh.expires = expires;
        // Defense in depth: confirmation never forwards a blocked or unacknowledged proposal.
        if !shown.remaining_blockers.is_empty()
            || self.acknowledged.len() != self.offered.len()
            || self.reason.trim().is_empty()
        {
            return self.back_to_edit(
                "Hard blockers remain or the proposal is incomplete; nothing was applied".into(),
            );
        }
        match (self.authorize)(ctx, &fresh, &input, self.reason.trim()) {
            Ok(record) => {
                let state = record.outcomes.last().map_or("authorized".into(), |o| {
                    format!("{:?}", o.state).to_lowercase()
                });
                Action::BackWithToast(
                    Toast::new(
                        Tone::Info,
                        format!("{} owner recovery {state}", self.task),
                        format!("{} · {}", record.id, labels::not_policy()),
                    )
                    .about(self.task.clone()),
                )
            }
            Err(e) => self.back_to_edit(format!("Not applied: {e:#}")),
        }
    }

    fn stale(&mut self, ctx: &Ctx, text: String) -> Action {
        self.refresh_preview(ctx);
        self.back_to_edit(text)
    }

    fn back_to_edit(&mut self, text: String) -> Action {
        self.step = Step::Edit;
        self.notice = Some((Tone::Warn, text));
        self.focus = self.position(Control::Cancel);
        self.follow = true;
        Action::None
    }

    fn activate(&mut self, control: Control, ctx: &Ctx) -> Action {
        match control {
            Control::Kind | Control::Param | Control::Reviewer => {
                self.cycle(control, 1, ctx);
                Action::None
            }
            Control::Check(i) => {
                self.toggle(i, ctx);
                Action::None
            }
            Control::Notes | Control::Reason => {
                self.move_focus(1);
                Action::None
            }
            Control::Cancel => Action::Back,
            Control::Continue => self.proceed(ctx),
            Control::Edit => {
                self.step = Step::Edit;
                self.focus = self.position(Control::Reason);
                self.follow = true;
                Action::None
            }
            Control::Confirm => self.confirm(ctx),
        }
    }

    fn text_mut(&mut self, control: Control) -> Option<&mut String> {
        match control {
            Control::Notes => Some(&mut self.notes),
            Control::Reason => Some(&mut self.reason),
            _ => None,
        }
    }

    pub(crate) fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if !ui::is_press(&key) {
            return Action::None;
        }
        let focused = self.focused();
        match key.code {
            KeyCode::Esc if self.step == Step::Confirm => self.activate(Control::Edit, ctx),
            KeyCode::Esc => Action::Back,
            KeyCode::Tab | KeyCode::Down => {
                self.move_focus(1);
                Action::None
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.move_focus(-1);
                Action::None
            }
            KeyCode::PageDown | KeyCode::PageUp => {
                self.follow = false;
                self.scroll.on_key(&key, self.page);
                Action::None
            }
            KeyCode::Left | KeyCode::Right
                if matches!(focused, Control::Kind | Control::Param | Control::Reviewer) =>
            {
                self.cycle(focused, if key.code == KeyCode::Left { -1 } else { 1 }, ctx);
                Action::None
            }
            KeyCode::Left => {
                self.move_focus(-1);
                Action::None
            }
            KeyCode::Right => {
                self.move_focus(1);
                Action::None
            }
            // Enter in a text field moves on; it never applies.
            KeyCode::Enter => self.activate(focused, ctx),
            KeyCode::Char(' ') if self.text_mut(focused).is_none() => self.activate(focused, ctx),
            KeyCode::Backspace => {
                if let Some(text) = self.text_mut(focused) {
                    text.pop();
                }
                Action::None
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(text) = self.text_mut(focused) {
                    text.clear();
                }
                Action::None
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(text) = self.text_mut(focused)
                    && text.chars().count() < 2000
                {
                    text.push(c);
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    pub(crate) fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if let Some(dir) = hit::wheel(&ev) {
            self.follow = false;
            self.scroll.wheel(dir);
            return Action::None;
        }
        if !hit::left(&ev) {
            return Action::None;
        }
        match m.hit {
            Some(Hit::Item(Z_KIND, i)) => {
                self.set_kind(i, m.ctx);
                Action::None
            }
            Some(Hit::Item(Z_PARAM, i)) => {
                self.focus = self.position(Control::Param);
                self.set_param(i, m.ctx);
                Action::None
            }
            Some(Hit::Item(Z_REVIEWER, i)) => {
                self.focus = self.position(Control::Reviewer);
                self.set_reviewer(i, m.ctx);
                Action::None
            }
            Some(Hit::Item(Z_ROW, i)) => {
                let Some(control) = self.controls().get(i).copied() else {
                    return Action::None;
                };
                self.focus = i;
                match control {
                    Control::Check(_)
                    | Control::Cancel
                    | Control::Continue
                    | Control::Edit
                    | Control::Confirm => self.activate(control, m.ctx),
                    _ => Action::None,
                }
            }
            _ => Action::None,
        }
    }

    pub(crate) fn captures_text(&self) -> bool {
        matches!(self.focused(), Control::Notes | Control::Reason)
    }

    pub(crate) fn has_unsaved_text(&self) -> bool {
        !self.reason.is_empty() || !self.notes.is_empty()
    }

    /// Periodic refresh: a changed task while confirming returns to editing with the draft kept.
    pub(crate) fn refresh(&mut self, ctx: &Ctx) {
        if self.step != Step::Confirm {
            return;
        }
        let Some(Ok(shown)) = self.preview.clone() else {
            return;
        };
        match self.evaluate(ctx) {
            Ok((_, mut fresh)) => {
                fresh.expires = shown.expires;
                if fresh != shown {
                    self.stale(
                        ctx,
                        "Proposal changed while confirming; review it again".into(),
                    );
                }
            }
            Err(e) => {
                self.stale(ctx, format!("Proposal refresh failed: {e:#}"));
            }
        }
    }

    pub(crate) fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("⇥", "next"),
            ("space", "select"),
            ("⏎", "activate"),
            ("pgdn", "scroll"),
            (
                "esc",
                if self.step == Step::Confirm {
                    "edit"
                } else {
                    "cancel"
                },
            ),
        ]
    }
}
