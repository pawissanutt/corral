//! Workspace PM scope regressions; all engines and credentials are disposable fixtures.
use super::{WORKSPACE, tests::fixture};
use crate::agent::{self, test_support as ts};
use corral_core::{OperatingSelection, Status};
use serde_json::json;

fn call(
    fx: &crate::agent::bridge::fixture::Fx,
    host: &ts::Host,
    op: &str,
    id: &str,
    payload: serde_json::Value,
) -> agent::CallResponse {
    agent::call(
        &fx.ctx,
        op,
        &ts::envelope(
            host,
            op,
            &agent::tool_request_id(&host.identity.session_id, id),
            payload,
            false,
        ),
    )
    .unwrap()
}

#[test]
fn approved_three_projects_preflight_and_authenticated_operations() {
    let mut fx = fixture();
    let mut tasks = vec![fx.id.clone()];
    for project in ["second", "third"] {
        fx.ctx.store.config.projects.insert(
            project.into(),
            serde_json::from_value(json!({"path":"app"})).unwrap(),
        );
        fx.ctx.store.save_config().unwrap();
        tasks.push(
            fx.ctx
                .store
                .create(project, Some(project), Status::Ready, "fixture")
                .unwrap()
                .id,
        );
    }
    let preview = fx
        .ctx
        .store
        .preview_operating_policy(&OperatingSelection {
            tasks: tasks.iter().cloned().collect(),
            ..Default::default()
        })
        .unwrap();
    fx.ctx
        .store
        .confirm_operating_policy(&preview, "owner", jiff::Timestamp::now())
        .unwrap();
    // The same entry point used by `corral pi --pm`, including preflight.
    super::profile(&fx.ctx).unwrap();
    assert_eq!(super::project(&fx.ctx).unwrap(), WORKSPACE);
    let process = agent::bridge::fixture::HostProc::spawn(true);
    let host = fx.bind(
        &agent::BindTarget::Pm {
            project: WORKSPACE.into(),
        },
        "pm-pane",
        &process,
    );
    let list = ts::committed(&call(&fx, &host, "read_list", "list", json!({})));
    assert_eq!(list.result["total"], 3);
    for task in &tasks {
        let receipt = ts::committed(&call(
            &fx,
            &host,
            "comment",
            task,
            json!({"task":task,"text":"workspace triage"}),
        ));
        assert_eq!(receipt.state, corral_core::agent::ReceiptState::Committed);
        assert_eq!(fx.ctx.store.get(task).unwrap().comments.len(), 1);
    }
    // A configured but unapproved fourth project is still excluded.
    fx.ctx.store.config.projects.insert(
        "outside".into(),
        serde_json::from_value(json!({"path":"app"})).unwrap(),
    );
    fx.ctx.store.save_config().unwrap();
    let outside = fx
        .ctx
        .store
        .create("outside", Some("outside"), Status::Ready, "")
        .unwrap();
    assert_eq!(
        ts::code(&call(
            &fx,
            &host,
            "comment",
            "outside",
            json!({"task":outside.id,"text":"no"})
        )),
        Some("out_of_scope")
    );
}

#[test]
fn empty_scope_refuses_with_the_owner_fix_command() {
    let fx = ts::fx();
    let error = super::project(&fx.ctx).unwrap_err().to_string();
    assert!(error.contains("pm_scope_"), "{error}");
    assert!(
        error.contains("corral mode autopilot --project <project>"),
        "{error}"
    );
    assert!(error.contains("--confirm <digest>"), "{error}");
}
