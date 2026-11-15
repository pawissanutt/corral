//! Served decisions and comments: the decide bar, the guards that keep each command bound to
//! the shown candidate, the lines that say where a sent command stands, and the reducer that
//! turns every served control into a canonical command, a detail fetch or a navigation.

use corral_protocol::cmd::Command;
use corral_protocol::cmd::annotation::{AnnotationCommand, AnnotationSide, LineRange};
use corral_protocol::cmd::review::ReviewCommand;
use corral_protocol::cmd::task::TaskCommand;
use corral_protocol::detail::{DetailGet, DetailRef};

use crate::model::act::{self, Act, ActStatus, Authority};
use crate::model::board::Mark;
use crate::model::daemon::{Feed, Link};
use crate::model::effect::{Effect, Env};
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::live::{Live, Selection};
use crate::model::nav::Form;
use crate::model::text::span;

use super::evidence::artifact_ref;
use super::live::{Shown, file_ref, review_ref, selected, short, task_ref};
use super::live_files::{draft_key, patch_refusal};
use super::view::{Button, ButtonStyle, Composer, DecideBar, StatusLine, button};
use super::{Action, Compose, Intent, Sent, State};

/// Why no command may be sent from this device right now, before any candidate check.
fn authority_refusal(feed: &Feed, env: Env, authority: Authority) -> Option<String> {
    if env.conn.refuses() || feed.link != Link::Up {
        return Some("Refused: needs a live connection. Nothing was sent.".into());
    }
    match &feed.session {
        None => Some("Waiting for the app session; nothing was sent.".into()),
        Some(session)
            if match authority {
                Authority::Reply => !session.authority.reply,
                Authority::Act => !session.authority.act,
                Authority::Assistant => !session.authority.assistant,
                Authority::Admin => !session.authority.admin,
            } =>
        {
            Some(authority.refusal().into())
        }
        Some(_) => None,
    }
}

fn device_refusal(feed: &Feed, env: Env) -> Option<String> {
    authority_refusal(feed, env, Authority::Act)
}

pub(super) fn reply_refusal(feed: &Feed, env: Env) -> Option<String> {
    authority_refusal(feed, env, Authority::Reply)
}

/// Why a decision on the shown candidate would not be bound to it.
fn candidate_refusal(shown: &Shown) -> Option<String> {
    if shown.review.is_none() {
        return Some("The candidate's detail has not loaded; nothing was sent.".into());
    }
    if shown
        .review_cache
        .is_some_and(|cache| cache.stale || cache.loading || cache.error.is_some())
    {
        return Some("The candidate detail is last known. Refresh it before deciding.".into());
    }
    if shown.superseded() {
        return Some("A newer candidate was recorded; decide after the detail refreshes.".into());
    }
    if shown.detail.is_none() {
        return Some("Waiting for the task's owner revision; nothing was sent.".into());
    }
    None
}

/// Why a comment cannot be anchored to the shown candidate.
pub(super) fn comment_refusal(feed: &Feed, shown: &Shown, env: Env) -> Option<String> {
    device_refusal(feed, env)
        .or_else(|| candidate_refusal(shown))
        .or_else(|| {
            shown.run().is_none().then(|| {
                "The candidate's run is not known, so the comment cannot be anchored.".into()
            })
        })
        .or_else(|| {
            shown
                .review
                .and_then(|review| review.base.as_ref())
                .is_none()
                .then(|| {
                    "corrald reports no base for this candidate, so the comment cannot be anchored."
                        .into()
                })
        })
}

fn in_flight(state: &State, feed: &Feed, task: &Id, what: impl Fn(&Intent) -> bool) -> bool {
    state.sent.iter().any(|sent| {
        sent.task == *task
            && what(&sent.intent)
            && act::status(feed, &sent.key).is_some_and(|status| !status.settled())
    })
}

fn approve_refusal(state: &State, feed: &Feed, shown: &Shown, env: Env) -> Option<String> {
    device_refusal(feed, env)
        .or_else(|| candidate_refusal(shown))
        .or_else(|| {
            let review = shown.review?;
            (!review.approvable).then(|| {
                format!(
                    "Approval is unavailable in state {}.",
                    review.state.replace('_', " ")
                )
            })
        })
        .or_else(|| {
            in_flight(state, feed, shown.id(), |intent| {
                matches!(intent, Intent::Approve { .. })
            })
            .then(|| "Approval was already sent; its result is shown below.".into())
        })
}

fn intent_label(intent: &Intent) -> String {
    match intent {
        Intent::Approve { sha } => format!("Approve {}", short(sha)),
        Intent::Changes { .. } => "Request changes".into(),
        Intent::Retry => "Retry".into(),
        Intent::Discard => "Discard".into(),
        Intent::Comment { at, .. } => format!("Comment on line {}", at.line),
        Intent::Reply { .. } => "Reply".into(),
        Intent::Resolve { .. } => "Resolve".into(),
        Intent::Reopen { .. } => "Reopen".into(),
    }
}

/// The unsent note an intent carries and the composer it belongs to.
fn note(sent: &Sent) -> Option<(String, &str)> {
    match &sent.intent {
        Intent::Changes { text } => Some((sent.candidate.as_ref()?.draft("changes"), text)),
        Intent::Comment { at, text } => Some((draft_key(sent.candidate.as_ref()?, at), text)),
        Intent::Reply {
            candidate,
            thread,
            text,
        } => Some((candidate.draft(&format!("reply:{thread}")), text)),
        _ => None,
    }
}

fn status_line(feed: &Feed, sent: &Sent) -> Option<StatusLine> {
    let Some(status) = act::status(feed, &sent.key) else {
        // A confirm-class note waits for this device's confirmation; a cancelled sheet leaves
        // the owner's words here to edit again.
        let Intent::Changes { text } = &sent.intent else {
            return None;
        };
        return Some(StatusLine {
            text: vec![
                span(intent_label(&sent.intent)).bold(),
                span(" · "),
                span("Not sent"),
            ],
            hue: Hue::Held,
            actions: vec![
                button(
                    "Edit again",
                    Action::Restore {
                        key: sent.key.clone(),
                        draft: sent.candidate.as_ref()?.draft("changes"),
                        text: text.clone(),
                    },
                    ButtonStyle::Quiet,
                ),
                button(
                    "Dismiss",
                    Action::Dismiss(sent.key.clone()),
                    ButtonStyle::Quiet,
                ),
            ],
        });
    };
    let (hue, actions) = match &status {
        ActStatus::Done => return None,
        ActStatus::Previewing
        | ActStatus::Confirming
        | ActStatus::Queued
        | ActStatus::Pending
        | ActStatus::Running => (Hue::Corral, Vec::new()),
        ActStatus::Unknown => (
            Hue::Held,
            vec![button(
                "Check again",
                Action::Recover(sent.key.clone()),
                ButtonStyle::Quiet,
            )],
        ),
        ActStatus::Failed(_)
        | ActStatus::Refused(_)
        | ActStatus::Conflict(_)
        | ActStatus::Abandoned => {
            let mut actions = Vec::new();
            if let Some((draft, text)) = note(sent) {
                actions.push(button(
                    "Edit again",
                    Action::Restore {
                        key: sent.key.clone(),
                        draft,
                        text: text.to_owned(),
                    },
                    ButtonStyle::Quiet,
                ));
            }
            actions.push(button(
                "Dismiss",
                Action::Dismiss(sent.key.clone()),
                ButtonStyle::Quiet,
            ));
            (
                if matches!(status, ActStatus::Abandoned | ActStatus::Conflict(_)) {
                    Hue::Held
                } else {
                    Hue::Fail
                },
                actions,
            )
        }
    };
    Some(StatusLine {
        text: vec![
            span(intent_label(&sent.intent)).bold(),
            span(" · "),
            span(status.line()),
        ],
        hue,
        actions,
    })
}

/// Lines for the commands this page sent about `task` that `concerns` selects.
pub(super) fn status_lines(
    state: &State,
    feed: &Feed,
    task: &Id,
    concerns: impl Fn(&Intent) -> bool,
) -> Vec<StatusLine> {
    state
        .sent
        .iter()
        .filter(|sent| sent.task == *task && concerns(&sent.intent))
        .filter_map(|sent| status_line(feed, sent))
        .collect()
}

fn decision(intent: &Intent) -> bool {
    matches!(
        intent,
        Intent::Approve { .. } | Intent::Changes { .. } | Intent::Retry | Intent::Discard
    )
}

fn refused(mut control: Button, refusal: Option<String>) -> Button {
    control.refusal = refusal;
    control
}

/// The decide bar of the shown candidate; without one, the bar stays empty.
pub(super) fn decide_bar(state: &State, feed: &Feed, shown: Option<&Shown>, env: Env) -> DecideBar {
    let phone = env.form == Form::Phone;
    let Some(shown) = shown else {
        return DecideBar {
            buttons: Vec::new(),
            secondary: Vec::new(),
            explanation: None,
            attribution: None,
            composer: None,
            status: Vec::new(),
        };
    };
    let sha = shown.sha().map(short).unwrap_or("candidate");
    let approve = refused(
        button(
            format!("Approve {sha}"),
            Action::Approve,
            ButtonStyle::Primary,
        ),
        approve_refusal(state, feed, shown, env),
    );
    let changes = button(
        if phone { "Changes" } else { "Request changes" },
        Action::RequestChanges,
        ButtonStyle::Ghost,
    );
    let decision_refusal = device_refusal(feed, env).or_else(|| candidate_refusal(shown));
    let mut secondary = Vec::new();
    if shown.task.rail[..3]
        .iter()
        .any(|step| matches!(step.mark, Mark::Fail | Mark::Stale))
    {
        secondary.push(refused(
            button("Retry", Action::Retry, ButtonStyle::Ghost),
            decision_refusal.clone(),
        ));
    }
    secondary.push(refused(
        button("Discard", Action::Discard, ButtonStyle::Quiet),
        decision_refusal.clone(),
    ));
    let device = feed
        .session
        .as_ref()
        .map(|session| session.device.name.as_str());
    DecideBar {
        buttons: if phone {
            vec![changes, approve]
        } else {
            vec![approve, changes]
        },
        secondary,
        explanation: (!phone).then(|| {
            vec![
                span("Verdicts apply to "),
                span(sha.to_owned()).mono(),
                span(" only; a new candidate makes them stale. Approving needs a fresh confirm."),
            ]
        }),
        attribution: device
            .filter(|_| !phone)
            .map(|device| vec![span(format!("actor=owner · via=app:{device}")).mono().dim()]),
        composer: state.changes.as_ref().filter(|at| at.task == *shown.id()).map(|at| Composer {
            draft: at.draft("changes"),
            target: Compose::Changes(at.clone()),
            placeholder: "What should the worker change? This goes to the worker as the next round's input.",
            label: "Request changes",
            send: "Send to worker",
            cancel: Some(button("Cancel", Action::CancelCompose, ButtonStyle::Quiet)),
            refusal: if shown.matches(at) { decision_refusal } else {
                Some("This note belongs to an older candidate. Reopen Request changes for the current one.".into())
            },
        }),
        status: status_lines(state, feed, shown.id(), decision),
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

fn toast(text: impl Into<String>) -> Vec<Effect> {
    vec![Effect::Toast(text.into())]
}

impl State {
    /// Revalidates the candidate's detail, its task and every patch and artifact shown for it.
    fn refresh(&self, feed: &Feed, task: &Id) -> Vec<Effect> {
        let mut effects = vec![fetch(feed, review_ref(task)), fetch(feed, task_ref(task))];
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
                .map(|(_, key)| fetch(feed, artifact_ref(task, key))),
        );
        effects
    }

    fn send(&mut self, shown: &Shown, intent: Intent, act: Act) -> Vec<Effect> {
        self.sent.push(Sent {
            key: act.key.clone(),
            task: shown.id().clone(),
            candidate: shown.identity(),
            intent,
        });
        vec![Effect::Act(act.about(Live::Review(shown.id().clone())))]
    }

    fn annotate(
        &mut self,
        shown: &Shown,
        intent: Intent,
        verb: &str,
        build: impl FnOnce(String) -> AnnotationCommand,
    ) -> Vec<Effect> {
        let mut act = Act::new(
            &mut self.ids,
            format!(
                "{verb} on {} {}",
                shown.id(),
                shown.sha().map(short).unwrap_or_default()
            ),
            Command::Annotation(AnnotationCommand::Other),
        );
        act.command = Command::Annotation(build(act.key.to_string()));
        self.send(shown, intent, act)
    }

    fn submit(
        &mut self,
        target: Compose,
        text: String,
        feed: &Feed,
        shown: &Shown,
        env: Env,
    ) -> Vec<Effect> {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return toast("Write the note first; nothing was sent.");
        }
        let task = shown.id().clone();
        match target {
            Compose::Changes(candidate) => {
                if !shown.matches(&candidate) {
                    return toast(
                        "The candidate changed. Reopen Request changes; nothing was sent.",
                    );
                }
                let refusal = device_refusal(feed, env).or_else(|| candidate_refusal(shown));
                let (Some(expect), None) = (shown.expect(), refusal.clone()) else {
                    return toast(refusal.unwrap_or_default());
                };
                self.changes = None;
                let act = Act::new(
                    &mut self.ids,
                    format!("Request changes on {task}"),
                    Command::Review(ReviewCommand::RequestChanges {
                        id: task.to_string(),
                        text: text.clone(),
                        escalate: false,
                    }),
                )
                .expect(expect);
                self.send(shown, Intent::Changes { text }, act)
            }
            Compose::New { candidate, at } => {
                if !shown.matches(&candidate) {
                    return toast(
                        "The candidate changed. Reopen a line on the current patch; nothing was sent.",
                    );
                }
                if let Some(refusal) = comment_refusal(feed, shown, env)
                    .or_else(|| patch_refusal(feed, shown, &at.path))
                {
                    return toast(refusal);
                }
                let corral_protocol::detail::ReviewCandidate { run, base, head } =
                    candidate.candidate;
                self.composing = None;
                let intent = Intent::Comment {
                    at: at.clone(),
                    text: text.clone(),
                };
                self.annotate(shown, intent, "Comment", move |key| {
                    AnnotationCommand::Open {
                        id: task.to_string(),
                        key,
                        expected_run: run,
                        base,
                        head,
                        path: at.path,
                        side: if at.old_side {
                            AnnotationSide::Old
                        } else {
                            AnnotationSide::New
                        },
                        range: Some(LineRange {
                            start: at.line,
                            end: at.line,
                        }),
                        text,
                    }
                })
            }
            Compose::Reply {
                candidate,
                thread,
                revision,
            } => {
                if candidate.task != task {
                    return toast(
                        "This reply belongs to another task. Reopen its discussion; nothing was sent.",
                    );
                }
                if let Some(refusal) = reply_refusal(feed, env) {
                    return toast(refusal);
                }
                let intent = Intent::Reply {
                    thread: thread.clone(),
                    text: text.clone(),
                    candidate,
                };
                self.annotate(shown, intent, "Reply", move |key| {
                    AnnotationCommand::Reply {
                        id: task.to_string(),
                        key,
                        thread: thread.to_string(),
                        expected_revision: revision,
                        text,
                    }
                })
            }
        }
    }

    fn thread_state(
        &mut self,
        feed: &Feed,
        shown: &Shown,
        env: Env,
        thread: Id,
        revision: u32,
        reopen: bool,
    ) -> Vec<Effect> {
        if let Some(refusal) = device_refusal(feed, env) {
            return toast(refusal);
        }
        let task = shown.id().to_string();
        let name = thread.to_string();
        if reopen {
            self.annotate(
                shown,
                Intent::Reopen { thread },
                "Reopen thread",
                move |key| AnnotationCommand::Reopen {
                    id: task,
                    key,
                    thread: name,
                    expected_revision: revision,
                    text: String::new(),
                },
            )
        } else {
            self.annotate(
                shown,
                Intent::Resolve { thread },
                "Resolve thread",
                move |key| AnnotationCommand::Resolve {
                    id: task,
                    key,
                    thread: name,
                    expected_revision: revision,
                    text: String::new(),
                },
            )
        }
    }

    fn decide(&mut self, action: &Action, feed: &Feed, shown: &Shown, env: Env) -> Vec<Effect> {
        let task = shown.id().clone();
        let sha = shown.sha().map(short).unwrap_or_default().to_owned();
        if let Action::Approve = action {
            if let Some(refusal) = approve_refusal(self, feed, shown, env) {
                return toast(refusal);
            }
        } else if let Some(refusal) = device_refusal(feed, env).or_else(|| candidate_refusal(shown))
        {
            return toast(refusal);
        }
        let (Some(expect), Some(review)) = (shown.expect(), shown.review) else {
            return Vec::new();
        };
        match action {
            Action::Approve => {
                let Some(full) = review.candidate_sha.clone() else {
                    return toast("corrald reports no candidate SHA; nothing was sent.");
                };
                let act = Act::new(
                    &mut self.ids,
                    format!("Approve {task} candidate {sha}"),
                    Command::Review(ReviewCommand::Approve {
                        id: task.to_string(),
                        sha: full.clone(),
                    }),
                )
                .expect(expect)
                .confirmed();
                self.send(shown, Intent::Approve { sha: full }, act)
            }
            Action::Retry => {
                let act = Act::new(
                    &mut self.ids,
                    format!("Retry {task}"),
                    Command::Task(TaskCommand::Retry {
                        id: task.to_string(),
                        run: shown.run().map(str::to_owned),
                        profile: None,
                    }),
                )
                .expect(expect);
                self.send(shown, Intent::Retry, act)
            }
            Action::Discard => {
                let act = Act::new(
                    &mut self.ids,
                    format!("Discard {task} candidate {sha}"),
                    Command::Review(ReviewCommand::Discard {
                        id: task.to_string(),
                    }),
                )
                .expect(expect)
                .confirmed();
                self.send(shown, Intent::Discard, act)
            }
            _ => Vec::new(),
        }
    }

    /// Applies a served control's intent with the feed and the root's selection.
    pub fn apply_live(
        &mut self,
        action: Action,
        feed: &Feed,
        selection: &Selection,
        env: Env,
    ) -> Vec<Effect> {
        match &action {
            Action::Tab(tab) => return self.rtab(*tab),
            Action::Layout(layout) => return self.rlayout(*layout),
            Action::Pick(id) => {
                let live = Live::Review(id.clone());
                let mut effects = vec![if env.form == Form::Phone {
                    Effect::OpenLive(live)
                } else {
                    Effect::Select(live)
                }];
                effects.extend(self.refresh(feed, id));
                return effects;
            }
            Action::Recover(key) => return vec![Effect::Recover(key.clone())],
            Action::Dismiss(key) => {
                self.sent.retain(|sent| sent.key != *key);
                return Vec::new();
            }
            _ => {}
        }
        let Some(task) = selected(selection, feed) else {
            return Vec::new();
        };
        let shown = Shown::of(feed, task);
        let id = shown.id().clone();
        match action {
            Action::Expand(path) => {
                let effect = fetch(feed, file_ref(&id, &path));
                self.expanded.insert((id, path));
                vec![effect]
            }
            Action::Collapse(path) => {
                self.expanded.remove(&(id.clone(), path.clone()));
                if self
                    .composing
                    .as_ref()
                    .is_some_and(|(candidate, at)| candidate.task == id && at.path == path)
                {
                    self.composing = None;
                }
                Vec::new()
            }
            Action::AskAbout { path, hunk } => vec![Effect::AskAbout {
                context: Some(Live::Review(id).context(None)),
                text: Some(match hunk {
                    Some(hunk) => format!("{path} {hunk}: "),
                    None => format!("{path}: "),
                }),
                target: None,
            }],
            Action::Annotate { candidate, at } => {
                if !shown.matches(&candidate) {
                    return toast("The candidate changed. Reopen a line on the current patch.");
                }
                if let Some(refusal) = comment_refusal(feed, &shown, env)
                    .or_else(|| patch_refusal(feed, &shown, &at.path))
                {
                    return toast(refusal);
                }
                self.composing = Some((candidate, at));
                Vec::new()
            }
            Action::CancelCompose => {
                self.composing = None;
                self.changes = None;
                Vec::new()
            }
            Action::RequestChanges => {
                let Some(candidate) = shown.identity() else {
                    return toast("Waiting for the candidate's identity; no composer was opened.");
                };
                self.changes = if self.changes.as_ref() == Some(&candidate) {
                    None
                } else {
                    Some(candidate)
                };
                Vec::new()
            }
            Action::Submit { target, text } => self.submit(target, text, feed, &shown, env),
            Action::Resolve { thread, revision } => {
                self.thread_state(feed, &shown, env, thread, revision, false)
            }
            Action::Reopen { thread, revision } => {
                self.thread_state(feed, &shown, env, thread, revision, true)
            }
            decision @ (Action::Approve | Action::Retry | Action::Discard) => {
                self.decide(&decision, feed, &shown, env)
            }
            Action::Restore { key, .. } => {
                if let Some(sent) = self.sent.iter().find(|sent| sent.key == key) {
                    match &sent.intent {
                        Intent::Comment { at, .. } => {
                            self.composing = sent
                                .candidate
                                .clone()
                                .map(|candidate| (candidate, at.clone()));
                        }
                        Intent::Changes { .. } => self.changes = sent.candidate.clone(),
                        _ => {}
                    }
                }
                self.sent.retain(|sent| sent.key != key);
                Vec::new()
            }
            // Settled commands stay listed (Done shows no line) so the count that triggers this
            // revalidation only grows; Dismiss is the one way an entry leaves.
            Action::Refresh => self.refresh(feed, &id),
            Action::OpenArtifact(key) => {
                let effect = fetch(feed, artifact_ref(&id, &key));
                self.opened.insert((id, key));
                vec![effect]
            }
            Action::CloseArtifact(key) => {
                self.opened.remove(&(id, key));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}
