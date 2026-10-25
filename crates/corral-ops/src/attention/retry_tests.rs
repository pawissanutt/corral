use super::*;

fn observed() -> Receipt {
    Receipt {
        envelope: None,
        binding: None,
        observed_at: "2099-01-01T00:00:00Z".parse().unwrap(),
        via: "fixture".into(),
        detail: json!({}),
    }
}

fn episode(store: &Store, id: &str, recipient: &Recipient, kind: &str) -> Condition {
    classify(
        store,
        &store.get(id).unwrap(),
        std::slice::from_ref(recipient),
        false,
    )
    .into_iter()
    .find(|row| row.kind == kind)
    .unwrap()
}

#[test]
fn successful_retries_resolve_only_the_exact_predecessor_across_all_recipients() {
    for index in 0..4 {
        for overflow in [false, true] {
            for terminal in [
                TerminalState::Failed,
                TerminalState::Cancelled,
                TerminalState::Abandoned,
                TerminalState::Failed,
            ]
            .into_iter()
            .enumerate()
            {
                let (root, store, id, recipients) = fixture();
                let recipient = &recipients[index];
                let inbox = Inbox::new(&store, recipient.clone()).unwrap();
                let original = post_at(&inbox, overflow);
                if terminal.0 == 3 {
                    let mut envelope = publish(&inbox);
                    reject(&inbox, &mut envelope, true);
                    fail(&inbox, &envelope);
                } else {
                    inbox
                        .mark_terminal(original.seq, terminal.1, "fixture", "test")
                        .unwrap();
                }
                let rows = classify(&store, &store.get(&id).unwrap(), &recipients, false);
                assert_eq!(
                    message_row(&rows, &original.id).next_action.command,
                    Some(format!("corral inbox retry {}", original.id))
                );
                let replacement = inbox.retry(original.seq).unwrap();
                assert!(replacement.id.starts_with(&format!("{}#", inbox.key())));
                assert!(
                    inbox
                        .retry(original.seq)
                        .unwrap_err()
                        .to_string()
                        .contains("already_retried")
                );
                for reader in [&store, &Store::open_unobserved(root.path()).unwrap()] {
                    let task = reader.get(&id).unwrap();
                    let rows = classify(reader, &task, &recipients, false);
                    assert!(!rows.iter().any(|row| {
                        row.key
                            .as_ref()
                            .is_some_and(|key| key.0.ends_with(&format!("/{}", original.id)))
                    }));
                    let successor = message_row(&rows, &replacement.id);
                    assert_eq!(
                        successor.next_action.recipient.as_deref(),
                        Some(inbox.key())
                    );
                    assert_ne!(
                        successor.next_action.command,
                        Some(format!("corral inbox retry {}", original.id))
                    );
                }
            }
        }
    }
}

#[test]
fn incomplete_retry_claims_never_resolve_or_replay_a_predecessor() {
    for index in 0..4 {
        for overflow in [false, true] {
            for gap in 0..4 {
                let (_root, store, id, recipients) = fixture();
                let inbox = Inbox::new(&store, recipients[index].clone()).unwrap();
                let original = post_at(&inbox, overflow);
                inbox
                    .mark_terminal(original.seq, TerminalState::Failed, "fixture", "test")
                    .unwrap();
                if gap == 3 {
                    let successor = inbox.retry(original.seq).unwrap();
                    std::fs::remove_file(inbox.message_path(successor.seq)).unwrap();
                } else {
                    let mut claim = json!({"at": at()});
                    if gap > 0 {
                        claim["from"] = original.id.clone().into();
                        claim["to"] = if gap == 1 {
                            format!("{}#{}", inbox.key(), original.seq + 1).into()
                        } else {
                            format!("other_R1#{}", original.seq + 1).into()
                        };
                    }
                    std::fs::write(
                        inbox.record_path(original.seq, "retried"),
                        serde_json::to_vec(&claim).unwrap(),
                    )
                    .unwrap();
                }
                let rows = classify(&store, &store.get(&id).unwrap(), &recipients, false);
                let pending = message_row(&rows, &original.id);
                assert_eq!(pending.kind, "delivery_retry_pending");
                assert_eq!(pending.since, Some(original.created));
                assert_eq!(pending.responsible, Responsible::Owner);
                assert_eq!(
                    pending.next_action.command,
                    Some(format!("corral inbox inspect-retry {}", original.id))
                );
                assert!(!inbox.inspect_retry(original.seq).unwrap().eligible);
                assert_eq!(pending.next_action.recipient.as_deref(), Some(inbox.key()));
                assert!(
                    inbox
                        .retry(original.seq)
                        .unwrap_err()
                        .to_string()
                        .contains("already_retried")
                );
            }
        }
    }
}

#[test]
fn overflow_clear_refill_changes_episode_identity_and_survives_restart() {
    let first_at: Timestamp = "2026-10-25T01:00:00Z".parse().unwrap();
    let second_at: Timestamp = "2026-10-25T02:00:00Z".parse().unwrap();
    for index in 0..4 {
        let (root, store, id, recipients) = fixture();
        let recipient = &recipients[index];
        let inbox = Inbox::new(&store, recipient.clone()).unwrap();
        let messages = (0..9).map(|_| post(&inbox)).collect::<Vec<_>>();
        store
            .update(&id, |task| {
                recipient
                    .state_mut(task)?
                    .owner_header
                    .as_mut()
                    .unwrap()
                    .overflow_since = Some(first_at);
                Ok(())
            })
            .unwrap();
        let first = episode(&store, &id, recipient, "inbox_overflow");
        for message in messages {
            inbox
                .record_receipt(message.seq, ReceiptLevel::Observed, &observed())
                .unwrap();
        }
        inbox.maintain_owner_header(observed().observed_at).unwrap();
        let state = inbox.state().unwrap();
        assert!(state.owner_header.unwrap().overflow_entries.is_empty());
        for _ in 0..8 {
            post(&inbox);
        }
        store
            .update(&id, |task| {
                recipient
                    .state_mut(task)?
                    .owner_header
                    .as_mut()
                    .unwrap()
                    .overflow_since = Some(second_at);
                Ok(())
            })
            .unwrap();
        let second = episode(&store, &id, recipient, "inbox_overflow");
        assert_eq!(first.since, Some(first_at));
        assert_eq!(second.since, Some(second_at));
        assert_ne!(first.key, second.key);
        let reopened = Store::open_unobserved(root.path()).unwrap();
        assert_eq!(second, episode(&reopened, &id, recipient, "inbox_overflow"));
    }
}

#[test]
fn indexing_cursor_progress_keeps_identity_but_a_new_debt_episode_changes_it() {
    use corral_core::inbox::{IndexPhase, LegacyDebt, OwnerHeader};
    for index in 0..4 {
        let (root, store, id, recipients) = fixture();
        let recipient = &recipients[index];
        let debt = |started| LegacyDebt {
            phase: IndexPhase::Watermark,
            next_seq: 1,
            end_seq: 2,
            latest_observed_owner: None,
            started,
        };
        store
            .update(&id, |task| {
                let state = recipient.state_mut(task)?;
                state.next_seq = 3;
                state.owner_header = Some(OwnerHeader {
                    legacy_debt: Some(debt(at())),
                    ..Default::default()
                });
                Ok(())
            })
            .unwrap();
        let first = episode(&store, &id, recipient, "inbox_header_index_pending");
        store
            .update(&id, |task| {
                recipient
                    .state_mut(task)?
                    .owner_header
                    .as_mut()
                    .unwrap()
                    .legacy_debt
                    .as_mut()
                    .unwrap()
                    .next_seq = 2;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            first,
            episode(&store, &id, recipient, "inbox_header_index_pending")
        );
        let second_at = "2026-10-26T00:00:00Z".parse().unwrap();
        store
            .update(&id, |task| {
                recipient
                    .state_mut(task)?
                    .owner_header
                    .as_mut()
                    .unwrap()
                    .legacy_debt = Some(debt(second_at));
                Ok(())
            })
            .unwrap();
        let second = episode(&store, &id, recipient, "inbox_header_index_pending");
        assert_eq!(second.since, Some(second_at));
        assert_ne!(first.key, second.key);
        let reopened = Store::open_unobserved(root.path()).unwrap();
        assert_eq!(
            second,
            episode(&reopened, &id, recipient, "inbox_header_index_pending")
        );
    }
}
