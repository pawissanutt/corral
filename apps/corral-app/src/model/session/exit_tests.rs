use corral_protocol::detail::{CardKind, CardOption};
use corral_protocol::session_state::{
    ExitBrief, ExitChoice, ExitPhase, LandBrief, LandGate, LandStep, LandStepId, LandStepState,
    SendPolicy, SessionState,
};

use super::questions_tests::{SESSION, answers, card, feed_with, option, question};
use super::transcript::LiveSession;
use super::{Action, DockAction, ExitProgress, State, StepState};
use crate::model::Id;
use crate::model::daemon::Feed;

fn land_step(id: LandStepId, state: LandStepState, note: Option<&str>) -> LandStep {
    LandStep {
        id,
        state,
        note: note.map(str::to_owned),
        started_at: None,
        ended_at: None,
    }
}

/// The dock's exit progress of `SESSION` while its host reports `exit`.
fn progress_of(exit: ExitBrief) -> Option<ExitProgress> {
    let mut feed = Feed::default();
    let mut live = LiveSession::default();
    live.replace_state(Some(Box::new(SessionState {
        exit: Some(exit),
        ..SessionState::default()
    })));
    feed.live.insert(Id::from(SESSION), live);
    let id = Id::from(SESSION);
    let facts = super::Facts {
        role: super::SessionRole::Build,
        task: None,
    };
    let state = State::default();
    let env = crate::model::health::daemon_env(&feed, crate::model::nav::Form::Desktop);
    let cx = super::Cx::new(&feed, state.ui(SESSION), &id, env, &facts);
    super::progress(&cx)
}

fn exit(phase: ExitPhase, land: Option<LandBrief>) -> ExitBrief {
    ExitBrief {
        phase,
        detail: None,
        choice: Some(ExitChoice::Land),
        landed: None,
        base_branch: Some("main".into()),
        repair: None,
        land,
    }
}

/// A host that reused an earlier passing gate goes from rebasing to landing; deriving the rail
/// from the phase would claim a gate ran. With the host's steps the rail is theirs, notes
/// included, and a failed exit still shows which step failed.
#[test]
fn the_hosts_land_steps_replace_the_rail_derived_from_the_phase() {
    let steps = vec![
        land_step(LandStepId::Rebase, LandStepState::Done, Some("3f2a9c1")),
        land_step(
            LandStepId::Gate,
            LandStepState::Done,
            Some("G2 passed earlier on this commit"),
        ),
        land_step(LandStepId::Land, LandStepState::Failed, None),
    ];
    let shown = progress_of(exit(
        ExitPhase::Failed,
        Some(LandBrief { steps, gate: None }),
    ));
    let shown = shown.expect("a failed exit has progress");
    let rail: Vec<(StepState, Option<&str>)> = shown
        .steps
        .iter()
        .map(|step| (step.state, step.note.as_deref()))
        .collect();
    assert_eq!(
        rail,
        [
            (StepState::Done, Some("3f2a9c1")),
            (StepState::Done, Some("G2 passed earlier on this commit")),
            (StepState::Failed, None),
        ]
    );
    assert!(shown.outcome.is_some() && shown.finish.is_some());

    let bare = progress_of(exit(
        ExitPhase::Landing,
        Some(LandBrief {
            steps: vec![land_step(LandStepId::Rebase, LandStepState::Done, None)],
            gate: None,
        }),
    ));
    assert_eq!(bare.map(|shown| shown.steps.len()), Some(1));
}

/// The gate's log belongs under a gate that runs or failed; a passed gate's leftover facts must
/// not show a log the host no longer reports as current.
#[test]
fn the_gate_log_shows_for_a_running_or_failed_gate_only() {
    let gate = LandGate {
        log: Some("/tmp/gate-G4.log".into()),
        command: Some("scripts/gate.sh".into()),
        index: Some(1),
        of: Some(2),
        tail: vec!["test result: FAILED".into()],
        ..LandGate::default()
    };
    let log_of = |state: LandStepState, gate: LandGate| {
        let land = LandBrief {
            steps: vec![land_step(LandStepId::Gate, state, None)],
            gate: Some(gate),
        };
        super::land::rows(&land, "main", 0).remove(0).log
    };
    for state in [LandStepState::Running, LandStepState::Failed] {
        let log = log_of(state, gate.clone()).expect("a log under the gate");
        assert_eq!(log.lines.len(), 1);
        assert!(log.head.is_some() && log.path.is_some());
    }
    let passed = LandGate {
        command: None,
        index: None,
        of: None,
        tail: Vec::new(),
        ..gate
    };
    assert_eq!(log_of(LandStepState::Done, passed), None);
}

/// Land over open findings needs the owner's reason, and it is destructive: once confirmed it
/// must still wait for the reason, and an empty reason must not land.
#[test]
fn a_land_that_needs_a_reason_is_not_sent_without_one() {
    let land = CardOption {
        needs_text: Some("1 major finding is open".into()),
        destructive: true,
        ..option("land")
    };
    let feed = feed_with(
        card(
            "exit-1",
            CardKind::Exit,
            vec![question(
                vec![option("keep"), land, option("iterate")],
                false,
            )],
        ),
        SessionState::default(),
    );
    let session = Id::from(SESSION);
    let mut state = State::default();
    let send = |text: &str| Action::Send {
        session: session.clone(),
        text: text.into(),
        policy: SendPolicy::Steer,
        attachments: Vec::new(),
    };

    let chose = state.act(
        Action::Dock(DockAction::Choose {
            session: session.clone(),
            card: "exit-1".into(),
            option: "land".into(),
        }),
        &feed,
    );
    assert!(answers(&chose).is_empty());
    let confirmed = state.act(
        Action::Dock(DockAction::Confirm {
            session: session.clone(),
            yes: true,
        }),
        &feed,
    );
    assert!(answers(&confirmed).is_empty());
    assert!(answers(&state.act(send("   "), &feed)).is_empty());

    let sent = answers(&state.act(send("the finding is a false positive"), &feed));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].card, "exit-1");
    assert_eq!(sent[0].answers[0].options, ["land"]);
    assert_eq!(
        sent[0].answers[0].text.as_deref(),
        Some("the finding is a false positive")
    );
}

/// Land with no commits on the branch is unavailable: its row says why, and does not also ask
/// for a reason to land over open findings.
#[test]
fn an_unavailable_exit_choice_does_not_ask_for_a_reason() {
    let land = CardOption {
        unavailable: Some("There are no commits beyond the session base.".into()),
        needs_text: Some("No approving review covers the current head.".into()),
        ..option("land")
    };
    let asked = question(vec![option("keep"), land], false);
    let shown = super::choice::options("exit-1", &asked, &[], true, |chosen| {
        Action::Dock(DockAction::Choose {
            session: Id::from(SESSION),
            card: "exit-1".into(),
            option: chosen.to_owned(),
        })
    });
    assert!(shown[1].unavailable.is_some());
    assert_eq!(shown[1].needs_text, None);
    assert_eq!(shown[1].action, None);
}
