//! Mode-specific facts; prefer live host briefs to the roster.
use corral_protocol::build::BuildMode;
use corral_protocol::detail::{ChangesComparison, DetailBody, DetailRef};
use corral_protocol::rows::{AssistantKind, AssistantLocation, AssistantState};
use corral_types::Role;

use crate::model::Id;
use crate::model::daemon::{Feed, age};
use crate::model::pipeline::pipeline;
use crate::model::session::transcript::{Message, MessageItem, MessageKind};
use crate::model::session::{Cx, exit_status, status};
use crate::model::status::Stage;

use super::words_of;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeBlock {
    pub title: &'static str,
    pub facts: Vec<(&'static str, String)>,
}

impl ModeBlock {
    fn new(title: &'static str) -> Self {
        Self {
            title,
            facts: Vec::new(),
        }
    }

    fn add(&mut self, label: &'static str, value: Option<impl ToString>) {
        self.facts.extend(
            value
                .map(|value| (label, value.to_string()))
                .filter(|(_, value)| !value.trim().is_empty()),
        );
    }
}

pub fn changes_target(id: &Id) -> DetailRef {
    DetailRef::SessionChanges {
        id: id.to_string(),
        comparison: ChangesComparison::Session,
        scope: None,
    }
}

pub fn watch_host(feed: &Feed, id: &Id) -> bool {
    feed.assistants.get(id.as_str()).is_some_and(|row| {
        matches!(
            row.live_in,
            AssistantLocation::Pane | AssistantLocation::Daemon
        ) && !matches!(row.state, AssistantState::Cold | AssistantState::Ended)
    })
}

pub fn watch_changes(feed: &Feed, id: &Id) -> bool {
    watch_host(feed, id)
        && feed
            .assistants
            .get(id.as_str())
            .is_some_and(|row| row.build.is_some())
        && feed
            .live
            .get(id.as_str())
            .and_then(|live| live.state.as_ref())
            .is_some_and(|state| {
                state
                    .can
                    .iter()
                    .any(|cap| cap.id == "changes" && cap.unavailable.is_none())
            })
}

fn diff(cx: &Cx<'_>) -> Option<String> {
    if watch_changes(cx.feed, cx.id)
        && let Some(DetailBody::SessionChanges(changes)) = cx
            .feed
            .detail(&changes_target(cx.id))
            .and_then(|cache| cache.body())
        && changes.error.is_none()
    {
        let bound = if changes.truncated { " · partial" } else { "" };
        return Some(format!(
            "{} files · +{} −{}{bound}",
            changes.total, changes.added, changes.removed
        ));
    }
    let diff = cx.build()?.diff.as_ref()?;
    Some(format!(
        "{} files · +{} −{} · {} commits",
        diff.files, diff.added, diff.removed, diff.commits
    ))
}

fn coding(cx: &Cx<'_>, code: bool) -> ModeBlock {
    let mut block = ModeBlock::new(if code { "Code" } else { "Build" });
    if let Some(build) = cx.build() {
        block.add(
            "checkout",
            Some(if code {
                &build.worktree
            } else {
                &build.repo_root
            }),
        );
        block.add("branch", build.branch.as_deref());
        if !code {
            block.add("worktree", Some(&build.worktree));
            block.add("base", build.base.as_deref());
        }
    }
    block.add("diff", diff(cx));
    block.add(
        "plan step",
        cx.state.and_then(|s| s.plan.as_ref()?.current.as_deref()),
    );
    let children = cx
        .state
        .map(|s| s.children)
        .or_else(|| cx.build().map(|b| b.children));
    block.add(
        "children",
        children.map(|c| {
            format!(
                "{} live · {} waiting · {} total",
                c.live, c.waiting, c.total
            )
        }),
    );
    block.add(
        "context",
        status(cx)
            .meter
            .percent
            .map(|percent| format!("{percent}%")),
    );
    block.add(
        "runs in",
        cx.row.and_then(|row| match row.live_in {
            AssistantLocation::Pane => Some("terminal"),
            AssistantLocation::Daemon => Some("corrald"),
            _ => None,
        }),
    );
    if !code {
        block.add(
            "exit",
            cx.exit().and_then(exit_status).map(|exit| {
                match exit
                    .detail
                    .as_deref()
                    .filter(|detail| !detail.trim().is_empty())
                {
                    Some(detail) => format!("{} · {detail}", exit.short),
                    None => exit.short,
                }
            }),
        );
    }
    block
}

fn prose(message: &Message) -> Option<String> {
    message
        .items
        .iter()
        .filter(|item| matches!(item, MessageItem::Prose(_) | MessageItem::Paragraph(_)))
        .find_map(words_of)
}

/// Don't pair an old reply with a new unanswered input, or use tool rows as a reply.
fn exchange(messages: &[Message]) -> (Option<String>, Option<String>) {
    let mut reply = None;
    for message in messages.iter().rev() {
        match message.kind {
            MessageKind::Bubble => return (prose(message), reply),
            MessageKind::Agent if reply.is_none() => reply = prose(message),
            _ => (),
        }
    }
    (None, reply)
}

fn queued(cx: &Cx<'_>) -> Option<u32> {
    // The chat roster's `pending` is currently always zero, not a queue reading.
    cx.state.map(|s| s.queue_total)
}

fn chat(cx: &Cx<'_>, messages: &[Message]) -> ModeBlock {
    let mut block = ModeBlock::new("Chat");
    block.add("focus", cx.row.and_then(|row| row.focus.as_deref()));
    let (input, reply) = exchange(messages);
    block.add("last input", input);
    block.add("last reply", reply);
    block.add("queued", queued(cx));
    block
}

fn quiet_since(cx: &Cx<'_>) -> Option<u64> {
    let agent = cx.run?;
    if agent.status == corral_types::SessionStatus::Gone {
        return None;
    }
    let run = &agent.run;
    let since = u64::try_from(run.quiet_since?.as_second()).ok()?;
    (cx.feed.now > 0 && since <= cx.feed.now && run.ended.is_none()).then_some(since)
}

fn run(cx: &Cx<'_>, role: Role) -> ModeBlock {
    let mut block = ModeBlock::new(match role {
        Role::Worker => "Worker",
        Role::Reviewer => "Reviewer",
    });
    block.add("task", cx.task.map(ToString::to_string));
    if let Some(task) = cx.task.and_then(|id| cx.feed.board.task(id)) {
        let round = task.task().round.max(1);
        block.add(
            "round",
            Some(match task.row.max_rounds {
                Some(max) => format!("{round} of {max}"),
                None => round.to_string(),
            }),
        );
        let steps = pipeline(task);
        block.add(
            "pipeline",
            steps
                .into_iter()
                .rev()
                .find(|step| !matches!(step.stage, Stage::None | Stage::Wait))
                .map(|step| step.spoken()),
        );
    }
    block.add(
        "quiet",
        quiet_since(cx).map(|since| age(Some(since), cx.feed.now)),
    );
    block
}

pub(super) fn preview(cx: &Cx<'_>, messages: &[Message]) -> ModeBlock {
    if let Some(agent) = cx.run {
        run(cx, agent.run.role)
    } else if cx
        .build()
        .is_some_and(|build| build.mode == BuildMode::Code)
    {
        coding(cx, true)
    } else if cx.row.is_some_and(|row| row.kind == AssistantKind::Build) {
        coding(cx, false)
    } else {
        chat(cx, messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Form;
    use crate::model::effect::{Conn, Env};
    use crate::model::session::{Facts, Ui};

    fn sample() -> Feed {
        crate::model::preview::feed(1_800_000_000, String::new(), String::new(), None)
    }

    fn with_session<T>(feed: &Feed, id: &str, read: impl FnOnce(&Cx<'_>) -> T) -> T {
        let id = Id::from(id);
        let facts = Facts::of(feed, &id);
        let env = Env {
            conn: Conn::Healthy,
            form: Form::Desktop,
        };
        read(&Cx::new(feed, &Ui::default(), &id, env, &facts))
    }

    #[test]
    fn code_does_not_expose_build_worktree_metadata() {
        let mut feed = sample();
        let build = feed
            .assistants
            .get_mut("B-code")
            .unwrap()
            .build
            .as_mut()
            .unwrap();
        build.worktree = "/code/checkout".into();
        build.repo_root = "/code/record-root".into();
        build.base = Some("not-a-code-base".into());
        with_session(&feed, "B-code", |cx| {
            let block = preview(cx, &[]);
            assert!(
                block
                    .facts
                    .iter()
                    .any(|(_, value)| value == "/code/checkout")
            );
            assert!(
                !block
                    .facts
                    .iter()
                    .any(|(_, value)| value == "/code/record-root" || value == "not-a-code-base")
            );
        });
    }

    #[test]
    fn build_keeps_checkout_worktree_and_base_distinct() {
        let feed = sample();
        with_session(&feed, "B-k3x9", |cx| {
            let block = preview(cx, &[]);
            let build = cx.build().unwrap();
            for fact in [
                &build.repo_root,
                &build.worktree,
                build.base.as_ref().unwrap(),
            ] {
                assert!(block.facts.iter().any(|(_, value)| value == fact));
            }
        });
    }

    #[test]
    fn chat_queue_is_unknown_until_host_state_arrives() {
        let mut feed = sample();
        feed.live.remove("A-15");
        with_session(&feed, "A-15", |cx| assert_eq!(queued(cx), None));
        crate::model::preview::effect(
            &mut feed,
            &crate::model::effect::Effect::Watch {
                session: Id::from("A-15"),
                on: true,
            },
        )
        .unwrap();
        feed.live
            .get_mut("A-15")
            .unwrap()
            .state
            .as_mut()
            .unwrap()
            .queue_total = 7;
        with_session(&feed, "A-15", |cx| assert_eq!(queued(cx), Some(7)));
    }

    #[test]
    fn worker_quiet_time_uses_output_timestamp_not_start_time() {
        let mut feed = sample();
        let (id, agent) = feed
            .agents
            .iter_mut()
            .find(|(_, a)| a.run.role == Role::Worker && a.run.ended.is_none())
            .unwrap();
        let id = id.clone();
        agent.status = corral_types::SessionStatus::Working;
        agent.run.quiet_since = Some(jiff::Timestamp::from_second(1_799_999_950).unwrap());
        with_session(&feed, id.as_str(), |cx| {
            assert_eq!(quiet_since(cx), Some(1_799_999_950))
        });
    }
}
