//! The selected member's Story inspector for corrald's plans: the board row of the task (who
//! acts, the agent, cost) and the plan member's rail and dependencies. Worker progress steps are
//! not served, so the progress block stays absent rather than invented.

use corral_protocol::detail::{PlanDetail, PlanMember};
use corral_protocol::rows::PlanRow as WirePlan;

use super::live::{open_question, rail};
use super::story::{field, ids};
use super::{Action, Button, ButtonKind, LiveAction, Story, StoryStage};
use crate::model::board::{Mark, STAGES, money};
use crate::model::daemon::{Feed, Group};
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

/// The inspector of `member` in `detail`.
pub(super) fn story(
    feed: &Feed,
    plan: &Id,
    row: Option<&WirePlan>,
    detail: &PlanDetail,
    member: &PlanMember,
) -> Story {
    let id = Id::from(&member.id);
    let task = feed.board.task(&member.id);
    let question = open_question(feed, &member.id);
    let you = question.is_some() || task.is_some_and(|task| task.group == Group::NeedsYou);
    let marks = rail(member);
    let agent = task.and_then(|task| task.agent.as_ref());
    let context = match agent {
        Some(agent) => {
            let mut words = agent.role.clone();
            if let Some(profile) = &agent.profile {
                words.push_str(&format!(" · {profile}"));
            }
            if let Some(machine) = &agent.machine {
                words.push_str(&format!(" · on {machine}"));
            }
            vec![span(words)]
        }
        None => vec![span(format!("Plan {plan} · {}", member.state))],
    };
    let stages = STAGES
        .into_iter()
        .enumerate()
        .map(|(i, label)| {
            let mark = marks[i];
            let sub = match (i, &member.rail[i].sha7) {
                (0, _) => agent
                    .and_then(|agent| agent.profile.clone())
                    .or_else(|| agent.map(|agent| agent.role.clone()))
                    .unwrap_or_else(|| "−".into()),
                (3, _) => row
                    .and_then(|row| row.delivery.clone())
                    .unwrap_or_else(|| "−".into()),
                (_, Some(sha)) => sha.clone(),
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
    let needs: Vec<&str> = detail
        .edges
        .iter()
        .filter(|edge| edge.to == member.id)
        .map(|edge| edge.from.as_str())
        .collect();
    let mut needs_rich = vec![];
    for (i, dep) in needs.iter().enumerate() {
        if i > 0 {
            needs_rich.push(span(" · "));
        }
        needs_rich.push(span(*dep).mono());
        if let Some(prior) = detail.members.iter().find(|m| m.id == *dep) {
            needs_rich.push(span(format!(" {}", rail(prior)[3].glyph())));
        }
    }
    if needs_rich.is_empty() {
        needs_rich.push(span("−"));
    }
    let unblocks: Vec<&str> = detail
        .edges
        .iter()
        .filter(|edge| edge.from == member.id)
        .map(|edge| edge.to.as_str())
        .collect();
    let mut you_field = field(
        "You",
        if you {
            vec![span("needs you").hue(Hue::You)]
        } else {
            vec![span("nothing needed").hue(Hue::Ok)]
        },
    );
    if let Some(key) = &question {
        you_field.value = vec![];
        you_field.button = Some(Button {
            kind: ButtonKind::Standard,
            ..Button::new(
                format!("Answer {key}"),
                Action::Live(LiveAction::Question(key.clone())),
            )
        });
    }
    let cost = match task.and_then(|task| task.cost) {
        Some(cost) => {
            let mut words = vec![span(money(Some(cost as f32)))];
            if task.is_some_and(|task| task.cost_estimated) {
                words.push(span(" · estimated").dim());
            }
            if cost > 20.0 {
                words.push(span(" · over the $20 warning").hue(Hue::Held));
            }
            words
        }
        None => vec![span("unknown").dim()],
    };
    let mut buttons = vec![];
    if let Some(session) = agent.and_then(|agent| agent.session.as_ref()) {
        buttons.push(Button::new(
            "Open session",
            Action::Live(LiveAction::Session(Id::from(session))),
        ));
    }
    buttons.push(Button::new(
        "Ask about this",
        Action::Live(LiveAction::Ask(
            Live::Task(id.clone()),
            task.map(|task| task.rev),
        )),
    ));
    if !matches!(marks[2], Mark::Next | Mark::Na) {
        buttons.push(
            Button::new(
                "Review candidate",
                Action::Live(LiveAction::Review(id.clone())),
            )
            .quiet(),
        );
    }
    Story {
        title: task.map_or_else(|| member.title.clone(), |task| task.title.clone()),
        plan: Some(format!("· plan {plan}")),
        changed: None,
        context,
        stages,
        progress: None,
        progress_count: None,
        steps: vec![],
        fields: vec![
            field(
                "Next",
                vec![span(task.map_or_else(
                    || member.state.clone(),
                    |task| task.now.clone(),
                ))],
            ),
            you_field,
            field("Needs", needs_rich),
            field("Unblocks", ids(&unblocks[..], " ")),
            field("Cost", cost),
        ],
        buttons,
        id,
    }
}
