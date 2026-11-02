//! Test-only canonical store observer for Pi cross-lane fixtures. No public CLI or SQL.
use corral_core::{
    Store,
    inbox::{Inbox, Recipient},
};
use notify::{
    RecursiveMode, Watcher,
    event::{AccessKind, AccessMode},
};
use serde_json::{Value, json};
use std::io::{Read, Write};

fn emit(value: Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "CORRAL_STORE_PROBE {value}").unwrap();
    out.flush().unwrap();
}

#[test]
#[ignore = "driven by the private Pi integration fixtures"]
fn canonical_store_probe() {
    let Ok(spec) = std::env::var("CORRAL_FIXTURE_STORE_PROBE") else {
        return;
    };
    let spec: Value = serde_json::from_str(&spec).unwrap();
    let store = Store::open(std::path::Path::new(spec["root"].as_str().unwrap())).unwrap();
    match spec["mode"].as_str().unwrap() {
        "paths" => {
            emit(json!({"launch_exits":store.scratch_path("tmp").unwrap().join("launch-exits")}))
        }
        "artifact" => {
            let ctx = crate::Ctx::from_store(store);
            let artifact = crate::artifact_storage::resolve_artifact(
                &ctx,
                spec["reference"].as_str().unwrap(),
            )
            .unwrap();
            let mut text = String::new();
            crate::artifact_storage::artifact_reader(&ctx, &artifact)
                .unwrap()
                .take(8 * 1024 * 1024 + 1)
                .read_to_string(&mut text)
                .unwrap();
            assert!(
                text.len() <= 8 * 1024 * 1024,
                "fixture artifact exceeds read bound"
            );
            emit(json!({"text":text}));
        }
        "gate" => {
            let ctx = crate::Ctx::from_store(store);
            let paths = crate::gate::gate_paths(
                &ctx,
                spec["task"].as_str().unwrap(),
                spec["run"].as_str().unwrap(),
            );
            emit(json!({"result":paths.result,"script":paths.script,"log":paths.log}));
        }
        "admit" => {
            let recipient = Recipient::parse_key(spec["recipient"].as_str().unwrap()).unwrap();
            let ctx = crate::Ctx::from_store(store);
            let status = crate::delivery::deliver(&ctx, &recipient, &AdmissionOnly, None).unwrap();
            emit(json!({"status":format!("{status:?}")}));
        }
        "inbox" => {
            let recipient = Recipient::parse_key(spec["recipient"].as_str().unwrap()).unwrap();
            let inbox = Inbox::new(&store, recipient).unwrap();
            let messages = inbox.messages().unwrap();
            let entries: Vec<_> = messages
                .iter()
                .map(|m| inbox.entry(m.seq).unwrap())
                .collect();
            let messages: Vec<_> = messages
                .into_iter()
                .map(|m| {
                    let mut value = serde_json::to_value(&m).unwrap();
                    value["body"] = m.body.into();
                    value
                })
                .collect();
            let envelopes: Vec<_> = inbox
                .envelopes()
                .unwrap()
                .into_iter()
                .map(|e| {
                    let mut value = serde_json::to_value(&e).unwrap();
                    value["payload"] = inbox.payload(&e).unwrap().into();
                    value
                })
                .collect();
            emit(json!({"messages":messages,"entries":entries,"envelopes":envelopes}));
        }
        "watch" => {
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            let failure = std::sync::Arc::new(std::sync::Mutex::new(None));
            let callback_failure = std::sync::Arc::clone(&failure);
            // Notification only, never an authority: every payload and cursor is read through
            // Store. Cross-lane writers are real corral command processes; CLOSE_WRITE is
            // after their commit, unlike a pre-commit data notification. No timer/poll loop.
            let mut watcher = notify::recommended_watcher(
                move |event: notify::Result<notify::Event>| match event {
                    Ok(event)
                        if matches!(
                            event.kind,
                            notify::EventKind::Access(AccessKind::Close(AccessMode::Write))
                        ) =>
                    {
                        let _ = tx.try_send(());
                    }
                    Err(error) => {
                        *callback_failure.lock().unwrap() = Some(error);
                        let _ = tx.try_send(());
                    }
                    _ => {}
                },
            )
            .unwrap();
            watcher
                .watch(store.dir(), RecursiveMode::Recursive)
                .unwrap();
            // Arm first, then capture the exact canonical tail and acknowledge readiness.
            // Actions cannot start until the parent receives this frame.
            let mut reader = store.events_since(None).unwrap();
            emit(json!({"ready":reader.cursor().seq}));
            loop {
                if let Some(error) = failure.lock().unwrap().take() {
                    panic!("canonical notification failed: {error}");
                }
                // EventReader pages by persisted sequence; queued notifications cannot
                // create duplicates and events between empty read and recv are not lost.
                while let Some((event, _)) = reader.next_event().unwrap() {
                    emit(json!({"event":event}));
                }
                rx.recv().unwrap();
            }
        }
        mode => panic!("unknown fixture probe mode {mode}"),
    }
}

/// Keep admission and canonical observation separate so tests can force a fast model to
/// run before the observer, without sleeps, host load or weakening production guards.
struct AdmissionOnly;

impl crate::delivery::DeliveryAdapter for AdmissionOnly {
    fn harness(&self) -> &'static str {
        crate::agent::delivery::CorralPiAdapter.harness()
    }
    fn capabilities(&self) -> crate::delivery::Capabilities {
        crate::agent::delivery::CorralPiAdapter.capabilities()
    }
    fn readiness(
        &self,
        ctx: &crate::Ctx,
        recipient: &Recipient,
    ) -> anyhow::Result<crate::delivery::Readiness> {
        crate::agent::delivery::CorralPiAdapter.readiness(ctx, recipient)
    }
    fn bind(
        &self,
        ctx: &crate::Ctx,
        recipient: &Recipient,
    ) -> anyhow::Result<crate::delivery::BindOutcome> {
        crate::agent::delivery::CorralPiAdapter.bind(ctx, recipient)
    }
    fn send(
        &self,
        ctx: &crate::Ctx,
        binding: &corral_core::inbox::SessionBinding,
        envelope: &corral_core::inbox::Envelope,
    ) -> corral_core::inbox::SendOutcome {
        crate::agent::delivery::CorralPiAdapter.send(ctx, binding, envelope)
    }
    fn observe(
        &self,
        _ctx: &crate::Ctx,
        _binding: &corral_core::inbox::SessionBinding,
        _cursor: &mut corral_core::inbox::ObserveCursor,
    ) -> anyhow::Result<Vec<crate::delivery::Observation>> {
        Ok(Vec::new())
    }
}
