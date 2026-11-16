use super::*;
use crate::agent::app_runtime::{AuthenticatedDevice, file_as_idea, send};
use crate::agent::test_support as support;
use corral_core::records::RecordArea;
use corral_protocol::assistant::{AskTarget, AssistantSend, ContextRef};
use jiff::Timestamp;

#[test]
fn project_context_defaults_filing_and_allows_explicit_override() -> Result<()> {
    let mut fx = support::fx();
    for project in ["alpha", "beta"] {
        fx.ctx.store.config.projects.insert(
            project.into(),
            serde_json::from_value(json!({"path": "."}))?,
        );
    }
    fx.ctx.store.save_config()?;
    let device = AuthenticatedDevice {
        id: "project-owner".into(),
        name: "Owner".into(),
        assistant: true,
        assistant_act: false,
        reply: true,
        act: true,
    };
    RecordArea::<Value>::new("devices").save(
        &fx.ctx.store,
        &device.id,
        &json!({"name": device.name, "revoked_at": null, "authority": {
            "assistant": true, "assistant_act": false, "act": true, "reply": true
        }}),
    )?;
    let request = AssistantSend {
        cmd_id: "project-default".into(),
        thread: "quick".into(),
        target: AskTarget::Assistant,
        text: "File this idea".into(),
        typed_at: Timestamp::now().to_string(),
        context: ["unknown-project", "alpha"]
            .into_iter()
            .map(|id| ContextRef {
                kind: "project".into(),
                id: id.into(),
                rev: None,
            })
            .collect(),
        attachments: Vec::new(),
    };
    let sent = send(&fx.ctx, &device, &request)?;
    let message = fx.ctx.store.app_message(&sent.entry)?;
    assert_eq!(message.context.len(), 1);
    let prompt = context(&fx.ctx, &message)?;
    assert!(prompt.contains("\"alpha\""));
    assert!(!prompt.contains("unknown-project"));
    let task = file_as_idea(&fx.ctx, &device, &sent.entry)?;
    assert_eq!(fx.ctx.store.head(&task)?.project.as_deref(), Some("alpha"));

    let mut request = request;
    request.cmd_id = "project-override".into();
    request.text = "File this other idea in beta".into();
    let sent = send(&fx.ctx, &device, &request)?;
    let message = fx.ctx.store.app_message(&sent.entry)?;
    let host = support::bind(
        &fx,
        &crate::agent::BindTarget::Assistant {
            session: message.session.clone(),
        },
        "project-pane",
        "project-conversation",
    );
    support::committed(&support::hello(&fx, &host));
    let key = crate::agent::tool_request_id(&host.identity.session_id, "project-override");
    let payload = json!({
        "input": message.input, "title": "Other idea", "body": "", "project": "beta"
    });
    let bytes = support::envelope(&host, "idea_file", &key, payload, false);
    let response = crate::agent::call(&fx.ctx, "idea_file", &bytes)?;
    let receipt = support::committed(&response);
    let task = receipt.result["task"].as_str().unwrap();
    assert_eq!(fx.ctx.store.head(task)?.project.as_deref(), Some("beta"));

    let mut unknown = message;
    unknown.context[0].id = "unknown-project".into();
    assert!(context(&fx.ctx, &unknown)?.is_empty());
    Ok(())
}
