//! `submit_operating_action_head` against `submit_operating_action` on identical stores and
//! inputs, on both backends (T-331 action-heads): the same dispositions, committed task rows,
//! events, records, authorization rows and change entries; the send callback runs once on
//! success and never on refusal.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::sync::Arc;

use corral_core::backend::fake::FakeBackend;
use corral_core::backend::{
    BackendConfig, FaultInjector, ManualClock, PageReq, RecordQuery, RecordWrite, TaskNum,
    TaskParts,
};
use corral_core::{
    ActionAuthorization, OperatingActionIdentity, OperatingActionKind, OperatingAuthority,
    OperatingOrigin, OperatingSelection, Status, Store, SubmissionOutcome,
};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Case {
    Success,
    SendError,
    CallbackCheck,
    ChangedIdentity,
    RevokedGrant,
}

#[derive(Clone, Copy, Debug)]
enum Form {
    Full,
    Head,
}

fn at() -> jiff::Timestamp {
    "2026-10-18T03:00:00Z".parse().unwrap()
}

fn setup(fake: bool, automatic: bool) -> (tempfile::TempDir, Store, ActionAuthorization) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("app")).unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    if fake {
        store = store.with_backend(Box::new(FakeBackend::new(
            BackendConfig::default(),
            Arc::new(ManualClock::new(at())),
            Arc::new(FaultInjector::default()),
        )));
    }
    store
        .edit_config(|doc| doc.set_str("projects.app.path", "app"))
        .unwrap();
    let task = store
        .create("Submission", Some("app"), Status::Ready, "Scope\n")
        .unwrap();
    let origin = if automatic {
        let preview = store
            .preview_operating_policy(&OperatingSelection {
                tasks: BTreeSet::from([task.id.clone()]),
                authority: OperatingAuthority {
                    dispatch: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        store
            .confirm_operating_policy(&preview, "owner", at())
            .unwrap();
        let approval = store.operating_state().unwrap().approval.unwrap();
        OperatingOrigin::Automatic {
            authorization: approval.authorization,
            revision: approval.revision,
        }
    } else {
        OperatingOrigin::ExplicitOwner {
            approval: "launch-1".into(),
            confirmed_by: "owner".into(),
            confirmed_at: at(),
        }
    };
    let authorization = store
        .authorize_operating_action(
            OperatingActionIdentity {
                task: task.id,
                run: "R1".into(),
                attempt: "launch-1".into(),
                kind: OperatingActionKind::Dispatch,
                candidate: None,
                plan: None,
                round: None,
            },
            origin,
        )
        .unwrap();
    (dir, store, authorization)
}

fn prepare_effects(store: &Store, id: &str, refuse: bool) -> anyhow::Result<()> {
    store.log_event(Some(id), "submission-prepared", json!({"marker": true}))?;
    store.put_records(vec![RecordWrite {
        area: "submission".into(),
        key: "reservation".into(),
        expected: None,
        doc: Some(json!({"marker": true})),
        updated: at(),
    }])?;
    anyhow::ensure!(!refuse, "the candidate moved");
    Ok(())
}

/// Replaces timestamps and the per-store identities (workspace root, repository identity,
/// policy revision, authorization and grant ids), which differ between stores.
fn normalize(value: Value, masks: &[(String, &str)]) -> Value {
    match value {
        Value::String(s) => {
            let s = masks
                .iter()
                .fold(s, |s, (from, to)| s.replace(from.as_str(), to));
            if s.parse::<jiff::Timestamp>().is_ok() {
                Value::String("<ts>".into())
            } else {
                Value::String(s)
            }
        }
        Value::Array(items) => {
            Value::Array(items.into_iter().map(|v| normalize(v, masks)).collect())
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, normalize(v, masks)))
                .collect(),
        ),
        other => other,
    }
}

fn run(fake: bool, case: Case, form: Form) -> Value {
    let (dir, store, authorization) = setup(fake, case == Case::RevokedGrant);
    let id = authorization.action.task.clone();
    match case {
        Case::ChangedIdentity => {
            store
                .update(&id, |task| {
                    task.body = "Changed scope\n".into();
                    Ok(())
                })
                .unwrap();
        }
        Case::RevokedGrant => {
            store.assist_operating_policy().unwrap();
        }
        _ => {}
    }
    let seq = store.view().unwrap().seq();
    let sends = Cell::new(0);
    let refuse = case == Case::CallbackCheck;
    let send = || {
        sends.set(sends.get() + 1);
        if case == Case::SendError {
            anyhow::bail!("transport lost");
        }
        Ok(17)
    };
    let mut dispositions = Vec::new();
    for attempt in 0..2 {
        let outcome = match form {
            Form::Full => store.submit_operating_action(
                &authorization,
                &authorization.action,
                |fresh, task| {
                    task.queued = true;
                    prepare_effects(fresh, &task.id, refuse)
                },
                send,
            ),
            Form::Head => store.submit_operating_action_head(
                &authorization,
                &authorization.action,
                |fresh, head| {
                    head.queued = true;
                    prepare_effects(fresh, &head.id, refuse)
                },
                send,
            ),
        };
        dispositions.push(match outcome {
            Ok(SubmissionOutcome::Submitted(n)) => json!({"submitted": n}),
            Ok(SubmissionOutcome::AuthorizationRefused(reason)) => json!({"refused": reason}),
            Err(error) => json!({"error": format!("{error:#}"), "attempt": attempt}),
        });
        dispositions.push(
            serde_json::to_value(
                store
                    .authorized_submission(&authorization.id)
                    .unwrap()
                    .disposition,
            )
            .unwrap(),
        );
    }
    let view = store.view().unwrap();
    let task = view.task(TaskNum(1), &TaskParts::All).unwrap().unwrap();
    let events: Vec<Value> = store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.task.as_deref() == Some(id.as_str()))
        .map(|e| json!({"kind": e.kind, "detail": e.detail}))
        .collect();
    let records: Vec<Value> = view
        .records(
            &RecordQuery {
                area: Some("submission".into()),
                key: None,
            },
            PageReq::first(10),
        )
        .unwrap()
        .items
        .into_iter()
        .map(|r| r.doc)
        .collect();
    let changes: Vec<Value> = store
        .changes_since(seq, 100)
        .unwrap()
        .entries
        .into_iter()
        .map(|c| json!({"op": format!("{:?}", c.op), "scopes": format!("{:?}", c.scopes)}))
        .collect();
    let row = store.authorized_submission(&authorization.id).unwrap();
    let observed = json!({
        "dispositions": dispositions,
        "sends": sends.get(),
        "head": {"doc": task.head.doc, "summary": task.head.summary},
        "rows": task.rows,
        "events": events,
        "records": records,
        "changes": changes,
        "authorization": row,
    });
    let mut masks = vec![
        (dir.path().to_string_lossy().into_owned(), "<root>"),
        (authorization.id.clone(), "<auth>"),
        (
            authorization.member.repository_identity.clone(),
            "<repository>",
        ),
        (authorization.policy_revision.clone(), "<policy>"),
    ];
    if let OperatingOrigin::Automatic {
        authorization: grant,
        ..
    } = &authorization.origin
    {
        masks.push((grant.clone(), "<grant>"));
    }
    normalize(observed, &masks)
}

fn differences(a: &Value, b: &Value, path: String, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for key in x.keys().chain(y.keys()).collect::<BTreeSet<_>>() {
                let (l, r) = (
                    x.get(key).unwrap_or(&Value::Null),
                    y.get(key).unwrap_or(&Value::Null),
                );
                differences(l, r, format!("{path}/{key}"), out);
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (l, r)) in x.iter().zip(y).enumerate() {
                differences(l, r, format!("{path}/{i}"), out);
            }
        }
        _ if a != b => out.push(format!("{path}: {a} != {b}")),
        _ => {}
    }
}

#[test]
fn head_submission_matches_full_submission_on_both_backends() {
    for fake in [false, true] {
        for case in [
            Case::Success,
            Case::SendError,
            Case::CallbackCheck,
            Case::ChangedIdentity,
            Case::RevokedGrant,
        ] {
            let full = run(fake, case, Form::Full);
            let head = run(fake, case, Form::Head);
            let mut diff = Vec::new();
            differences(&full, &head, String::new(), &mut diff);
            assert!(diff.is_empty(), "fake={fake} {case:?}: {diff:#?}");
            let sends = full["sends"].as_u64().unwrap();
            let first = &full["dispositions"][0];
            match case {
                Case::Success => {
                    assert_eq!(first, &json!({"submitted": 17}));
                    assert_eq!(sends, 1);
                }
                Case::SendError => assert_eq!(sends, 1),
                Case::CallbackCheck | Case::ChangedIdentity | Case::RevokedGrant => {
                    assert!(first.get("refused").is_some(), "{case:?}: {first}");
                    assert_eq!(sends, 0, "{case:?} invoked send");
                }
            }
            if matches!(case, Case::Success | Case::SendError) {
                let replay = full["dispositions"][2]["error"].as_str().unwrap();
                assert!(replay.contains("refusing replay"), "{replay}");
            }
        }
    }
}
