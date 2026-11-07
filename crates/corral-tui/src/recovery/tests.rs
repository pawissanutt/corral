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
