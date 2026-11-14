//! Served Inbox state and reducers. Rows come from `feed.inbox`, the selected question from
//! its `detail.get` body; this module keeps only what the owner is doing on this device: the
//! selected row, each question's choice, alternate answer, evidence tab and follow-up draft,
//! and the identity of every command sent. Answers and follow-ups are canonical `question.*`
//! commands carrying the question's own revision; nothing is answered locally.

use std::collections::BTreeMap;

use corral_protocol::cmd::Command;
use corral_protocol::cmd::question::QuestionCommand;
use corral_protocol::detail::{DetailBody, DetailGet, DetailRef, QuestionDetail};
use corral_protocol::rows::{InboxKind, InboxRow, InboxState};

use super::{Action, QuestionState, State};
use crate::model::act::{self, Act, ActStatus};
use crate::model::daemon::Feed;
use crate::model::effect::{Effect, Env};
use crate::model::nav::AskTarget;
use crate::model::{Id, Live, Selection};

/// A command this device sent for a question: its identity, kept for the same-id recovery
/// of an uncertain result, and what it carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    /// The command identity (`cmd_id`).
    pub cmd: Id,
    /// The question revision the command expected.
    pub rev: u32,
    /// Zero-based option, absent for a written answer or a follow-up.
    pub option: Option<usize>,
    /// The answer or follow-up text.
    pub text: String,
}

/// What the owner is doing with one served question on this device.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Draft {
    /// Chosen option; absent until chosen, when the recommended option is shown chosen.
    pub choice: Option<usize>,
    /// The alternate written answer.
    pub text: String,
    /// Selected evidence block.
    pub evidence: usize,
    /// The follow-up being written to the asker.
    pub reply: String,
    /// The last answer sent.
    pub answer: Option<Sent>,
    /// The last follow-up sent.
    pub followup: Option<Sent>,
}

/// Served Inbox state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Desk {
    /// The selected row on desktop and terminal; the first row when absent or gone.
    pub row: Option<Id>,
    /// Per-question drafts and sent commands, by question key.
    pub drafts: BTreeMap<Id, Draft>,
}

impl Desk {
    /// The draft of a question, if the owner touched it.
    pub fn draft(&self, key: &str) -> Option<&Draft> {
        self.drafts.get(key)
    }

    fn draft_mut(&mut self, key: &Id) -> &mut Draft {
        self.drafts.entry(key.clone()).or_default()
    }
}

/// The question selector for `detail.get`.
pub fn question_ref(key: &str) -> DetailRef {
    DetailRef::Question { key: key.into() }
}

/// The selected question's served body, when it has arrived.
pub fn question_detail<'a>(feed: &'a Feed, key: &str) -> Option<&'a QuestionDetail> {
    match feed.detail(&question_ref(key))?.body()? {
        DetailBody::Question(detail) => Some(detail),
        _ => None,
    }
}

/// A revalidation of the question's detail with its cached etag.
pub fn refetch(feed: &Feed, key: &str) -> Effect {
    let target = question_ref(key);
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

/// Rows that carry a canonical question record.
pub fn is_question(row: &InboxRow) -> bool {
    match row.kind {
        InboxKind::WorkerQuestion | InboxKind::PmAsk | InboxKind::AssistantAsk => true,
        InboxKind::Escalated => row.question_rev.is_some(),
        _ => false,
    }
}

/// Whether a row still waits on the owner.
pub fn is_open(row: &InboxRow) -> bool {
    matches!(row.state, InboxState::Open)
}

/// Where opening a row leads: its question, assistant thread, plan, review or task.
pub fn target(row: &InboxRow) -> Option<Live> {
    let id = |value: &Option<String>| value.as_deref().filter(|id| !id.is_empty()).map(Id::new);
    match row.kind {
        _ if is_question(row) => Some(Live::Question(Id::new(&row.id))),
        InboxKind::Prompt => (!row.from.is_empty()).then(|| Live::Thread(Id::new(&row.from))),
        InboxKind::PlanAcceptance => Some(Live::Plan(
            id(&row.plan).unwrap_or_else(|| Id::new(&row.id)),
        )),
        InboxKind::Review => Some(Live::Review(
            id(&row.task).unwrap_or_else(|| Id::new(&row.id)),
        )),
        _ => id(&row.task).map(Live::Task),
    }
}

/// Rows in the order the owner works them: open before settled, blocking first, then the
/// oldest ask, then id.
pub fn ordered(feed: &Feed) -> Vec<&InboxRow> {
    let mut rows: Vec<&InboxRow> = feed.inbox.values().collect();
    rows.sort_by_cached_key(|row| {
        (
            !is_open(row),
            !row.blocking,
            row.asked_at.as_deref().and_then(unix).unwrap_or(u64::MAX),
            row.id.clone(),
        )
    });
    rows
}

/// The selected row: the remembered one while it is served, else the first.
pub fn selected<'a>(desk: &Desk, rows: &[&'a InboxRow]) -> Option<&'a InboxRow> {
    desk.row
        .as_ref()
        .and_then(|id| rows.iter().find(|row| row.id == id.as_str()).copied())
        .or_else(|| rows.first().copied())
}

/// Unix seconds of an RFC 3339 time.
pub fn unix(at: &str) -> Option<u64> {
    at.parse::<jiff::Timestamp>()
        .ok()
        .and_then(|time| u64::try_from(time.as_second()).ok())
}

/// A served time on this device's clock: `HH:MM` today, `MM-DD HH:MM` otherwise; the text
/// as served when it does not parse.
pub fn clock(at: &str, now: u64) -> String {
    let Ok(time) = at.parse::<jiff::Timestamp>() else {
        return at.to_owned();
    };
    let zone = jiff::tz::TimeZone::system();
    let local = time.to_zoned(zone.clone());
    let today = i64::try_from(now)
        .ok()
        .filter(|now| *now > 0)
        .and_then(|now| jiff::Timestamp::from_second(now).ok())
        .map(|now| now.to_zoned(zone).date());
    if today == Some(local.date()) {
        local.strftime("%H:%M").to_string()
    } else {
        local.strftime("%m-%d %H:%M").to_string()
    }
}

/// Whole-unit age of a served time, empty when unknown.
pub fn age(at: Option<&str>, feed: &Feed) -> String {
    if feed.now == 0 {
        return String::new();
    }
    crate::model::daemon::age(at.and_then(unix), feed.now)
}

fn pending(status: &ActStatus) -> bool {
    matches!(
        status,
        ActStatus::Previewing
            | ActStatus::Confirming
            | ActStatus::Queued
            | ActStatus::Pending
            | ActStatus::Running
    )
}

/// The status of a command this device sent. A command is only built once the session,
/// authority and link allow it, so one the client has not journaled yet is on its way.
pub fn sent_status(feed: &Feed, sent: Option<&Sent>) -> Option<ActStatus> {
    let sent = sent?;
    Some(act::status(feed, &sent.cmd).unwrap_or(ActStatus::Pending))
}

/// Whether a recorded answer came from this device (`via=app:<device-id>`).
pub fn from_this_device(feed: &Feed, device: Option<&str>, via: &str) -> bool {
    feed.session.as_ref().is_some_and(|session| {
        let id = session.device.id.as_str();
        device == Some(id) || via == format!("app:{id}")
    })
}

/// The effective answer state: the canonical record first (a stale detail yields to a
/// newer row), then this device's in-flight or uncertain command.
pub fn question_state(state: &State, feed: &Feed, detail: &QuestionDetail) -> QuestionState {
    let row = feed.inbox.get(detail.key.as_str());
    let canonical = match (&detail.state, row.map(|row| &row.state)) {
        (InboxState::Open | InboxState::Other, Some(newer)) => newer,
        (state, _) => state,
    };
    match canonical {
        InboxState::Answered { via, .. } => {
            let device = detail
                .answer
                .as_ref()
                .and_then(|answer| answer.device.as_deref());
            let via = detail
                .answer
                .as_ref()
                .map_or(via.as_str(), |answer| answer.via.as_str());
            if from_this_device(feed, device, via) {
                QuestionState::Mine
            } else {
                QuestionState::Elsewhere
            }
        }
        InboxState::Cancelled | InboxState::Expired => QuestionState::Closed,
        InboxState::Open | InboxState::Other => {
            let sent = state
                .desk
                .draft(&detail.key)
                .and_then(|draft| draft.answer.as_ref());
            match sent_status(feed, sent) {
                Some(status) if pending(&status) => QuestionState::Pending,
                Some(ActStatus::Unknown) => QuestionState::Uncertain,
                Some(ActStatus::Done) => QuestionState::Mine,
                _ => QuestionState::Open,
            }
        }
    }
}

/// The option shown chosen: the owner's choice, else the asker's recommendation.
pub fn choice(draft: Option<&Draft>, detail: &QuestionDetail) -> Option<usize> {
    draft
        .and_then(|draft| draft.choice)
        .filter(|choice| *choice < detail.options.len())
        .or_else(|| detail.options.iter().position(|option| option.recommended))
}

/// Whether this device may answer: an authenticated session with `reply` authority, a live
/// link, and a question corrald says is answerable here.
pub fn can_answer(feed: &Feed, row: Option<&InboxRow>) -> bool {
    let reply = feed
        .session
        .as_ref()
        .is_some_and(|session| session.authority.reply);
    reply && feed.link == crate::model::daemon::Link::Up && row.is_none_or(|row| row.answerable)
}

/// The follow-up text shown in the field: cleared once the last one sent is applied.
pub fn reply_value(state: &State, feed: &Feed, key: &str) -> String {
    let Some(draft) = state.desk.draft(key) else {
        return String::new();
    };
    let posted = draft.followup.as_ref().is_some_and(|sent| {
        sent.text == draft.reply && matches!(sent_status(feed, Some(sent)), Some(ActStatus::Done))
    });
    if posted {
        String::new()
    } else {
        draft.reply.clone()
    }
}

impl State {
    /// Dispatch a served-Inbox action; every command and navigation is an effect.
    pub fn act_live(&mut self, action: Action, feed: &Feed, _env: Env) -> Vec<Effect> {
        self.prune(feed);
        match action {
            Action::Row(id) => self.select_row(id, feed),
            Action::OpenRow(id) => self.open_row(&id, feed),
            Action::Live(live) => vec![Effect::OpenLive(live)],
            Action::Pick(key, index) => {
                if question_detail(feed, &key).is_some_and(|detail| {
                    index < detail.options.len()
                        && question_state(self, feed, detail) == QuestionState::Open
                }) {
                    self.desk.draft_mut(&key).choice = Some(index);
                }
                Vec::new()
            }
            Action::Proof(key, index) => {
                self.desk.draft_mut(&key).evidence = index;
                Vec::new()
            }
            Action::Text(key, text) => {
                self.desk.draft_mut(&key).text = text;
                Vec::new()
            }
            Action::Reply(key, text) => {
                let draft = self.desk.draft_mut(&key);
                if draft.followup.as_ref().is_some_and(|sent| {
                    sent.text == draft.reply
                        && matches!(sent_status(feed, Some(sent)), Some(ActStatus::Done))
                }) {
                    draft.followup = None;
                }
                draft.reply = text;
                Vec::new()
            }
            Action::Submit(key) => self.submit(key, feed),
            Action::Followup(key) => self.followup(key, feed),
            Action::Recover(cmd) => vec![Effect::Recover(cmd)],
            Action::Refetch(key) => vec![refetch(feed, &key)],
            Action::AskAbout(key, pm) => {
                let rev = question_detail(feed, &key).map(|detail| u64::from(detail.rev));
                vec![Effect::AskAbout {
                    context: Some(Live::Question(key).context(rev)),
                    text: None,
                    target: pm.then_some(AskTarget::Pm),
                }]
            }
            Action::FocusReply => {
                self.reply_focus = self.reply_focus.wrapping_add(1);
                Vec::new()
            }
            Action::Next(id) => self.next(&id, feed),
            Action::Thread => {
                self.thread_open = true;
                Vec::new()
            }
            Action::CloseThread => {
                self.thread_open = false;
                Vec::new()
            }
            Action::Go(destination) => vec![Effect::Go(destination)],
            // Fixture-only controls are never built from served data.
            _ => Vec::new(),
        }
    }

    /// Served-Inbox keys on desktop and terminal: j/k and the arrows move the selected row,
    /// Enter and f open it, digits choose an option of the selected question. On a question's
    /// own page digits and j/k move the choice and e cycles the evidence.
    pub fn key_live(
        &mut self,
        key: &str,
        top: Option<&Live>,
        _selection: &Selection,
        feed: &Feed,
        env: Env,
    ) -> Option<Vec<Effect>> {
        if let Some(top) = top {
            let Live::Question(question) = top else {
                return None;
            };
            return self.question_key(key, question.clone(), feed, env);
        }
        let rows = ordered(feed);
        let current = selected(&self.desk, &rows)?;
        let at = rows
            .iter()
            .position(|row| row.id == current.id)
            .unwrap_or(0);
        let id = |index: usize| Id::new(&rows[index].id);
        match key {
            "j" | "down" => Some(self.select_row(id((at + 1).min(rows.len() - 1)), feed)),
            "k" | "up" => Some(self.select_row(id(at.saturating_sub(1)), feed)),
            "enter" | "f" => Some(self.open_row(&id(at), feed)),
            digit if is_question(current) => {
                let index = digit.parse::<usize>().ok()?.checked_sub(1)?;
                Some(self.act_live(Action::Pick(id(at), index), feed, env))
            }
            _ => None,
        }
    }

    fn question_key(
        &mut self,
        key: &str,
        question: Id,
        feed: &Feed,
        env: Env,
    ) -> Option<Vec<Effect>> {
        let detail = question_detail(feed, &question)?;
        let count = detail.options.len();
        let draft = self.desk.draft(&question);
        let current = choice(draft, detail);
        let index = match key {
            "j" | "down" => current.map_or(0, |at| (at + 1).min(count.saturating_sub(1))),
            "k" | "up" => current.map_or(0, |at| at.saturating_sub(1)),
            "e" => {
                let blocks = detail.evidence.len();
                if blocks > 0 {
                    let draft = self.desk.draft_mut(&question);
                    draft.evidence = (draft.evidence + 1) % blocks;
                }
                return Some(Vec::new());
            }
            digit => digit.parse::<usize>().ok()?.checked_sub(1)?,
        };
        if count == 0 {
            return Some(Vec::new());
        }
        Some(self.act_live(Action::Pick(question, index), feed, env))
    }

    fn select_row(&mut self, id: Id, feed: &Feed) -> Vec<Effect> {
        self.desk.row = Some(id.clone());
        self.thread_open = false;
        match feed.inbox.get(id.as_str()) {
            Some(row) if is_question(row) => vec![Effect::Select(Live::Question(id))],
            _ => Vec::new(),
        }
    }

    fn open_row(&mut self, id: &Id, feed: &Feed) -> Vec<Effect> {
        let Some(row) = feed.inbox.get(id.as_str()) else {
            return Vec::new();
        };
        self.desk.row = Some(id.clone());
        self.thread_open = false;
        target(row).map(Effect::OpenLive).into_iter().collect()
    }

    /// Open the next open item after this one, wrapping to the first.
    fn next(&mut self, id: &Id, feed: &Feed) -> Vec<Effect> {
        let rows = ordered(feed);
        let open: Vec<&InboxRow> = rows.into_iter().filter(|row| is_open(row)).collect();
        let at = open.iter().position(|row| row.id == id.as_str());
        let next = match at {
            Some(at) => open
                .iter()
                .cycle()
                .skip(at + 1)
                .take(open.len())
                .find(|row| row.id != id.as_str()),
            None => open.first(),
        };
        match next {
            Some(row) => self.open_row(&Id::new(&row.id), feed),
            None => Vec::new(),
        }
    }

    /// Answer with the chosen option, or the written answer when there is one, at the
    /// question's own revision. One identity per intent: an in-flight or uncertain answer is
    /// never replaced; a refused, failed, conflicting or abandoned one may be sent again.
    fn submit(&mut self, key: Id, feed: &Feed) -> Vec<Effect> {
        let Some(detail) = question_detail(feed, &key) else {
            return Vec::new();
        };
        if question_state(self, feed, detail) != QuestionState::Open
            || !can_answer(feed, feed.inbox.get(key.as_str()))
        {
            return Vec::new();
        }
        let chosen = choice(self.desk.draft(&key), detail);
        let written = self
            .desk
            .draft(&key)
            .map(|draft| draft.text.trim().to_owned())
            .unwrap_or_default();
        let (text, option, index) = if !written.is_empty() {
            (written, None, None)
        } else if let Some(index) = chosen {
            let label = detail.options[index].label.clone();
            (label.clone(), Some(label), Some(index))
        } else {
            return Vec::new();
        };
        let rev = detail.rev;
        let act = Act::new(
            format!("Answer {key}"),
            Command::Question(QuestionCommand::Answer {
                key: key.to_string(),
                rev,
                text: text.clone(),
                option,
                basis: Vec::new(),
                scope_note: None,
                owner: true,
                json: false,
            }),
        )
        .about(Live::Question(key.clone()));
        self.desk.draft_mut(&key).answer = Some(Sent {
            cmd: act.key.clone(),
            rev,
            option: index,
            text,
        });
        vec![Effect::Act(act)]
    }

    /// Post the follow-up to the asker at the question's revision; same identity rules as
    /// answers.
    fn followup(&mut self, key: Id, feed: &Feed) -> Vec<Effect> {
        let Some(detail) = question_detail(feed, &key) else {
            return Vec::new();
        };
        let open = matches!(
            question_state(self, feed, detail),
            QuestionState::Open | QuestionState::Pending
        );
        if !open || !can_answer(feed, None) {
            return Vec::new();
        }
        let draft = self.desk.draft(&key);
        let text = draft
            .map(|draft| draft.reply.trim().to_owned())
            .unwrap_or_default();
        let busy = sent_status(feed, draft.and_then(|draft| draft.followup.as_ref()))
            .is_some_and(|status| pending(&status) || matches!(status, ActStatus::Unknown));
        if text.is_empty() || busy {
            return Vec::new();
        }
        let rev = detail.rev;
        let act = Act::new(
            format!("Follow-up on {key}"),
            Command::Question(QuestionCommand::Followup {
                key: key.to_string(),
                rev,
                text: text.clone(),
            }),
        )
        .about(Live::Question(key.clone()));
        let reply = self
            .desk
            .draft(&key)
            .map(|draft| draft.reply.clone())
            .unwrap_or_default();
        self.desk.draft_mut(&key).followup = Some(Sent {
            cmd: act.key.clone(),
            rev,
            option: None,
            text: reply,
        });
        vec![Effect::Act(act)]
    }

    /// Forget drafts of questions corrald no longer serves, unless a command for them is
    /// still unsettled. Only a complete, current inbox is evidence of absence.
    fn prune(&mut self, feed: &Feed) {
        let current = feed
            .topics
            .get("inbox")
            .is_some_and(|topic| !topic.loading && !topic.stale && topic.error.is_none());
        if !current {
            return;
        }
        self.desk.drafts.retain(|key, draft| {
            feed.inbox.contains_key(key.as_str())
                || [&draft.answer, &draft.followup].into_iter().any(|sent| {
                    sent_status(feed, sent.as_ref()).is_some_and(|status| !status.settled())
                })
        });
        if self
            .desk
            .row
            .as_ref()
            .is_some_and(|row| !feed.inbox.contains_key(row.as_str()))
        {
            self.desk.row = None;
        }
    }
}
