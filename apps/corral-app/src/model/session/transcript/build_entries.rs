//! The entries a build session posts for the owner's decisions (`ui/build/build-entries.ts`):
//! the finish card the owner opened (`corral_build_finish`, drawn as the `Build finished`
//! result card), the exit choice and how it ended (`corral_build_exit`: `you chose Iterate`
//! with the host's one-line result) and the board filing (`corral_build_board`: `you chose File
//! and start` with the tasks filed). Their text is what the agent was told; it is not drawn,
//! and opening the entry shows it.

use serde_json::Value;

use crate::model::session::Tab;

use super::build_cards::{BuildCard, ExitLine, finish};
use super::replay::{Cx, Rest};
use super::tool::{ToolState, list_of, str_of};
use super::{Action, Section};

pub(super) const FINISH_ENTRY: &str = "corral_build_finish";
const EXIT_ENTRY: &str = "corral_build_exit";
const BOARD_ENTRY: &str = "corral_build_board";

/// What the host told the agent with an entry, shown once the entry is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Told {
    pub open: bool,
    pub body: Vec<Section>,
    pub action: Action,
}

/// `you chose Keep iterating`, then the host's one-line result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitEntry {
    /// The option's label as the owner saw it.
    pub chose: String,
    pub outcome: ExitLine,
    /// The reason the owner gave to land over open review findings.
    pub reason: Option<String>,
    pub told: Told,
}

/// `you chose File and start`, then `✓ Filed 3 tasks: T-1, T-2, T-3 · queued …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiledEntry {
    pub chose: &'static str,
    pub filed: String,
    pub told: Told,
}

/// The card a build entry reads as, or none when the entry is not one of these in its shape.
pub(super) fn entry(
    title: &str,
    said: &str,
    data: &Value,
    key: &str,
    rest: Rest<'_>,
    cx: Cx<'_>,
) -> Option<BuildCard> {
    let details = data.get("details")?;
    let open = cx.open(key);
    let told = Told {
        open,
        body: if open && !said.trim().is_empty() {
            vec![Section::Text {
                label: "what the agent was told".into(),
                text: said.to_owned(),
            }]
        } else {
            Vec::new()
        },
        action: rest.toggle(key, cx),
    };
    Some(match title {
        FINISH_ENTRY => {
            let agents = Action::Tab {
                session: cx.id.clone(),
                tab: Tab::Agents,
            };
            let mut card = finish(str_of(details, "id")?, None, cx, agents);
            card.told = Some(told);
            BuildCard::Finish(card)
        }
        EXIT_ENTRY => BuildCard::Exit(exit(details, told)?),
        BOARD_ENTRY => BuildCard::Filed(filed(details, told)?),
        _ => return None,
    })
}

/// A sentence ends in its mark.
fn sentence(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() || text.ends_with(['.', '!', '?']) {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}

fn exit(details: &Value, told: Told) -> Option<ExitEntry> {
    let choice = str_of(details, "choice")?;
    let phase = str_of(details, "phase")?;
    let detail = str_of(details, "detail").map(sentence);
    let chose = str_of(details, "label")
        .map(str::to_owned)
        .unwrap_or_else(|| {
            match choice {
                "keep" => "Keep on the branch",
                "land" => "Land on the base branch",
                "iterate" => "Keep iterating",
                other => other,
            }
            .to_owned()
        });
    let line = |state: ToolState, default: &str| ExitLine {
        state,
        words: detail.clone().unwrap_or_else(|| default.to_owned()),
        step: None,
        facts: None,
    };
    let outcome = match phase {
        "rebasing" => line(ToolState::Running, "Rebasing onto the base branch."),
        "verifying" => line(
            ToolState::Running,
            "Running the full gate on the rebased head.",
        ),
        "repairing" => line(
            ToolState::Running,
            "The full gate failed; the session is fixing it before Land runs again.",
        ),
        "landing" => line(ToolState::Running, "Fast-forwarding the base branch."),
        "kept" => line(ToolState::Ok, "Kept on the branch."),
        "failed" => line(ToolState::Failed, "The exit failed."),
        "landed" => {
            let facts = str_of(details, "landed").map(|sha| {
                let mut facts: String = sha.chars().take(7).collect();
                if let Some(base) = str_of(details, "baseBranch") {
                    facts.push_str(&format!(" on {base}"));
                }
                if let Some(gate) = str_of(details, "gateAttempt") {
                    facts.push_str(&format!(" · gate {gate}"));
                }
                facts
            });
            ExitLine {
                facts,
                ..line(ToolState::Ok, "Landed.")
            }
        }
        _ => line(
            ToolState::Waiting,
            if choice == "iterate" {
                "The build session continues; nothing was landed."
            } else {
                "No exit ran."
            },
        ),
    };
    Some(ExitEntry {
        chose,
        outcome,
        reason: str_of(details, "overrideReason")
            .map(|reason| format!("landed over open findings: {reason}")),
        told,
    })
}

fn filed(details: &Value, told: Told) -> Option<FiledEntry> {
    let tasks = details.get("filed")?.as_array()?;
    let started = details.get("started").and_then(Value::as_bool) == Some(true);
    let ids: Vec<&str> = tasks.iter().filter_map(|task| str_of(task, "id")).collect();
    let count = match ids.len() {
        1 => "1 task".to_owned(),
        n => format!("{n} tasks"),
    };
    let listed = if ids.is_empty() {
        String::new()
    } else {
        format!(": {}", ids.join(", "))
    };
    let after = if started {
        " · queued, they start as slots free"
    } else {
        " · in Backlog"
    };
    // Each filed task's title stays with the opened entry.
    let mut told = told;
    if told.open {
        let titles: Vec<String> = tasks
            .iter()
            .filter_map(|task| {
                let id = str_of(task, "id")?;
                let depends: Vec<&str> = list_of(task, "depends")
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                let after = if depends.is_empty() {
                    String::new()
                } else {
                    format!(" (after {})", depends.join(", "))
                };
                Some(format!(
                    "{id}  {}{after}",
                    str_of(task, "title").unwrap_or_default()
                ))
            })
            .collect();
        if !titles.is_empty() {
            told.body.insert(
                0,
                Section::Text {
                    label: "filed".into(),
                    text: titles.join("\n"),
                },
            );
        }
    }
    Some(FiledEntry {
        chose: if started {
            "File and start"
        } else {
            "File only"
        },
        filed: format!("Filed {count}{listed}{after}"),
        told,
    })
}
