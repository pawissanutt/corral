//! The owner board's confirmation: trusted inside herdr, never in a managed-agent context, and
//! bound to the exact current preview. The CLI ceremony keeps its outside-herdr fence.
use super::*;

#[test]
fn board_confirmation_applies_once_inside_herdr_while_the_cli_fence_remains() {
    if isolated_env(
        "tui::board_confirmation_applies_once_inside_herdr_while_the_cli_fence_remains",
        &[("HERDR_PANE_ID", "w1:p1")],
    ) {
        return;
    }
    let f = Fixture::new(true);
    let input = f.input();
    let preview = recover_preview(&f.ctx, &f.id, &input).unwrap();
    // A CLI-surface confirmation inside a herdr pane is still refused, with no record.
    let error = recover_apply(&f.ctx, &preview, &input, Some(confirmation(&preview))).unwrap_err();
    assert!(error.to_string().contains("outside herdr"), "{error:#}");
    assert!(records(&f.ctx, &f.id).unwrap().is_empty());
    // A blank board reason is refused before any intent is written.
    assert!(recover_confirm_tui(&f.ctx, &preview, &input, "  ").is_err());
    assert!(records(&f.ctx, &f.id).unwrap().is_empty());
    let token = recover_confirm_tui(&f.ctx, &preview, &input, " Board owner reason ").unwrap();
    let record = recover_apply(&f.ctx, &preview, &input, Some(token)).unwrap();
    assert_eq!(
        state(&record).unwrap(),
        RecoveryState::Applied,
        "{:?}",
        record.outcomes
    );
    assert_eq!(record.authorization.reason, "Board owner reason");
    let RecoveryAttestation::LocalOwnerAttestation {
        interaction,
        controlling_terminal,
        ..
    } = &record.authorization.attestation;
    assert_eq!(*interaction, OwnerInteraction::Tui);
    assert_eq!(controlling_terminal, "tui:w1:p1");
    // An identical retry returns the durable record without a second effect.
    assert_eq!(
        recover_apply(&f.ctx, &preview, &input, None).unwrap(),
        record
    );
    assert_eq!(records(&f.ctx, &f.id).unwrap().len(), 1);
}

#[test]
fn board_confirmation_is_refused_in_a_managed_agent_context() {
    if isolated_env(
        "tui::board_confirmation_is_refused_in_a_managed_agent_context",
        &[("HERDR_PANE_ID", "w1:p2"), ("CORRAL_AGENT_ROLE", "pm")],
    ) {
        return;
    }
    let f = Fixture::new(true);
    let input = f.input();
    let preview = recover_preview(&f.ctx, &f.id, &input).unwrap();
    let error = recover_confirm_tui(&f.ctx, &preview, &input, "Owner reason").unwrap_err();
    assert!(error.to_string().contains("managed context"), "{error:#}");
    assert!(records(&f.ctx, &f.id).unwrap().is_empty());
}

#[test]
fn board_confirmation_refuses_a_stale_preview() {
    if isolated("tui::board_confirmation_refuses_a_stale_preview") {
        return;
    }
    let f = Fixture::new(true);
    let input = f.input();
    let preview = recover_preview(&f.ctx, &f.id, &input).unwrap();
    f.ctx
        .store
        .update(&f.id, |t| {
            t.title = "Changed while the sheet was open".into();
            Ok(())
        })
        .unwrap();
    let error = recover_confirm_tui(&f.ctx, &preview, &input, "Owner reason").unwrap_err();
    assert!(error.to_string().contains("recovery_stale"), "{error:#}");
    assert!(records(&f.ctx, &f.id).unwrap().is_empty());
}
