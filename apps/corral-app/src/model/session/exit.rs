//! A build's exit: Finish… docks the exit choice (or asks the host to open it), whose options,
//! availability, landing policy and reasons are the host's; after Land, the host's exit phase
//! walks the step rail (rebase, full gate, land), a failed gate shows its repair round, and the
//! outcome says what was landed or kept, or what failed and what to do next. Nothing here
//! predicts a phase: each step reads the phase the host reports.

use corral_protocol::detail::{CardKind, SessionCard};
use corral_protocol::session_host::{SessionAction, SessionActionRequest};
use corral_protocol::session_state::ExitPhase;

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

use super::actions::{ActionGroup, row};
use super::dock::{self, DockAction, Pending};
use super::{Action, Can, Cx, Focus, Host, SessionRole, State};

/// The exit choice's summary: the host's lines, else the build's recorded diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitSummary {
    /// `14 files +412 −88 on build/auth, 3 commits`.
    pub line: Option<String>,
    /// `! Uncommitted changes in the worktree`, Held.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Done,
    Running,
    Failed,
    Waiting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitStep {
    /// `Rebase onto main`, `Full gate`, `Land (fast-forward main)`.
    pub label: String,
    pub state: StepState,
    /// `✓`, `✗`, `○`; none while running (the spinner).
    pub glyph: Option<&'static str>,
    pub hue: Hue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitOutcome {
    pub hue: Hue,
    /// `✓ Landed 3f2a91c on main`, `✗ The exit failed: …`.
    pub text: String,
    /// What to do next: `Inspect the worktree and choose Finish again.`
    pub next: Option<String>,
}

/// Where a build's exit stands, once the owner chose it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExitProgress {
    pub phase: ExitPhase,
    pub steps: Vec<ExitStep>,
    /// Round n of N of a repair.
    pub repair: Option<(u32, u32)>,
    /// The host's detail: a failed gate's text, Held.
    pub detail: Option<(Hue, String)>,
    pub outcome: Option<ExitOutcome>,
    /// `Finish…` to choose again after a failure.
    pub finish: Option<Action>,
}

/// The summary of the exit `card`: its body's lines (a line opening with `!` is a warning),
/// else the build's recorded diff. None when neither says anything.
pub(super) fn summary(cx: &Cx<'_>, card: Option<&SessionCard>) -> Option<ExitSummary> {
    let mut line = None;
    let mut warnings = Vec::new();
    for text in card
        .and_then(|card| card.body.as_deref())
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        match text.strip_prefix('!') {
            Some(warning) => warnings.push(format!("! {}", warning.trim())),
            None if line.is_none() => line = Some(text.to_owned()),
            None => warnings.push(text.to_owned()),
        }
    }
    let line = line.or_else(|| {
        let build = cx.build()?;
        let diff = build.diff.as_ref()?;
        let branch = build
            .branch
            .as_deref()
            .map_or_else(String::new, |branch| format!(" on {branch}"));
        Some(format!(
            "{} files +{} −{}{branch}, {} commits",
            diff.files, diff.added, diff.removed, diff.commits
        ))
    });
    (line.is_some() || !warnings.is_empty()).then_some(ExitSummary { line, warnings })
}

fn step(label: String, state: StepState) -> ExitStep {
    let (glyph, hue) = match state {
        StepState::Done => (Some("✓"), Hue::Ok),
        StepState::Running => (None, Hue::Corral),
        StepState::Failed => (Some("✗"), Hue::Fail),
        StepState::Waiting => (Some("○"), Hue::Neutral),
    };
    ExitStep {
        label,
        state,
        glyph,
        hue,
    }
}

/// The exit's progress, as the host's exit brief reports it; none before a choice runs.
pub fn progress(cx: &Cx<'_>) -> Option<ExitProgress> {
    let exit = cx.exit()?;
    let base = exit
        .base_branch
        .clone()
        .or_else(|| cx.build().and_then(|build| build.base.clone()))
        .unwrap_or_else(|| "the base branch".into());
    let rail = |states: [StepState; 3]| {
        let [rebase, gate, land] = states;
        vec![
            step(format!("Rebase onto {base}"), rebase),
            step("Full gate".into(), gate),
            step(format!("Land (fast-forward {base})"), land),
        ]
    };
    use StepState::{Done, Failed, Running, Waiting};
    let finish = Action::Dock(DockAction::Finish(cx.id.clone()));
    let mut progress = ExitProgress {
        phase: exit.phase,
        steps: Vec::new(),
        repair: exit.repair.map(|repair| (repair.round, repair.of)),
        detail: exit.detail.clone().map(|detail| (Hue::Held, detail)),
        outcome: None,
        finish: None,
    };
    match exit.phase {
        ExitPhase::None | ExitPhase::Pending | ExitPhase::Other => return None,
        ExitPhase::Rebasing => progress.steps = rail([Running, Waiting, Waiting]),
        ExitPhase::Verifying => progress.steps = rail([Done, Running, Waiting]),
        ExitPhase::Repairing => {
            progress.steps = rail([Done, Failed, Waiting]);
            let round = exit.repair.map_or_else(
                || "Repair".to_owned(),
                |repair| format!("Repair {} of {}", repair.round, repair.of),
            );
            let failed = exit.detail.as_deref().map_or_else(
                || "! Full gate failed".to_owned(),
                |detail| format!("! Full gate failed: {detail}"),
            );
            progress.detail = Some((
                Hue::Held,
                format!(
                    "{failed} · {round}: the session is fixing it; Land resumes when it finishes"
                ),
            ));
        }
        ExitPhase::Landing => progress.steps = rail([Done, Done, Running]),
        ExitPhase::Landed => {
            progress.steps = rail([Done, Done, Done]);
            let sha = exit
                .landed
                .as_deref()
                .map(|sha| sha.chars().take(7).collect::<String>());
            progress.outcome = Some(ExitOutcome {
                hue: Hue::Ok,
                text: match sha {
                    Some(sha) => format!("✓ Landed {sha} on {base}"),
                    None => format!("✓ Landed on {base}"),
                },
                next: None,
            });
        }
        ExitPhase::Kept => {
            let build = cx.build();
            let branch = build
                .and_then(|build| build.branch.clone())
                .unwrap_or_else(|| "its branch".into());
            let place = build.map_or_else(|| "its worktree".into(), |build| build.worktree.clone());
            progress.outcome = Some(ExitOutcome {
                hue: Hue::Ok,
                text: format!("✓ Kept on {branch} in {place}"),
                next: None,
            });
        }
        ExitPhase::Failed => {
            progress.detail = None;
            progress.outcome = Some(ExitOutcome {
                hue: Hue::Fail,
                text: match &exit.detail {
                    Some(detail) => format!("✗ {detail}"),
                    None => "✗ The exit failed; the host gave no detail".into(),
                },
                next: Some("Inspect the worktree and choose Finish again.".into()),
            });
            progress.finish = Some(finish);
        }
    }
    Some(progress)
}

/// Finish…: docks the exit choice the host already holds pending, else asks the host to open
/// it. Reopening it leaves reason mode.
pub(super) fn finish(state: &mut State, session: &Id, feed: &Feed) -> Vec<Effect> {
    let pending = dock::briefs(feed, session.as_str())
        .iter()
        .find(|brief| brief.kind == CardKind::Exit)
        .map(|brief| brief.id.clone());
    if let Some(card) = pending {
        let ui = state.ui_mut(session);
        dock::leave(ui, feed.now);
        ui.dock.opened.insert(card);
        ui.focus = Focus::Dock;
        return Vec::new();
    }
    let key = state.ids.next_id();
    let dock = &mut state.ui_mut(session).dock;
    dock.pending
        .insert(key.clone(), Pending::Action { what: "Finish" });
    // The exit choice opens by itself once the host docks it.
    dock.exit_wanted = true;
    vec![Effect::SessionAction {
        key,
        request: SessionActionRequest {
            id: session.to_string(),
            action: SessionAction::ExitOpen,
        },
    }]
}

/// The Message group's `Finish…` (`/finish`), for a build whose host serves its exit.
pub fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    if *cx.role != SessionRole::Build {
        return None;
    }
    let unavailable = match cx.can("exit") {
        Can::Absent => return None,
        Can::Unavailable(why) => Some(why),
        Can::Yes | Can::Unknown => (cx.host != Host::Running)
            .then(|| "the host is not running; resume it first".to_owned()),
    };
    let mut finish = row(
        "finish",
        "Finish…",
        None,
        Action::Dock(DockAction::Finish(cx.id.clone())),
    );
    finish.command = Some("/finish");
    finish.unavailable = unavailable;
    Some(ActionGroup {
        title: "Message",
        rows: vec![finish],
    })
}
