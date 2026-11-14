//! Global discoverability must not depend on a current assistant or any loaded thread.
use super::*;
use crate::agent::app_runtime::{ensure_filing_log, filings, send};
use crate::agent::test_support as support;
use corral_protocol::assistant::{AskTarget, AssistantSend};

#[test]
fn ended_quick_filings_backfill_without_threads_and_keep_current_task_decisions() {
    let fx = support::fx();
    let device = AuthenticatedDevice {
        id: "dev-global".into(),
        name: "Browser".into(),
        assistant: true,
        assistant_act: false,
        reply: true,
        act: true,
    };
    RecordArea::<serde_json::Value>::new("devices")
        .save(
            &fx.ctx.store,
            &device.id,
            &json!({
                "name":device.name,"revoked_at":null,"authority":{
                    "assistant":true,"assistant_act":false,"act":true,"reply":true
                }
            }),
        )
        .unwrap();
    let old = fx.ctx.store.app_thread("quick").unwrap();
    let words = "  Original AI-filed owner words.  \n";
    let input = input::journal(
        &fx.ctx.store,
        &old.session,
        &device,
        "ai-words",
        words,
        &Timestamp::now().to_string(),
        false,
    )
    .unwrap();
    let host = support::bind(
        &fx,
        &crate::agent::BindTarget::Assistant {
            session: old.session.clone(),
        },
        "old-pane",
        "old-conversation",
    );
    support::committed(&support::hello(&fx, &host));
    let payload = json!({"input":input.id,"title":"Structured AI idea","body":"Explanation",
        "labels":["chat"],"criteria":["Keep the exact owner words."]});
    let request = crate::agent::tool_request_id(&host.identity.session_id, "file-global");
    let bytes = support::envelope(&host, "idea_file", &request, payload, false);
    let response = crate::agent::call(&fx.ctx, "idea_file", &bytes).unwrap();
    let receipt = support::committed(&response);
    let ai_task = receipt.result["task"].as_str().unwrap().to_owned();
    assert!(fx.ctx.store.get(&ai_task).unwrap().body.contains(words));
    assert!(matches!(
        crate::agent::call(&fx.ctx, "idea_file", &bytes).unwrap(),
        CallResponse::Receipt { replayed: true, .. }
    ));
    fx.ctx.store.end_assistant(&old.session).unwrap();

    let new = fx.ctx.store.app_thread("quick").unwrap();
    assert_ne!(new.session, old.session);
    let owner = send(
        &fx.ctx,
        &device,
        &AssistantSend {
            cmd_id: "owner-words".into(),
            thread: "quick".into(),
            target: AskTarget::Assistant,
            text: "Owner file-as-written words".into(),
            typed_at: Timestamp::now().to_string(),
            context: Vec::new(),
            attachments: Vec::new(),
        },
    )
    .unwrap();
    let owner_task = file_as_idea(&fx.ctx, &device, &owner.entry).unwrap();
    assert_eq!(
        file_as_idea(&fx.ctx, &device, &owner.entry).unwrap(),
        owner_task
    );
    fx.ctx.store.end_assistant(&new.session).unwrap();
    assert_eq!(filings(&fx.ctx, None).unwrap().total, 2);

    // Simulate the pre-upgrade store: only the two derived areas are absent.
    let canonical = fx.ctx.store.record_rows("app_idea_filings").unwrap();
    for area in ["app_idea_filing_log", "app_idea_filing_log_head"] {
        for row in fx.ctx.store.record_rows(area).unwrap() {
            fx.ctx.store.delete_record(area, &row.key).unwrap();
        }
    }
    assert!(
        filings(&fx.ctx, None)
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );
    ensure_filing_log(&fx.ctx).unwrap();
    ensure_filing_log(&fx.ctx).unwrap();
    let page = filings(&fx.ctx, None).unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(
        page.rows
            .iter()
            .map(|row| row.task.as_str())
            .collect::<Vec<_>>(),
        vec![owner_task.as_str(), ai_task.as_str()]
    );
    assert!(page.rows.iter().all(|row| row.quick));
    assert_eq!(page.rows[1].session, old.session);
    assert_eq!(page.rows[1].device.as_deref(), Some("Browser"));
    assert_eq!(
        fx.ctx.store.record_rows("app_idea_filings").unwrap(),
        canonical
    );

    fx.ctx
        .store
        .update(&ai_task, |task| {
            task.title = "Retitled after filing".into();
            Ok(())
        })
        .unwrap();
    crate::ideas::decide_idea(
        &fx.ctx,
        &ai_task,
        "merge",
        "Use the owner's newer idea",
        crate::ideas::IdeaDecisionOptions {
            into: Some(&owner_task),
            body: None,
            owner_ok: true,
            actor: "owner",
        },
    )
    .unwrap();
    let updated = filings(&fx.ctx, None).unwrap();
    assert_eq!(updated.total, 2);
    assert_eq!(updated.rows[1].title, "Retitled after filing");
    assert_eq!(updated.rows[1].status, "dropped");
    let decision = updated.rows[1].decision.as_ref().unwrap();
    assert_eq!(decision.action, "merge");
    assert_eq!(decision.into.as_deref(), Some(owner_task.as_str()));
}
