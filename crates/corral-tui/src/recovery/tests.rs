use std::cell::Cell;

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::draw::wrap;
use super::*;
use crate::board::tests::seeded as fixture;
use crate::board::tests::{buffer_text, code, key};
use crate::choice::tests::{click, locate};
use crate::{HitMap, View};

thread_local! { static CALLS: Cell<usize> = const { Cell::new(0) }; }

fn counting(_: &Ctx, _: &RecoveryPreview, _: &RecoveryInput, _: &str) -> Result<RecoveryRecord> {
    CALLS.with(|c| c.set(c.get() + 1));
    anyhow::bail!("test authorizer records the call only")
}

fn calls() -> usize {
    CALLS.with(Cell::get)
}

fn sheet(ctx: &Ctx) -> RecoverySheet {
    CALLS.with(|c| c.set(0));
    RecoverySheet::with(ctx, "T-002", Kind::ForceAdmission, counting)
}

fn render(view: &mut RecoverySheet, ctx: &Ctx, w: u16, h: u16) -> (String, HitMap) {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| View::draw(view, f, f.area(), ctx, &mut hits))
        .unwrap();
    (buffer_text(term.backend().buffer()), hits)
}

fn type_text(view: &mut RecoverySheet, ctx: &Ctx, text: &str) {
    for c in text.chars() {
        view.on_key(key(c), ctx);
    }
}

#[test]
fn wrap_keeps_every_character_within_width() {
    let text = "a".repeat(50);
    let lines = wrap(&text, 20, "    ");
    assert!(lines.iter().all(|l| l.chars().count() <= 20));
    let joined: String = lines.iter().map(|l| l.trim_start()).collect();
    assert_eq!(joined, text);
}

#[test]
fn opens_previewed_with_cancel_focused_and_esc_has_no_effect() {
    let fx = fixture();
    let before = fx.ctx.store.get("T-002").unwrap();
    let mut s = sheet(&fx.ctx);
    assert!(s.preview.is_some(), "preview is computed on open");
    assert_eq!(s.focused(), Control::Cancel);
    assert_eq!(s.on_key(code(KeyCode::Esc), &fx.ctx), Action::Back);
    assert_eq!(calls(), 0);
    assert_eq!(fx.ctx.store.get("T-002").unwrap(), before);
    assert!(
        recovery::recover_history(&fx.ctx, "T-002")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn enter_in_reason_never_applies_and_continue_requires_a_clean_preview() {
    let fx = fixture();
    let mut s = sheet(&fx.ctx);
    s.focus = s
        .controls()
        .iter()
        .position(|c| *c == Control::Reason)
        .unwrap();
    type_text(&mut s, &fx.ctx, "owner reason");
    assert_eq!(s.reason, "owner reason");
    assert_eq!(s.on_key(code(KeyCode::Enter), &fx.ctx), Action::None);
    assert_eq!(s.step, Step::Edit);
    assert_eq!(calls(), 0);
    // The seeded task has hard blockers: Continue refuses with a visible notice.
    s.focus = s
        .controls()
        .iter()
        .position(|c| *c == Control::Continue)
        .unwrap();
    s.on_key(code(KeyCode::Enter), &fx.ctx);
    assert_eq!(s.step, Step::Edit);
    assert!(s.notice.is_some());
    assert_eq!(calls(), 0);
}

#[test]
fn stale_confirmation_refreshes_the_proposal_and_keeps_the_draft() {
    let fx = fixture();
    let mut s = sheet(&fx.ctx);
    s.reason = "draft reason".into();
    // Reach the confirmation step on the shown proposal, then change task identity underneath.
    s.step = Step::Confirm;
    s.focus = s
        .controls()
        .iter()
        .position(|c| *c == Control::Confirm)
        .unwrap();
    let shown = s.preview.clone();
    fx.ctx
        .store
        .update("T-002", |t| {
            t.body.push_str("\nchanged while the sheet was open\n");
            Ok(())
        })
        .unwrap();
    assert_eq!(s.on_key(code(KeyCode::Enter), &fx.ctx), Action::None);
    assert_eq!(
        calls(),
        0,
        "a stale confirmation never reaches the authorizer"
    );
    assert_eq!(s.step, Step::Edit);
    assert_eq!(s.reason, "draft reason");
    assert_ne!(s.preview, shown, "the proposal was refreshed");
    assert!(
        s.notice
            .as_ref()
            .is_some_and(|(_, t)| t.contains("changed"))
    );
}

#[test]
fn confirmation_with_remaining_blockers_does_not_authorize() {
    let fx = fixture();
    let mut s = sheet(&fx.ctx);
    let Some(Ok(p)) = &s.preview else {
        panic!("preview")
    };
    assert!(
        !p.remaining_blockers.is_empty(),
        "fixture keeps hard blockers"
    );
    s.reason = "r".into();
    s.step = Step::Confirm;
    s.focus = s
        .controls()
        .iter()
        .position(|c| *c == Control::Confirm)
        .unwrap();
    s.on_key(code(KeyCode::Enter), &fx.ctx);
    assert_eq!(calls(), 0);
    assert_eq!(s.step, Step::Edit);
}

#[test]
fn every_control_reachable_by_keyboard_and_mouse_at_80x24() {
    let fx = fixture();
    let mut s = sheet(&fx.ctx);
    let n = s.controls().len();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..n {
        s.on_key(code(KeyCode::Tab), &fx.ctx);
        let (text, _) = render(&mut s, &fx.ctx, 80, 24);
        let f = s.focused();
        seen.insert(format!("{f:?}"));
        // Focused body controls are scrolled into view; buttons sit on the fixed row.
        if !matches!(f, Control::Cancel | Control::Continue) {
            assert!(text.contains('▸'), "{f:?} not visible\n{text}");
        }
    }
    assert_eq!(seen.len(), n);
    // Mouse: action pills switch the action, and the always-visible Cancel button closes without effect.
    let (text, hits) = render(&mut s, &fx.ctx, 80, 24);
    let at = locate(&text, "Action", "accept-code");
    click(&mut s, &fx.ctx, &hits, at, false);
    assert_eq!(s.kind(), Kind::AcceptCode);
    let (text, hits) = render(&mut s, &fx.ctx, 80, 24);
    assert!(text.contains("[Continue…]"), "{text}");
    let at = locate(&text, "[Cancel]", "[Cancel]");
    assert_eq!(click(&mut s, &fx.ctx, &hits, at, false), Action::Back);
    assert_eq!(calls(), 0);
}

#[test]
fn recovery_history_names_exceptions_in_text_with_original_checks() {
    let fx = fixture();
    let s = sheet(&fx.ctx);
    let Some(Ok(mut preview)) = s.preview.clone() else {
        panic!("preview")
    };
    preview.checks = vec![
        corral_core::RecoveryCheckRecord {
            check: RecoveryCheck::GateOutcome,
            observed: corral_core::RecoveryObservation::Gate {
                result: corral_core::GateResult::Fail,
            },
            disposition: CheckDisposition::Overridden,
            evidence: vec![],
        },
        corral_core::RecoveryCheckRecord {
            check: RecoveryCheck::ReviewOutcome,
            observed: corral_core::RecoveryObservation::Review { verdict: None },
            disposition: CheckDisposition::Waived,
            evidence: vec![],
        },
    ];
    preview.request.exceptions = std::collections::BTreeSet::from([
        RecoveryCheck::GateOutcome,
        RecoveryCheck::ReviewOutcome,
    ]);
    let applied = corral_core::RecoveryOutcome {
        state: corral_core::RecoveryState::Applied,
        at: jiff::Timestamp::UNIX_EPOCH,
        detail: "applied".into(),
        actual_effect: Some(preview.proposed_effect.clone()),
        after: None,
        evidence: vec![],
    };
    let record = RecoveryRecord {
        version: corral_core::RECOVERY_VERSION,
        id: "REC-abc".into(),
        preview,
        authorization: corral_core::RecoveryAuthorization {
            version: corral_core::RECOVERY_VERSION,
            preview_digest: "d".into(),
            reason: "owner reason".into(),
            attestation: corral_core::RecoveryAttestation::LocalOwnerAttestation {
                interaction: corral_core::OwnerInteraction::Tui,
                at: jiff::Timestamp::UNIX_EPOCH,
                controlling_terminal: "tui".into(),
                context: None,
            },
        },
        outcomes: vec![],
        links: vec![],
    };
    // Requested but never applied: no exception labels, only the pending operation.
    assert_eq!(
        labels::badges(std::slice::from_ref(&record)),
        ["recovery pending"]
    );
    let mut record = record;
    record.outcomes.push(applied);
    assert_eq!(
        labels::badges(std::slice::from_ref(&record)),
        [
            labels::OWNER_RECOVERY,
            labels::REVIEW_WAIVED,
            labels::CHECK_OVERRIDDEN
        ]
    );
    let text: String = labels::history_lines(&[record])
        .iter()
        .map(|l| l.to_string() + "\n")
        .collect();
    assert!(
        text.contains("gate-outcome: original gate fail → check overridden"),
        "{text}"
    );
    assert!(
        text.contains("review-outcome: original review pending → waived"),
        "{text}"
    );
    assert!(text.contains("not ordinary policy success"), "{text}");
    assert!(text.contains("reason: owner reason"), "{text}");
    assert!(!text.contains("passed"), "{text}");
}

/// A plan-first task whose P1 (two owner decisions) is submitted with no independent review.
fn decisions_fixture() -> (crate::board::tests::Fixture, String) {
    let mut fx = fixture();
    let id = crate::plan::tests::planning(&mut fx);
    let text = crate::plan::tests::PLAN.replace(
        "owner_decisions: []",
        "owner_decisions: [Choose boundary, Pick storage]",
    );
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, text).unwrap();
    corral_ops::plan_submit(&fx.ctx, &id, &path).unwrap();
    (fx, id)
}

fn focus(s: &mut RecoverySheet, control: Control) {
    s.focus = s.controls().iter().position(|c| *c == control).unwrap();
}

#[test]
fn two_owner_decisions_each_need_a_resolution_before_separate_confirmation() {
    let (fx, id) = decisions_fixture();
    CALLS.with(|c| c.set(0));
    let mut s = RecoverySheet::with(&fx.ctx, &id, Kind::AcceptPlan, counting);
    // Both questions are shown, each with its own resolution field.
    assert_eq!(s.questions, ["Choose boundary", "Pick storage"]);
    let (text, _) = render(&mut s, &fx.ctx, 80, 40);
    assert!(
        text.contains("1. Choose boundary") && text.contains("2. Pick storage"),
        "{text}"
    );
    // Keyboard: acknowledge the waiver, resolve only the first decision, write a reason.
    focus(&mut s, Control::Check(0));
    s.on_key(key(' '), &fx.ctx);
    focus(&mut s, Control::Decision(0));
    type_text(&mut s, &fx.ctx, "Keep the closed action set");
    s.on_key(code(KeyCode::Enter), &fx.ctx);
    assert_eq!(
        s.focused(),
        Control::Decision(1),
        "Enter moves on, never applies"
    );
    focus(&mut s, Control::Reason);
    type_text(&mut s, &fx.ctx, "Reviewer unavailable");
    focus(&mut s, Control::Continue);
    s.on_key(code(KeyCode::Enter), &fx.ctx);
    assert_eq!(s.step, Step::Edit, "one open decision blocks confirmation");
    assert_eq!(calls(), 0);
    // Resolve the second decision: Continue reaches the separate confirmation.
    focus(&mut s, Control::Decision(1));
    type_text(&mut s, &fx.ctx, "Rows in the task database");
    focus(&mut s, Control::Continue);
    s.on_key(code(KeyCode::Enter), &fx.ctx);
    assert_eq!(s.step, Step::Confirm, "{:?}", s.notice);
    assert_eq!(
        s.focused(),
        Control::Cancel,
        "confirmation defaults to Cancel"
    );
    assert_eq!(
        s.resolution_notes(),
        "Resolved: Choose boundary => Keep the closed action set\n\
         Resolved: Pick storage => Rows in the task database"
    );
    let (text, hits) = render(&mut s, &fx.ctx, 80, 24);
    assert!(text.contains("Confirm owner recovery"), "{text}");
    assert!(text.contains("[Confirm recovery]"), "{text}");
    assert_eq!(calls(), 0, "reaching confirmation applies nothing");
    // Mouse: the explicit Confirm button reaches the authorizer exactly once.
    let at = locate(&text, "[Confirm recovery]", "[Confirm recovery]");
    click(&mut s, &fx.ctx, &hits, at, false);
    assert_eq!(calls(), 1);
}

#[test]
fn many_reviewers_wrap_so_every_profile_is_visible_and_clickable_at_80x24() {
    let mut fx = fixture();
    for i in 1..=8 {
        fx.ctx.store.config.profiles.insert(
            format!("reviewer{i:02}"),
            corral_core::Profile {
                harness: "claude".into(),
                model: Some(format!("fixture-{i}")),
                effort: None,
                args: Vec::new(),
                vendor: Some("fixture".into()),
                escalate_to: None,
                metadata: None,
            },
        );
    }
    fx.ctx.store.save_config().unwrap();
    CALLS.with(|c| c.set(0));
    let mut s = RecoverySheet::with(&fx.ctx, "T-002", Kind::UseReviewer, counting);
    let n = s.reviewers.len();
    assert!(n >= 12);
    focus(&mut s, Control::Reviewer);
    for _ in 1..n {
        s.on_key(code(KeyCode::Right), &fx.ctx);
    }
    let last = s.reviewers[n - 1].clone();
    let (text, hits) = render(&mut s, &fx.ctx, 80, 24);
    assert!(
        text.contains(&format!("[{last}]")),
        "selected reviewer visible\n{text}"
    );
    for name in &s.reviewers {
        assert!(text.contains(name.as_str()), "{name} reachable\n{text}");
    }
    let at = locate(&text, "reviewer03", "reviewer03");
    click(&mut s, &fx.ctx, &hits, at, false);
    assert_eq!(s.reviewers[s.reviewer], "reviewer03");
    assert_eq!(calls(), 0);
}

#[test]
fn detail_plan_tab_and_review_screen_open_recover_by_key_and_by_click() {
    let (fx, id) = decisions_fixture();
    // Detail Plan tab: key F, then the visible button.
    for by_mouse in [false, true] {
        let mut v = crate::detail::DetailView::plan(&fx.ctx, &id).unwrap();
        let (text, hits) = crate::choice::tests::draw(&mut v, &fx.ctx);
        if by_mouse {
            let at = locate(&text, "[Recover…]", "[Recover…]");
            click(&mut v, &fx.ctx, &hits, at, false);
        } else {
            v.on_key(key('F'), &fx.ctx);
        }
        let (text, _) = crate::choice::tests::draw(&mut v, &fx.ctx);
        assert!(text.contains(&format!("Recover {id}")), "{text}");
        assert!(text.contains("Choose boundary"), "{text}");
        assert_eq!(v.on_key(code(KeyCode::Esc), &fx.ctx), Action::None);
    }
    // Review screen: key F and the Recover… button both ask the app for the same sheet.
    // Review reads the canonical snapshot; give the seeded card a real diff artifact.
    let diff = fx.ctx.store.root().join("review.diff");
    std::fs::write(
        &diff,
        "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-old\n+new\n",
    )
    .unwrap();
    fx.ctx
        .store
        .update("T-006", |task| {
            task.artifacts.clear();
            Ok(())
        })
        .unwrap();
    corral_ops::add_artifact(
        &fx.ctx,
        "T-006",
        &diff,
        Some(corral_core::ArtifactKind::Diff),
        Some("Final diff"),
    )
    .unwrap();
    let mut r = crate::review::ReviewView::new(&fx.ctx, "T-006").unwrap();
    assert_eq!(
        r.on_key(key('F'), &fx.ctx),
        Action::OpenRecover("T-006".into())
    );
    let (text, hits) = crate::choice::tests::draw(&mut r, &fx.ctx);
    let at = locate(&text, "[Recover…]", "[Recover…]");
    assert_eq!(
        click(&mut r, &fx.ctx, &hits, at, false),
        Action::OpenRecover("T-006".into())
    );
    assert!(recovery::recover_history(&fx.ctx, &id).unwrap().is_empty());
}

use crate::board::hot_read_tests::recovery_rows;

#[test]
fn acceptance_detail_shows_populated_recovery_history_beside_original_results() {
    let fx = fixture();
    let mut record = recovery_rows::recovery(&fx.ctx.store);
    record.preview.request.target.task = "T-006".into();
    record.preview.affected_targets = vec!["T-006".into()];
    record
        .preview
        .checks
        .push(corral_core::RecoveryCheckRecord {
            check: RecoveryCheck::ReviewOutcome,
            observed: corral_core::RecoveryObservation::Review {
                verdict: Some(corral_core::Verdict::Changes),
            },
            disposition: CheckDisposition::Waived,
            evidence: vec![],
        });
    record
        .preview
        .request
        .exceptions
        .insert(RecoveryCheck::ReviewOutcome);
    recovery_rows::append_outcome(&mut record, corral_core::RecoveryState::Applied);
    record.outcomes.last_mut().unwrap().actual_effect =
        Some(record.preview.proposed_effect.clone());
    fx.ctx.store.save_recovery_record("T-006", &record).unwrap();
    let mut v = crate::detail::DetailView::acceptance(&fx.ctx, "T-006").unwrap();
    let mut term = Terminal::new(TestBackend::new(160, 120)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| v.draw(f, f.area(), &fx.ctx, &mut hits))
        .unwrap();
    let text = buffer_text(term.backend().buffer());
    for needle in [
        "[owner recovery]",
        "[review waived]",
        "[check overridden]",
        "owner recovery, not ordinary policy success",
        "reviewer-family-policy: original not satisfied → check overridden",
        "review-outcome: original review changes → waived",
        "reason: Use this separate Astra review attempt only",
        "attested: local owner via cli",
        "Gate and review per run",
    ] {
        assert!(text.contains(needle), "{needle} missing\n{text}");
    }
}
