use corral_core::{PlanReview, PlanRun, PlanRunState, Profile, Status, Verdict};
use corral_ops::{PlanVerdict, plan_spec_revision, plan_submit, plan_verdict};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;

use super::{PlanStage, stage};
use crate::board::tests::{Fixture, buffer_text, code, key, seeded};
use crate::detail::DetailView;
use crate::{Action, HitMap, Mode, Tone, View};

pub(crate) const PLAN: &str = r#"---
goal: Make the widget spin
non_goals: [Reverse spin]
decisions: []
affected_paths: [widget.rs]
contracts: []
dependencies:
  - task: Spin core
    write_scope: [widget.rs]
    depends_on: []
    acceptance: [{scenario: Call spin, expected: turns once}]
  - task: Spin docs
    write_scope: [docs/spin.md]
    depends_on: [Spin core]
acceptance:
  - criterion: C1
    scenario: widget --spin
    expected: clockwise
verification: [cargo test]
owner_decisions: []
---
Spin it with care.
"#;

/// A plan-first task whose planner run D1 (gpt-sol) is still writing the plan.
pub(crate) fn planning(fx: &mut Fixture) -> String {
    let config = &mut fx.ctx.store.config;
    let reviewer = Profile {
        harness: "claude".into(),
        model: Some("fixture-reviewer".into()),
        effort: None,
        args: Vec::new(),
        vendor: None,
        escalate_to: None,
        metadata: None,
    };
    config.profiles.insert("reviewer".into(), reviewer);
    config.review.reviewer = Some("reviewer".into());
    fx.ctx.store.save_config().unwrap();
    let store = &fx.ctx.store;
    let id = store
        .create("Spin widget", Some("corral"), Status::Ready, "Spin it.\n")
        .unwrap()
        .id;
    let task = store
        .update(&id, |t| {
            t.plan_required = Some(true);
            Ok(())
        })
        .unwrap();
    let spec_revision = plan_spec_revision(&task).unwrap();
    let started = fx.now.get();
    store
        .update(&id, |t| {
            t.status = Status::Running;
            t.plan_runs.push(PlanRun {
                agent_host: None,
                execution: None,
                route: None,
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                id: "D1".into(),
                profile: "gpt-sol".into(),
                harness: "omo".into(),
                model: Some("fixture-planner".into()),
                vendor: None,
                effort: None,
                spec_revision,
                started,
                ended: None,
                herdr: None,
                output_dir: store.root().join("plan-out"),
                submitted: None,
                rounds: 0,
                state: PlanRunState::Planning,
                pending_reply: None,
            });
            Ok(())
        })
        .unwrap();
    id
}

/// Submit P1 and seed a pending reviewer attempt PR1 at the store seam (no reviewer pane is launched).
pub(crate) fn reviewing(fx: &Fixture, id: &str) {
    reviewing_text(fx, id, PLAN);
}

pub(crate) fn reviewing_text(fx: &Fixture, id: &str, text: &str) {
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, text).unwrap();
    let plan = plan_submit(&fx.ctx, id, &path).unwrap();
    let probe = PlanVerdict {
        plan: &plan.id,
        attempt: None,
        verdict: Verdict::Changes,
        notes: "digest probe",
    };
    let digest = plan_verdict(&fx.ctx, id, &probe).unwrap().policy_digest;
    let at = fx.now.get();
    fx.ctx
        .store
        .update(id, |t| {
            t.status = Status::Review;
            t.plan_runs[0].state = PlanRunState::Reviewing;
            t.plans[0].reviews = vec![PlanReview {
                agent_host: None,
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                route: None,
                id: "PR1".into(),
                reviewer: "reviewer".into(),
                policy_digest: digest,
                verdict: None,
                notes: None,
                at,
                herdr: None,
                pending_reply: None,
            }];
            Ok(())
        })
        .unwrap();
}

/// The independent reviewer approves PR1, so the plan waits for the owner.
pub(crate) fn awaiting(fx: &Fixture, id: &str) {
    let verdict = PlanVerdict {
        plan: "P1",
        attempt: Some("PR1"),
        verdict: Verdict::Approve,
        notes: "Independent review passed: scope split into two tasks.",
    };
    plan_verdict(&fx.ctx, id, &verdict).unwrap();
}

fn screen(v: &mut dyn View, fx: &Fixture) -> String {
    // Tall enough that the whole Plan tab, snapshot body included, is on screen.
    let mut term = Terminal::new(TestBackend::new(160, 90)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| v.draw(f, f.area(), &fx.ctx, &mut hits))
        .unwrap();
    buffer_text(term.backend().buffer())
}

fn app_screen(fx: &Fixture) -> String {
    let mut app = fx.app(Mode::Board);
    crate::app::mouse_tests::render(&mut app)
}

fn task_stage(fx: &Fixture, id: &str) -> Option<PlanStage> {
    stage(&fx.ctx.store.get(id).unwrap(), Some(&fx.ctx.store.config))
}

pub(crate) fn awaiting_decisions(fx: &mut Fixture, count: usize) -> String {
    let id = planning(fx);
    let decisions = (1..=count)
        .map(|i| format!("Owner decision {i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let text = PLAN.replace(
        "owner_decisions: []",
        &format!("owner_decisions: [{decisions}]"),
    );
    reviewing_text(fx, &id, &text);
    awaiting(fx, &id);
    id
}

#[test]
fn crlf_snapshot_renders_body_only_alternatives_risks_and_rationale() {
    let mut fx = seeded();
    let id = planning(&mut fx);
    let body = [
        "Prefer an explicit rotation over implicit state.",
        "Command and API drift must be checked.",
        "Keep the widget boundary visible to callers.",
    ];
    let text = PLAN
        .replace(
            "Spin it with care.",
            &format!(
                "## Alternatives\n{}\n## Risks\n{}\n## Rationale\n{}\n",
                body[0], body[1], body[2]
            ),
        )
        .replace('\n', "\r\n");
    reviewing_text(&fx, &id, &text);
    awaiting(&fx, &id);
    let mut view = DetailView::plan(&fx.ctx, &id).unwrap();
    let rendered = screen(&mut view, &fx);
    for fact in body {
        assert!(
            rendered.contains(fact),
            "snapshot body missing {fact:?}\n{rendered}"
        );
    }
}

#[test]
fn compact_density_retains_a_decomposed_parents_plan_stage() {
    let mut fx = seeded();
    let id = planning(&mut fx);
    reviewing(&fx, &id);
    awaiting(&fx, &id);
    plan_verdict(
        &fx.ctx,
        &id,
        &PlanVerdict {
            plan: "P1",
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Accept the two scopes",
        },
    )
    .unwrap();
    fx.ctx.store.config.board.card.density = corral_core::Density::Compact;
    let task = fx.ctx.store.get(&id).unwrap();
    let lines = crate::board::card_lines(
        &task,
        &fx.ctx.store.list().unwrap(),
        &fx.ctx.store.config,
        100,
        false,
        fx.now.get(),
        crate::board::CardExtra::default(),
    );
    assert_eq!(lines.len(), 1, "actual Compact-density branch");
    let text = lines[0].to_string();
    assert!(text.contains("decomposed into 2 tasks"), "{text}");
}

#[test]
fn compact_plan_view_keeps_stage_labels_and_snapshot_navigation_visible() {
    // Given an awaiting plan on an 80-column terminal.
    let mut fx = seeded();
    let id = planning(&mut fx);
    reviewing(&fx, &id);
    awaiting(&fx, &id);
    fx.ctx
        .store
        .update(&id, |task| {
            task.plans[0].reviews[0].reviewer = "reviewer-with-a-long-profile-name".into();
            Ok(())
        })
        .unwrap();
    let mut view = DetailView::new(&fx.ctx, &id).unwrap();
    view.on_key(key('8'), &fx.ctx);
    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| view.draw(f, f.area(), &fx.ctx, &mut hits))
        .unwrap();
    let text = buffer_text(term.backend().buffer());
    let lines: Vec<_> = text.lines().collect();
    let plan_row = lines
        .iter()
        .position(|line| line.contains("Your approval >"))
        .unwrap();
    let agent_row = lines
        .iter()
        .position(|line| line.contains("F1 Agent >"))
        .unwrap();
    assert!(plan_row < agent_row);
    assert!(lines[plan_row].contains("Plan review >"));
    assert!(!text.contains("No live run."));
    assert!(text.contains("D1"));
    assert!(lines[plan_row + 2].contains('…'));
    assert!(view.hints().contains(&("pgup/pgdn", "scroll")));
    // When the owner pages through the plan with the advertised keyboard control.
    for _ in 0..12 {
        view.on_key(code(KeyCode::PageDown), &fx.ctx);
    }
    term.draw(|f| view.draw(f, f.area(), &fx.ctx, &mut hits))
        .unwrap();
    // Then the last part of the stored snapshot is reachable.
    assert!(buffer_text(term.backend().buffer()).contains("Spin it with care."));
}

#[test]
fn cancelling_edited_owner_notes_requires_explicit_discard() {
    let mut fx = seeded();
    let id = planning(&mut fx);
    reviewing(&fx, &id);
    awaiting(&fx, &id);
    let mut view = DetailView::plan(&fx.ctx, &id).unwrap();
    view.on_key(key('c'), &fx.ctx);
    for c in "Keep these notes".chars() {
        view.on_key(key(c), &fx.ctx);
    }
    view.on_key(code(KeyCode::Esc), &fx.ctx);
    assert!(screen(&mut view, &fx).contains("Discard owner notes?"));
    assert!(view.has_unsaved_text());
    view.on_key(code(KeyCode::Esc), &fx.ctx);
    assert!(screen(&mut view, &fx).contains("Keep these notes"));
    view.on_key(code(KeyCode::Esc), &fx.ctx);
    view.on_key(code(KeyCode::Enter), &fx.ctx);
    assert!(!view.captures_text());
    assert!(fx.ctx.store.get(&id).unwrap().plans[0].approval.is_none());
}

#[test]
fn cards_follow_the_plan_stage_and_awaiting_plans_need_you() {
    // Given a plan-first task whose planner is writing.
    let mut fx = seeded();
    let id = planning(&mut fx);
    assert_eq!(task_stage(&fx, &id), Some(PlanStage::Planning { round: 0 }));
    let text = app_screen(&fx);
    assert!(text.contains("✎ planning"), "{text}");
    assert!(text.contains("▲ 2 blocked"), "{text}");

    // When the plan is submitted, then the card shows the review round and reviewer.
    reviewing(&fx, &id);
    assert_eq!(task_stage(&fx, &id), Some(PlanStage::Review { round: 1 }));
    let text = app_screen(&fx);
    assert!(text.contains("◎ plan review round 1"), "{text}");
    assert!(text.contains("reviewer reviewer"), "{text}");

    // When the reviewer approves, then the plan waits for the owner in Blocked and the header counts it.
    awaiting(&fx, &id);
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(task_stage(&fx, &id), Some(PlanStage::AwaitingOwner));
    let text = app_screen(&fx);
    assert!(text.contains("awaiting your"), "{text}");
    assert!(text.contains("→ r decide the plan"), "{text}");
    assert!(text.contains("▲ 3 blocked"), "{text}");

    // And Enter opens the plan instead of an implementation review.
    let mut board = fx.board();
    for _ in 0..3 {
        board.on_key(key('l'), &fx.ctx);
    }
    for _ in 0..12 {
        if board.selected_task().is_some_and(|task| task.id == id) {
            break;
        }
        board.on_key(key('j'), &fx.ctx);
    }
    assert_eq!(
        board.on_key(key('r'), &fx.ctx),
        Action::OpenPlan(id.clone())
    );
    assert!(board.reply.is_none());
    assert_eq!(
        board.on_key(code(KeyCode::Enter), &fx.ctx),
        Action::OpenDetail(id)
    );
}

#[test]
fn plan_tab_shows_snapshot_history_and_strip_before_implementation() {
    let mut fx = seeded();
    let id = planning(&mut fx);
    reviewing(&fx, &id);
    awaiting(&fx, &id);
    let mut v = DetailView::plan(&fx.ctx, &id).unwrap();
    let text = screen(&mut v, &fx);
    for needle in [
        "8 Plan",
        "F5 Plan >",
        "Plan review >",
        "Your approval >",
        "Tasks >",
        "F1 Agent >",
        "waiting on you",
        "round 1 · reviewer",
        "[Approve plan]",
        "[Request changes]",
        "▲ awaiting your approval · plan P1 (revision 1 of 1) · planner gpt-sol",
        "Child tasks (0)",
        "2 planned tasks are created in Backlog on owner approval.",
        "Revisions",
        "PR1 reviewer · approve",
        "Independent review passed",
        "Plan snapshot P1",
        "Make the widget spin",
        "Reverse spin",
        "Spin core writes widget.rs",
        "accept: Call spin → turns once",
        "[C1] widget --spin → clockwise",
        "Spin it with care.",
    ] {
        assert!(text.contains(needle), "missing {needle:?}\n{text}");
    }
    let strip = text.lines().find(|l| l.contains("F5 Plan >")).unwrap();
    assert!(
        strip.find("Tasks >") < strip.find("F1 Agent >"),
        "plan cells lead the implementation stages\n{text}"
    );
    assert!(v.hints().contains(&("a", "approve plan")));

    // And tasks without a plan show Timeline last.
    let other = fx.ctx.store.list().unwrap();
    let plain = other
        .iter()
        .find(|t| t.status == Status::Running && t.id != id)
        .unwrap();
    let mut v = DetailView::plan(&fx.ctx, &plain.id).unwrap();
    let text = screen(&mut v, &fx);
    assert!(
        text.contains("6 Comments") && text.contains("7 Timeline") && !text.contains("8 Plan"),
        "{text}"
    );
    assert!(!text.contains("F5 Plan >"), "{text}");
    assert!(v.hints().contains(&("1-7", "tabs")));
}

#[test]
fn keyboard_owner_approval_records_the_decision_and_links_children() {
    let mut fx = seeded();
    let id = planning(&mut fx);
    reviewing(&fx, &id);
    awaiting(&fx, &id);
    let mut v = DetailView::plan(&fx.ctx, &id).unwrap();
    assert_eq!(v.on_key(key('a'), &fx.ctx), Action::None);
    let text = screen(&mut v, &fx);
    assert!(text.contains("approve plan P1 of"), "{text}");
    assert!(text.contains("note › Approved from the board."), "{text}");
    assert!(v.captures_text());

    let out = v.on_key(code(KeyCode::Enter), &fx.ctx);
    assert!(
        matches!(&out, Action::Toast(t) if t.tone == Tone::Good && t.body.contains("decomposed into 2")),
        "{out:?}"
    );
    let task = fx.ctx.store.get(&id).unwrap();
    let plan = &task.plans[0];
    assert_eq!(plan.approval.as_ref().unwrap().by, "owner");
    let last = plan.reviews.last().unwrap();
    assert_eq!(
        (last.reviewer.as_str(), last.verdict),
        ("owner", Some(Verdict::Approve))
    );
    assert_eq!(task.children.len(), 2);
    assert_eq!(task_stage(&fx, &id), Some(PlanStage::Decomposed(2)));

    v.refresh(&fx.ctx);
    let text = screen(&mut v, &fx);
    assert!(text.contains("⊞ decomposed into 2 tasks"), "{text}");
    assert!(text.contains("Child tasks (2)"), "{text}");
    assert!(
        text.contains(&format!("▸ {}  Spin core", task.children[0])),
        "{text}"
    );
    assert!(text.contains("2 tasks"), "strip Tasks cell\n{text}");
    assert!(
        !text.contains("[Approve plan]"),
        "no decision is due\n{text}"
    );
    let first = Action::OpenDetail(task.children[0].clone());
    assert_eq!(v.on_key(code(KeyCode::Enter), &fx.ctx), first);
    v.on_key(key('j'), &fx.ctx);
    let second = Action::OpenDetail(task.children[1].clone());
    assert_eq!(v.on_key(code(KeyCode::Enter), &fx.ctx), second);

    let text = app_screen(&fx);
    assert!(text.contains("decomposed into 2"), "Done card\n{text}");
}

#[test]
fn keyboard_request_changes_needs_notes_and_records_owner_changes() {
    let mut fx = seeded();
    let id = planning(&mut fx);
    reviewing(&fx, &id);
    awaiting(&fx, &id);
    let mut v = DetailView::plan(&fx.ctx, &id).unwrap();
    v.on_key(key('c'), &fx.ctx);
    let out = v.on_key(code(KeyCode::Enter), &fx.ctx);
    assert!(
        matches!(&out, Action::Toast(t) if t.tone == Tone::Warn),
        "{out:?}"
    );
    assert!(screen(&mut v, &fx).contains("request plan changes P1 of"));

    for c in "Split the docs task".chars() {
        v.on_key(key(c), &fx.ctx);
    }
    assert!(v.has_unsaved_text());
    let out = v.on_key(code(KeyCode::Enter), &fx.ctx);
    assert!(
        matches!(&out, Action::Toast(t) if t.tone == Tone::Info),
        "{out:?}"
    );
    let task = fx.ctx.store.get(&id).unwrap();
    let last = task.plans[0].reviews.last().unwrap();
    assert_eq!(
        (last.reviewer.as_str(), last.verdict, last.notes.as_deref()),
        ("owner", Some(Verdict::Changes), Some("Split the docs task"))
    );
    assert!(task.plans[0].approval.is_none());
    assert_eq!(task.plan_runs[0].state, PlanRunState::Changes);
    assert!(!v.captures_text(), "the sheet closed");
}
