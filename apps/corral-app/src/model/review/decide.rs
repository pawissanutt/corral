//! Decisions on a candidate: the decide bar, the lines that say where a sent operation stands,
//! and the reducer that turns every served control into an engine operation, a detail fetch or
//! a navigation. Every operation stays pressable; the engine decides and the app shows its
//! refusal with the typed next steps.

use corral_protocol::detail::{DetailGet, DetailRef};
use corral_types::Op;

use crate::model::act::{self, Act, ActStatus};
use crate::model::daemon::Feed;
use crate::model::effect::{Effect, Env};
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::live::{Live, Selection};
use crate::model::nav::Form;
use crate::model::projects::{self, ProjectFilter};
use crate::model::text::span;

use super::live::{Shown, artifact_ref, file_ref, review_ref, selected_filtered, sha7, task_ref};
use super::view::{Button, ButtonStyle, DecideBar, StatusLine, Target, button};
use super::{Action, Sent, State};
use crate::model::work::Verb;

fn status_line(feed: &Feed, sent: &Sent) -> Option<StatusLine> {
    let status = act::status(feed, &sent.key)?;
    let hue = match &status {
        ActStatus::Done => return None,
        ActStatus::Pending | ActStatus::Running => Hue::Corral,
        ActStatus::Refused(_) => Hue::Held,
        ActStatus::Failed(_) => Hue::Fail,
    };
    Some(StatusLine {
        text: vec![
            span(sent.title.clone()).bold(),
            span(" · "),
            span(status.line()),
        ],
        hue,
        actions: if status.settled() {
            vec![button(
                "Dismiss",
                Action::Dismiss(sent.key.clone()),
                ButtonStyle::Quiet,
            )]
        } else {
            Vec::new()
        },
    })
}

/// The Onto field of the shown candidate: what was typed for it, else the run's base, with the
/// project's local branches it offers for that.
fn target(state: &State, feed: &Feed, shown: &Shown) -> Target {
    let base = shown.base();
    let value = state
        .onto
        .get(shown.id())
        .cloned()
        .or_else(|| base.map(str::to_owned))
        .unwrap_or_default();
    let project = shown.task.task().project.as_deref();
    // The candidate's own branch is never a target: the engine refuses landing it onto itself.
    let mut branches = projects::branches(feed, project);
    branches.retain(|branch| Some(branch.as_str()) != shown.branch());
    Target {
        label: "Onto",
        branches: projects::suggest(&branches, &value),
        value,
        placeholder: "Branch to land onto",
        hint: match base {
            Some(base) => format!(
                "The branch approving lands onto: {base}, the run's base, unless you name another existing branch."
            ),
            None => "The branch approving lands onto; empty lands onto the run's base.".into(),
        },
    }
}

/// The branch approving names: the typed one when it is not the run's base, which the engine
/// takes without being told.
fn onto(state: &State, shown: &Shown) -> Option<String> {
    let typed = state.onto.get(shown.id())?.trim();
    (!typed.is_empty() && Some(typed) != shown.base()).then(|| typed.to_owned())
}

/// The decide bar of the shown candidate.
pub(super) fn decide_bar(state: &State, feed: &Feed, shown: &Shown, env: Env) -> DecideBar {
    let phone = env.form == Form::Phone;
    let id = shown.id();
    let onto = onto(state, shown);
    let approve = button(
        act::label(&approve_op(id, onto.clone()), false),
        Action::Approve,
        ButtonStyle::Primary,
    );
    let changes = Button {
        short: Some("Changes"),
        ..button(
            act::label(
                &Op::Changes {
                    id: id.to_string(),
                    text: String::new(),
                },
                false,
            ),
            Action::Verb(Verb::Changes),
            ButtonStyle::Ghost,
        )
    };
    let ask = button("Ask", Action::AskCandidate, ButtonStyle::Quiet);
    let mut secondary = vec![
        button(
            act::label(&Op::Gate { id: id.to_string() }, false),
            Action::Gate,
            ButtonStyle::Ghost,
        ),
        button(
            act::label(
                &Op::Review {
                    id: id.to_string(),
                    profile: None,
                },
                false,
            ),
            Action::StartReview,
            ButtonStyle::Ghost,
        ),
    ];
    let refused = state.sent.iter().any(|sent| {
        sent.task == *id
            && sent.approve
            && matches!(act::status(feed, &sent.key), Some(ActStatus::Refused(_)))
    });
    if shown.gate_red() || refused {
        let land = approve_op(id, onto.clone());
        secondary.push(Button {
            hint: act::skips(&land).map(|skips| format!("Skips {skips}.")),
            ..button(
                act::label(&land, true),
                Action::LandAnyway,
                ButtonStyle::Ghost,
            )
        });
    }
    let target = target(state, feed, shown);
    let device = feed
        .session
        .as_ref()
        .map(|session| session.device.name.as_str());
    DecideBar {
        buttons: if phone {
            vec![changes, approve, ask]
        } else {
            vec![approve, changes, ask]
        },
        secondary,
        explanation: (!phone).then(|| {
            let mut line = vec![span("Approving rebases ")];
            line.push(span(shown.head().map_or_else(|| "the task branch".into(), sha7)).mono());
            line.push(span(if target.value.trim().is_empty() {
                " onto its base, fast-forwards it and removes the worktree.".into()
            } else {
                format!(
                    " onto {}, fast-forwards it and removes the worktree.",
                    target.value.trim()
                )
            }));
            line
        }),
        target,
        attribution: device
            .filter(|_| !phone)
            .map(|device| vec![span(format!("actor=owner · via=app:{device}")).mono().dim()]),
        status: state
            .sent
            .iter()
            .filter(|sent| sent.task == *id)
            .filter_map(|sent| status_line(feed, sent))
            .collect(),
    }
}

fn approve_op(id: &Id, onto: Option<String>) -> Op {
    Op::Approve {
        id: id.to_string(),
        onto,
    }
}

fn fetch(feed: &Feed, target: DetailRef) -> Effect {
    let etag = feed
        .detail(&target)
        .and_then(|cache| cache.pages.first())
        .map(|page| page.etag.clone());
    Effect::Detail(DetailGet {
        target,
        etag,
        page: None,
    })
}

impl State {
    /// Revalidates the candidate's detail, its task, its gate log and every patch and artifact
    /// shown for it.
    fn refresh(&self, feed: &Feed, shown: &Shown) -> Vec<Effect> {
        let task = shown.id();
        let mut effects = vec![fetch(feed, review_ref(task)), fetch(feed, task_ref(task))];
        effects.extend(self.gate_log(feed, shown));
        effects.extend(
            self.expanded
                .iter()
                .filter(|(owner, _)| owner == task)
                .map(|(_, path)| fetch(feed, file_ref(task, path))),
        );
        effects.extend(
            self.opened
                .iter()
                .filter(|(owner, _)| owner == task)
                .map(|(_, reference)| fetch(feed, artifact_ref(reference))),
        );
        effects
    }

    /// The gate log's artifact while the Gate log tab shows it.
    fn gate_log(&self, feed: &Feed, shown: &Shown) -> Option<Effect> {
        if self.tab != super::Tab::Log {
            return None;
        }
        let reference = shown.check(corral_types::CheckKind::Gate)?.log.as_deref()?;
        Some(fetch(feed, artifact_ref(reference)))
    }

    fn send(&mut self, task: &Id, op: Op, force: bool) -> Vec<Effect> {
        let mut act = Act::new(&mut self.ids, op, force);
        act.about = Some(Live::Review(task.clone()));
        self.sent.push(Sent {
            key: act.key.clone(),
            task: task.clone(),
            title: act.title.clone(),
            approve: matches!(act.op, Op::Approve { .. }) && !force,
        });
        vec![Effect::Act(act)]
    }

    /// Applies a served control to the pushed `focus`, or the root's selection in list mode.
    /// `filter` scopes list selection only; a pushed page keeps its explicit target.
    pub fn apply_live_filtered(
        &mut self,
        action: Action,
        feed: &Feed,
        selection: &Selection,
        focus: Option<&Id>,
        filter: &ProjectFilter,
    ) -> Vec<Effect> {
        match &action {
            Action::Layout(layout) => return self.rlayout(*layout),
            Action::Dismiss(key) => {
                self.sent.retain(|sent| sent.key != *key);
                return Vec::new();
            }
            _ => {}
        }
        let task = match focus {
            Some(id) => feed.board.task(id),
            None => selected_filtered(selection, feed, filter),
        };
        let Some(task) = task else {
            return Vec::new();
        };
        let shown = Shown::of(feed, task);
        if let Some(effects) = super::outline::navigation(&action, &shown) {
            return effects;
        }
        let id = shown.id().clone();
        match action {
            Action::Tab(tab) => {
                self.rtab(tab);
                self.gate_log(feed, &shown).into_iter().collect()
            }
            Action::Approve => self.send(&id, approve_op(&id, onto(self, &shown)), false),
            Action::LandAnyway => self.send(&id, approve_op(&id, onto(self, &shown)), true),
            Action::Onto(words) => {
                self.onto.insert(id, words);
                Vec::new()
            }
            Action::Gate => self.send(&id, Op::Gate { id: id.to_string() }, false),
            Action::StartReview => self.send(
                &id,
                Op::Review {
                    id: id.to_string(),
                    profile: None,
                },
                false,
            ),
            Action::Expand(path) => {
                let effect = fetch(feed, file_ref(&id, &path));
                self.expanded.insert((id, path));
                vec![effect]
            }
            Action::Collapse(path) => {
                self.expanded.remove(&(id, path));
                Vec::new()
            }
            Action::AskAbout { path, hunk } => vec![Effect::AskAbout {
                context: Some(Live::Review(id).context(None)),
                text: Some(match hunk {
                    Some(hunk) => format!("{path} {hunk}: "),
                    None => format!("{path}: "),
                }),
            }],
            Action::Refresh => self.refresh(feed, &shown),
            Action::OpenArtifact(reference) => {
                let effect = fetch(feed, artifact_ref(&reference));
                self.opened.insert((id, reference));
                vec![effect]
            }
            Action::CloseArtifact(reference) => {
                self.opened.remove(&(id, reference));
                Vec::new()
            }
            // A shared verb runs through the work state (the screen routes it there); the
            // remaining controls were answered above.
            Action::Verb(_)
            | Action::Layout(_)
            | Action::Dismiss(_)
            | Action::AskCandidate
            | Action::OpenSession
            | Action::OpenReviewer => Vec::new(),
        }
    }
}
