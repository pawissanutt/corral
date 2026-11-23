//! A build's exit: Finish… docks the exit choice (or asks the host to open it), whose options,
//! availability, landing policy and reasons are the host's; after Land, the step rail (rebase,
//! full gate, land) is the host's own steps with their notes, times and the gate's log when it
//! reports them (`land.rs`), else the host's exit phase walks it; a failed gate shows its repair
//! round, and the outcome says what was landed or kept, or what failed and what to do next, under
//! the rail that shows which step failed. Nothing here predicts a phase or a step.
//!
//! A host that prepares a Land candidate reports it: the choice's summary names the exact
//! fast-forward Land authorizes (base commit → candidate head) or what holds it back, and a
//! rebase or gate run while it is prepared reads as preparation, not as Land. A host that
//! reports no candidate keeps the rail above and is never shown as having prepared one.

use corral_protocol::detail::{CardKind, SessionCard};
use corral_protocol::session_host::{SessionAction, SessionActionRequest};
use corral_protocol::session_state::{ExitPhase, LandCandidateState};

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

use super::actions::{ActionGroup, row};
use super::dock::{self, DockAction, Pending};
use super::land::{self, CandidateView, GateLog};
use super::{Action, Can, Cx, Focus, Host, SessionRole, State};

/// The exit choice's summary: the host's lines, else the build's recorded diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitSummary {
    /// `14 files +412 −88 on build/auth, 3 commits`.
    pub line: Option<String>,
    /// `! Uncommitted changes in the worktree`, Held.
    pub warnings: Vec<String>,
    /// The Land candidate the host prepared: what Land would fast-forward, or what holds it back.
    pub candidate: Option<CandidateView>,
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
    /// The host's fact about the step, muted after the label: `3f2a9c1`, `G4 passed`.
    pub note: Option<String>,
    /// `3m 12s`.
    pub time: Option<String>,
    /// The gate's log, under the gate step.
    pub log: Option<GateLog>,
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
    /// The candidate while it is prepared, or once it held a Land back (the base moved).
    pub candidate: Option<CandidateView>,
}

/// The summary of the exit `card`: its body's lines (a line opening with `!` is a warning),
/// else the build's recorded diff, and the candidate the host prepared. None when none of them
/// says anything.
pub(super) fn summary(cx: &Cx<'_>, card: Option<&SessionCard>) -> Option<ExitSummary> {
    let candidate = cx
        .exit()
        .and_then(|exit| exit.candidate.as_ref())
        .map(land::candidate);
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
    (line.is_some() || !warnings.is_empty() || candidate.is_some()).then_some(ExitSummary {
        line,
        warnings,
        candidate,
    })
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
        note: None,
        time: None,
        log: None,
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
        candidate: None,
    };
    let candidate = exit.candidate.as_ref();
    // A rebase or gate while the candidate is prepared lands nothing: it shows as preparation,
    // its own steps under it, and no Land rail.
    if let Some(prepared) = candidate.filter(|c| c.state == LandCandidateState::Preparing)
        && !matches!(
            exit.phase,
            ExitPhase::None | ExitPhase::Pending | ExitPhase::Other
        )
    {
        progress.candidate = Some(land::candidate(prepared));
        progress.steps = exit.land.as_ref().map_or_else(Vec::new, |land| {
            land_steps(land::rows(land, &base, cx.feed.now))
        });
        return Some(progress);
    }
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
            // A host that reports its steps words the failure itself (`Gate G4 failed: …`).
            let failed = match (exit.detail.as_deref(), exit.land.is_some()) {
                (Some(detail), true) => {
                    format!("! {}", detail.strip_suffix('.').unwrap_or(detail))
                }
                (Some(detail), false) => format!("! Full gate failed: {detail}"),
                (None, _) => "! Full gate failed".to_owned(),
            };
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
            // The base fast-forwarded from the tip the candidate was prepared on, when the
            // host says it landed that candidate.
            let from = candidate
                .filter(|c| c.head.is_some() && c.head == exit.landed)
                .and_then(|c| c.base_tip.as_deref())
                .map_or_else(String::new, |tip| {
                    format!(
                        ", fast-forwarded from {}",
                        tip.chars().take(7).collect::<String>()
                    )
                });
            progress.outcome = Some(ExitOutcome {
                hue: Hue::Ok,
                text: match sha {
                    Some(sha) => format!("✓ Landed {sha} on {base}{from}"),
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
            // A Land its candidate held back (the base moved, say) shows why, apart from the
            // step that failed.
            progress.candidate = candidate
                .filter(|c| c.state != LandCandidateState::Ready)
                .map(land::candidate);
        }
    }
    if let Some(land) = &exit.land {
        progress.steps = land_steps(land::rows_with(land, &base, cx.feed.now, candidate));
    }
    Some(progress)
}

fn land_steps(rows: Vec<land::LandRow>) -> Vec<ExitStep> {
    rows.into_iter()
        .map(|row| ExitStep {
            note: row.note,
            time: row.time,
            log: row.log,
            ..step(row.label, row.state)
        })
        .collect()
}

/// Explicit Finish asks the host to prepare again even if a previous blocked card remains.
/// Simply displaying a pending card never sends this action.
pub(super) fn finish(state: &mut State, session: &Id, feed: &Feed) -> Vec<Effect> {
    let cards = dock::briefs(feed, session.as_str());
    let pending = cards
        .iter()
        .find(|brief| brief.kind == CardKind::Exit)
        .map(|brief| brief.id.clone());
    if let Some(card) = pending {
        let ui = state.ui_mut(session);
        dock::leave(ui, feed.now);
        dock::unfold(ui, card, CardKind::Exit, cards, feed.now);
        ui.focus = Focus::Dock;
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
