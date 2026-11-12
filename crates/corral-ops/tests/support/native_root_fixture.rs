//! Root roles use real hello/PID identity and the same retained native families as task sync.
use crate::agent::{BindTarget, bridge::fixture as bridge, native_test_support as native};
use corral_core::{
    Status, Verdict,
    agent::{AgentPm, AgentRole, AgentRunState, pm_state},
    inbox::{Inbox, ObserveCursor, Recipient, SessionBinding, SessionCursor},
};
use std::path::Path;

pub(super) struct Roots {
    pub fx: bridge::Fx,
    pub assistant: String,
    pub pm: bridge::Bridge,
    pub assist: bridge::Bridge,
}

fn close(run: &mut corral_core::Run, socket: &Path) {
    let now = native::data::at(100_000);
    run.ended = Some(now);
    run.outcome = Some(corral_core::RunOutcome::Done);
    run.inbox.settled = Some(now);
    run.agent_host = None;
    run.obligations.slot_wait = None;
    for wait in &mut run.obligations.waits {
        wait.observation.state = corral_core::WaitState::Cancelled;
    }
    for decision in &mut run.obligations.decisions {
        decision.observation.state = corral_core::WaitState::Cancelled;
    }
    if let Some(p) = &mut run.herdr {
        p.socket = Some(socket.into());
    }
    if let Some(g) = &mut run.gate {
        g.socket = Some(socket.into());
        g.result = corral_core::GateResult::Pass;
    }
    if let Some(r) = &mut run.review {
        r.verdict = Some(Verdict::Approve);
    }
    for child in &mut run.review_runs {
        close(child, socket);
    }
}

fn role_state(mut state: AgentRunState, role: AgentRole, assistant: Option<&str>) -> AgentRunState {
    state.binding.role = role;
    state.binding.project.clear();
    for receipt in &mut state.receipts {
        receipt.identity.role = role;
        receipt.identity.project.clear();
        receipt.identity.task = None;
        receipt.identity.run = None;
        receipt.identity.assistant = assistant.map(str::to_owned);
    }
    state
}

pub(super) fn families(state: &AgentRunState, scale: usize) {
    assert_eq!(state.generations.len(), 2 * scale + 2); // retired prefix, old current, real hello
    for count in [
        state.turns.len(),
        state.receipts.len() - 1,
        state.questions.len(),
        state.wakes.len(),
        state.reports.len(),
        state.jobs.len(),
        state.rewinds.len(),
        state.usage_sessions.len(),
        state.budget.requests.len(),
    ] {
        assert_eq!(count, 2 * scale, "actual canonical retained native family");
    }
    assert_eq!(
        state.binding.health,
        corral_core::agent::HostHealth::Healthy
    );
    assert_eq!(state.binding.generation, 1000);
    assert!(
        state
            .generations
            .iter()
            .filter(|g| g.n != 1000)
            .all(|g| g.fenced.is_some())
    );
    assert!(state.questions.iter().all(|q| !q.recorded_pending()));
}

fn inbox(store: &corral_core::Store, recipient: Recipient, scale: usize) {
    let inbox = Inbox::new(store, recipient).unwrap();
    // The approved T-669 fixture shape: completed effects and retired observation streams.
    inbox
        .update_state(|state| {
            state.effects_applied = (1..=2 * scale as u64).collect();
            state.observe_cursors = (1..=2 * scale as u64)
                .map(|n| SessionCursor {
                    adapter: "fixture".into(),
                    binding: SessionBinding {
                        session: format!("old-{n:04}"),
                        endpoint: None,
                        transcript: None,
                    },
                    cursor: ObserveCursor {
                        offset: n,
                        ..Default::default()
                    },
                })
                .collect();
            Ok(())
        })
        .unwrap();
    assert!(inbox.unfinished_messages().unwrap().is_empty());
}

pub(super) fn roots(
    scale: usize,
    pm_process: &bridge::HostProc,
    assistant_process: &bridge::HostProc,
) -> Roots {
    let fx = super::native_sync_budget::fixture(scale);
    let state = fx
        .task()
        .runs
        .last()
        .unwrap()
        .agent_host
        .as_ref()
        .unwrap()
        .as_ref()
        .clone();
    // No abandoned task pane in the inventory: root panes belong to no historical task.
    std::fs::remove_file(fx.root().join("w0:p1")).unwrap();
    for pane in ["w9:p2", "w9:p3"] {
        std::fs::write(fx.root().join(pane), "").unwrap();
    }
    let socket = fx.ctx.herdr.socket_path().unwrap();
    for mut task in fx.ctx.store.list().unwrap() {
        task.status = Status::Done;
        for run in &mut task.runs {
            close(run, &socket);
        }
        for run in &mut task.plan_runs {
            run.ended = Some(native::data::at(100_000));
            run.state = corral_core::PlanRunState::Approved;
            run.inbox.settled = run.ended;
            if let Some(p) = &mut run.herdr {
                p.socket = Some(socket.clone());
            }
        }
        for plan in &mut task.plans {
            for review in &mut plan.reviews {
                review.verdict = Some(Verdict::Approve);
                review.inbox.settled = Some(native::data::at(100_000));
            }
        }
        for attempt in &mut task.aggregate_attempts {
            for review in &mut attempt.reviews {
                review.verdict = Some(Verdict::Approve);
            }
        }
        fx.ctx.store.fixture_put_task(&task).unwrap();
    }
    let assistant = fx
        .ctx
        .store
        .create_assistant("pi", None, Some("w9:p3"), None)
        .unwrap()
        .id;
    fx.ctx
        .store
        .update_assistant(&assistant, |s| {
            s.body = "Retained assistant body.\n".repeat(20 * scale);
            Ok(())
        })
        .unwrap();
    fx.ctx
        .store
        .update_assistant_container(&assistant, |c| {
            c.state = role_state(state.clone(), AgentRole::Assistant, Some(&assistant));
            Ok(())
        })
        .unwrap();
    pm_state::update(&fx.ctx.store, |pm| {
        *pm = Some(AgentPm::new(
            "w9:p2".into(),
            role_state(state, AgentRole::Pm, None),
        ));
        Ok(((), true))
    })
    .unwrap();
    for _ in 0..2 * scale {
        let old = fx
            .ctx
            .store
            .create_assistant("pi", None, None, None)
            .unwrap();
        fx.ctx.store.end_assistant(&old.id).unwrap();
    }
    inbox(&fx.ctx.store, Recipient::Pm, scale);
    inbox(
        &fx.ctx.store,
        Recipient::Assistant {
            session: assistant.clone(),
        },
        scale,
    );
    // EventCursor is a numeric identity: compare equal-width identities, not decimal
    // width drift. Keep total event counts DIFFERENT, so a history-wide scan still fails.
    let count = fx.ctx.store.events().unwrap().len();
    assert!(count < 10_000);
    let filler = corral_core::Event {
        seq: 0,
        ts: native::data::at(100_000),
        task: None,
        kind: "budget_filler".into(),
        actor: None,
        via: None,
        detail: serde_json::json!({}),
    };
    let line = serde_json::to_string(&filler).unwrap() + "\n";
    fx.ctx
        .store
        .fixture_append_events(&line.repeat(10_000 + 2 * scale - count))
        .unwrap();
    let pm_host = fx.bind(
        &BindTarget::Pm {
            project: String::new(),
        },
        "w9:p2",
        pm_process,
    );
    let assistant_host = fx.bind(
        &BindTarget::Assistant {
            session: assistant.clone(),
        },
        "w9:p3",
        assistant_process,
    );
    for host in [&pm_host, &assistant_host] {
        assert_eq!(host.identity.generation, 1000);
    }
    families(
        &pm_state::load(&fx.ctx.store).unwrap().unwrap().state,
        scale,
    );
    families(
        &fx.ctx
            .store
            .get_assistant_container(&assistant)
            .unwrap()
            .state,
        scale,
    );
    assert_eq!(
        fx.ctx.store.active_assistant_ids().unwrap().as_slice(),
        std::slice::from_ref(&assistant)
    );
    assert!(fx.ctx.store.board(None).unwrap().heads.is_empty());
    let pm = bridge::bridge(
        &fx.ctx.store,
        &Recipient::Pm,
        1000,
        Some(&pm_host.ctl_cap),
        bridge::quiet,
    );
    let assist = bridge::bridge(
        &fx.ctx.store,
        &Recipient::Assistant {
            session: assistant.clone(),
        },
        1000,
        Some(&assistant_host.ctl_cap),
        bridge::quiet,
    );
    Roots {
        fx,
        assistant,
        pm,
        assist,
    }
}
