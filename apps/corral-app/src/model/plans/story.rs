//! The selected member's Story inspector. `story` resolves the fixture's member from the shared
//! board snapshot; `live_story.rs` builds the same inspector from corrald's board and plan.

use super::{Action, Button, ButtonKind, Chip};
use crate::model::Id;
use crate::model::board::{Actor, LAST_VISIT, Mark, STAGES, money, task};
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};

/// A stage in the selected member's run/gate/review/land stepper.
#[derive(Debug, Clone, PartialEq)]
pub struct StoryStage {
    /// Stage label.
    pub label: &'static str,
    /// Delivery mark.
    pub mark: Mark,
    /// State word, or “waits on you” for the owner's run stage.
    pub status: &'static str,
    /// Run/profile, candidate, prerequisite stage or delivery branch.
    pub sub: String,
    /// Colour of the active state, independent of selection.
    pub hue: Hue,
}

/// A definition-list line in the selected member's story.
#[derive(Debug, Clone, PartialEq)]
pub struct StoryField {
    /// Left-hand field label.
    pub label: &'static str,
    /// Complete styled value, with separators already present.
    pub value: Rich,
    /// Optional action instead of a textual value.
    pub button: Option<Button>,
}

/// Fully resolved inspector. Empty stage/step/button lists are the queued-member variant.
#[derive(Debug, Clone, PartialEq)]
pub struct Story {
    /// Selected task id.
    pub id: Id,
    /// The full task title.
    pub title: String,
    /// Optional parent-plan label next to the id.
    pub plan: Option<String>,
    /// Changed-since-last-visit chip, absent for the compact queued story.
    pub changed: Option<Chip>,
    /// Task context, excluding the separate illustrative-machine note.
    pub context: Rich,
    pub context_note: Option<&'static str>,
    /// Delivery stepper, omitted for queued tasks without steps.
    pub stages: Vec<StoryStage>,
    /// Progress heading, absent for the queued-member variant.
    pub progress: Option<&'static str>,
    /// Done/total fraction shown at the progress heading's trailing edge.
    pub progress_count: Option<String>,
    /// Worker progress marks and text.
    pub steps: Vec<(Mark, String)>,
    /// Next, You, Needs, Unblocks and Cost, or the queued task's compact fields.
    pub fields: Vec<StoryField>,
    /// Open session, Ask about this and Review candidate, absent for queued tasks.
    pub buttons: Vec<Button>,
}

/// Ids in mono joined by `separator`, or `−` when there are none.
pub(super) fn ids<S: AsRef<str>>(ids: &[S], separator: &str) -> Rich {
    let mut rich = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        if i > 0 {
            rich.push(span(separator));
        }
        rich.push(span(id.as_ref()).mono());
    }
    if rich.is_empty() {
        rich.push(span("−"));
    }
    rich
}

fn needs(id: &'static str) -> Rich {
    let mut rich = Vec::new();
    for (i, (id, mark)) in task(id).needs.iter().enumerate() {
        if i > 0 {
            rich.push(span(" · "));
        }
        rich.push(span(*id).mono());
        rich.push(span(format!(" {}", mark.glyph())));
    }
    rich
}

pub(super) fn field(label: &'static str, value: Rich) -> StoryField {
    StoryField {
        label,
        value,
        button: None,
    }
}

/// Build the desktop selected-member Story inspector without importing the Work domain.
pub fn story(id: &'static str) -> Story {
    let t = task(id);
    let simple = t.steps.is_empty();
    let mut view = Story {
        id: Id::from(id),
        title: t.title.into(),
        plan: if simple {
            None
        } else {
            t.plan.map(|id| format!("· plan {id}"))
        },
        changed: if !simple && t.changed {
            Some(Chip {
                label: format!("▲ changed since {LAST_VISIT}"),
                hue: Hue::Neutral,
            })
        } else {
            None
        },
        context: vec![],
        context_note: None,
        stages: vec![],
        progress: None,
        progress_count: None,
        steps: t
            .steps
            .iter()
            .map(|&(mark, words)| (mark, words.to_owned()))
            .collect(),
        fields: vec![],
        buttons: vec![],
    };
    if simple {
        view.context = vec![span(format!(
            "{}{}",
            t.plan.map_or(String::new(), |id| format!("Plan {id} · ")),
            t.now
        ))];
        let wait = if t.needs.is_empty() {
            vec![span("capacity")]
        } else {
            needs(id)
        };
        view.fields = vec![
            field("Waits on", wait),
            field("Unblocks", ids(t.unblocks, " ")),
            field("You", vec![span("nothing needed").hue(Hue::Ok)]),
        ];
        return view;
    }
    if let Some(machine) = t.machine {
        view.context = vec![span(format!(
            "{} · on {machine} · started {}",
            t.agent.unwrap_or(""),
            t.started.unwrap_or("")
        ))];
        view.context_note = Some("machine name illustrative");
    }
    let sha = match id {
        "T-890" => "1751bd2",
        "T-894" => "6b36916",
        "T-867" => "5949a4b",
        "T-852" => "af7a6dc",
        _ => "after run",
    };
    let is_you = t.actor == Actor::You;
    view.stages = STAGES
        .into_iter()
        .enumerate()
        .map(|(i, label)| {
            let mark = t.rail[i];
            StoryStage {
                label,
                mark,
                status: if is_you && i == 0 {
                    "waits on you"
                } else {
                    match mark {
                        Mark::Done => "done",
                        Mark::Active => "now",
                        Mark::Fail => "failed",
                        Mark::Stale => "stale",
                        Mark::Next => "next",
                        Mark::Na => "n/a",
                    }
                },
                sub: match i {
                    0 => format!("{} · {}", t.run.unwrap_or("R1"), t.agent.unwrap_or("")),
                    1 => sha.into(),
                    2 => "after gate".into(),
                    _ => "main".into(),
                },
                hue: if mark == Mark::Active {
                    if i == 0 {
                        if is_you { Hue::You } else { Hue::Worker }
                    } else {
                        Hue::Corral
                    }
                } else if mark == Mark::Stale {
                    Hue::Held
                } else if mark == Mark::Fail {
                    Hue::Fail
                } else {
                    Hue::Neutral
                },
            }
        })
        .collect();
    view.progress = Some("Progress");
    view.progress_count = Some(format!(
        "{}/{}",
        t.steps
            .iter()
            .filter(|(mark, _)| *mark == Mark::Done)
            .count(),
        t.steps.len()
    ));
    let next = t.next.map_or_else(Vec::new, |(who, what)| {
        vec![span(who).bold(), span(format!(" · {what}"))]
    });
    let mut you = field(
        "You",
        if is_you {
            vec![]
        } else {
            vec![span("nothing needed").hue(Hue::Ok)]
        },
    );
    if is_you {
        you.button = Some(Button {
            kind: ButtonKind::Standard,
            ..Button::new("Answer T-868/R1/Q1", Action::Question)
        });
    }
    let mut cost = vec![
        span(if t.cost.is_some() {
            money(t.cost)
        } else {
            "−".into()
        }),
        span(format!(" · {}", t.agent.unwrap_or(""))),
    ];
    if t.hot() {
        cost.push(span(" · over the $20 warning").hue(Hue::Held));
    }
    view.fields = vec![
        field("Next", next),
        you,
        field("Needs", needs(id)),
        field("Unblocks", ids(t.unblocks, " ")),
        field("Cost", cost),
    ];
    view.buttons = vec![
        Button::new("Open session", Action::OpenSession(id)),
        Button::new("Ask about this", Action::Ask(id)),
        Button {
            kind: ButtonKind::Quiet,
            ..Button::new("Review candidate", Action::ReviewCandidate)
        },
    ];
    view
}
