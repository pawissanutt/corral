//! T-669: exact historical subject lookup; no delivery, task or role work is exempted.
use super::*;
use crate::agent::test_support as ts;
use corral_core::work::{self, Counter, HotScope};

#[test]
fn first_published_subject_survives_closed_growth_and_reopen_with_exact_known_cost() {
    for scale in [1, 10] {
        let fx = ts::fx();
        let (_host, _pm) = super::tests::pm(&fx);
        let view = super::tests::question(&fx);
        let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();
        for _ in 0..2 * scale {
            inbox
                .post(Post {
                    kind: "old",
                    urgency: Urgency::Info,
                    author: "fixture",
                    body: "retained old message",
                    binding: MessageBinding::default(),
                    subject: None,
                    max_payload: 4096,
                })
                .unwrap();
        }
        let first = publish(&inbox, &view.question, "corral").unwrap();
        // Imported duplicates cannot replace the first published subject after a crash.
        inbox
            .post(Post {
                kind: "question_wake",
                urgency: Urgency::Wake,
                author: "fixture",
                body: "later duplicate",
                binding: MessageBinding::default(),
                subject: first.subject.clone(),
                max_payload: 4096,
            })
            .unwrap();
        for reopen in [false, true] {
            let store = if reopen {
                corral_core::Store::open(fx.ctx.store.root()).unwrap()
            } else {
                fx.ctx.store.clone()
            };
            let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
            let docs = inbox.records("messages").unwrap();
            let bytes: u64 = docs
                .iter()
                .map(|row| serde_json::to_vec(&row.doc).unwrap().len() as u64)
                .sum();
            assert_eq!(docs.len(), 2 * scale + 2);
            let header_bytes = serde_json::to_vec(
                &store
                    .record_row("pm-inbox", "current")
                    .unwrap()
                    .unwrap()
                    .doc,
            )
            .unwrap()
            .len() as u64;
            let hot = HotScope::enter();
            let before = work::snapshot();
            assert_eq!(publish(&inbox, &view.question, "corral").unwrap(), first);
            let report = work::snapshot().since(&before);
            drop(hot);
            report.check_identity().unwrap();
            assert_eq!(report.followup_calls("T-669", "pm_wake_subject_lookup"), 1);
            assert_eq!(report.followup_calls("T-669", "standalone_inbox_load"), 1);
            assert_eq!(report.followup_calls.len(), 2);
            for counter in Counter::ALL {
                let cells = report.cells(counter);
                let expected = match counter {
                    Counter::RecordDecodes => docs.len() as u64,
                    Counter::DecodedDocBytes => bytes,
                    Counter::ViewsOpened => 1,
                    _ => 0,
                };
                assert_eq!(
                    cells
                        .followup
                        .get(&("T-669".into(), "pm_wake_subject_lookup".into()))
                        .copied()
                        .unwrap_or(0),
                    expected,
                    "{counter:?}"
                );
                let header_expected = match counter {
                    Counter::RecordDecodes | Counter::ViewsOpened => 1,
                    Counter::DecodedDocBytes => header_bytes,
                    _ => 0,
                };
                assert_eq!(
                    cells
                        .followup
                        .get(&("T-669".into(), "standalone_inbox_load".into()))
                        .copied()
                        .unwrap_or(0),
                    header_expected,
                    "header {counter:?}"
                );
                assert_eq!(cells.unattributed, 0, "{counter:?}");
            }
        }
    }
}
