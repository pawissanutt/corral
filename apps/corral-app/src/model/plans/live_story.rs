//! The selected member's Story inspector: the member's task (its run, checks and where it
//! stands) and its place in the plan's dependency edges.

use corral_protocol::detail::PlanDetail;
use corral_types::Status;

use super::member::Member;
use super::path::next_step;
use super::story::{field, ids};
use super::{Action, Button, ButtonKind, LiveAction, Story, StoryStage};
use crate::model::board::{Mark, STAGES};
use crate::model::daemon::Feed;
use crate::model::hue::Hue;
use crate::model::text::span;
use crate::model::{Id, Live};

fn status(mark: Mark) -> &'static str {
    match mark {
        Mark::Done => "done",
        Mark::Active => "now",
        Mark::Fail => "failed",
        Mark::Stale => "stale",
        Mark::Next => "next",
        Mark::Na => "n/a",
    }
}

/// The inspector of `child` in `detail`.
pub(super) fn story(
    feed: &Feed,
    plan: &Id,
    detail: &PlanDetail,
    child: &corral_types::Task,
) -> Story {
    let member = Member::of(feed, child);
    let id = Id::new(member.id());
    let you = member.needs_you();
    let marks = member.marks();
    let run = member.row.and_then(|row| row.row.run.as_ref());
    let context = match run {
        Some(run) => {
            let mut words = format!("{} · {}", run.role.as_str(), run.profile);
            if let Some(model) = &run.model {
                words.push_str(&format!(" · {model}"));
            }
            vec![span(words)]
        }
        None => vec![span(format!("Plan {plan} · {}", member.status().as_str()))],
    };
    let stages = STAGES
        .into_iter()
        .enumerate()
        .map(|(i, label)| {
            let mark = marks[i];
            let sha = member.row.and_then(|row| row.rail[i].sha7.clone());
            let sub = match (i, sha) {
                (0, _) => run.map_or_else(|| "−".into(), |run| run.profile.clone()),
                (3, _) => member.task().base.clone().unwrap_or_else(|| "−".into()),
                (_, Some(sha)) => sha,
                (_, None) if mark == Mark::Next => format!("after {}", STAGES[i - 1]),
                _ => "−".into(),
            };
            StoryStage {
                label,
                mark,
                status: if you && i == 0 {
                    "waits on you"
                } else {
                    status(mark)
                },
                sub,
                hue: match mark {
                    Mark::Active if i == 0 && you => Hue::You,
                    Mark::Active if i == 0 => Hue::Worker,
                    Mark::Active => Hue::Corral,
                    Mark::Stale => Hue::Held,
                    Mark::Fail => Hue::Fail,
                    _ => Hue::Neutral,
                },
            }
        })
        .collect();
    let mut needs = vec![];
    for (i, dep) in member.task().depends_on.iter().enumerate() {
        if i > 0 {
            needs.push(span(" · "));
        }
        needs.push(span(dep.as_str()).mono());
        if let Some(prior) = detail.children.iter().find(|other| other.id == *dep) {
            needs.push(span(format!(" {}", Member::of(feed, prior).marks()[3].glyph())));
        }
    }
    if needs.is_empty() {
        needs.push(span("−"));
    }
    let unblocks: Vec<&str> = detail
        .children
        .iter()
        .filter(|other| other.depends_on.iter().any(|dep| *dep == child.id))
        .map(|other| other.id.as_str())
        .collect();
    let mut you_field = field(
        "You",
        if you {
            vec![span("needs you").hue(Hue::You)]
        } else {
            vec![span("nothing needed").hue(Hue::Ok)]
        },
    );
    if let Some(blocked) = member.blocked() {
        you_field.value = vec![span(blocked.reason.clone()).hue(Hue::You)];
        you_field.button = next_step(&id, blocked).map(|next| Button {
            kind: ButtonKind::Standard,
            ..Button::new(next.label, next.action).hinted(next.hint)
        });
    }
    let mut buttons = vec![];
    let session = run.map(|run| format!("{id}_{}", run.id));
    if let Some(session) = session.filter(|session| feed.agents.contains_key(session.as_str())) {
        buttons.push(Button::new(
            "Open session",
            Action::Live(LiveAction::Session(Id::new(&session))),
        ));
    }
    buttons.push(Button::new(
        "Ask about this",
        Action::Live(LiveAction::Ask {
            record: Live::Task(id.clone()),
        }),
    ));
    if member.status() == Status::Review {
        buttons.push(
            Button::new(
                "Review candidate",
                Action::Live(LiveAction::Review(id.clone())),
            )
            .quiet(),
        );
    }
    Story {
        title: member.title().to_owned(),
        plan: Some(format!("· plan {plan}")),
        context,
        stages,
        fields: vec![
            field("Next", vec![span(member.now())]),
            you_field,
            field("Needs", needs),
            field("Unblocks", ids(&unblocks[..], " ")),
        ],
        buttons,
        id,
    }
}
