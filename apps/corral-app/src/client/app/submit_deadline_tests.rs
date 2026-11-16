use super::*;

/// A command whose submit reply never arrived (slow Relaunch PM, for example) may have run:
/// it must become Unknown and be recovered by its id, never shown as Failed.
#[test]
fn an_expired_submit_is_recovered_by_id_instead_of_reported_failed() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    feed.update(|feed| {
        feed.commands.insert(
            Id::new("launch-pm"),
            CommandState {
                device: None,
                method: ACT_SUBMIT.into(),
                request: json!({"cmd_id":"launch-pm"}),
                status: CommandStatus::Pending,
                result: None,
                error: None,
                entry: None,
            },
        );
    });
    let mut app = AppState::new();
    let outgoing = app
        .event(
            feed,
            Event::Reply {
                method: ACT_SUBMIT,
                tag: Some("launch-pm".into()),
                params: json!({"cmd_id":"launch-pm"}),
                outcome: Err((
                    corral_protocol::rpc::ErrorCode::Unavailable,
                    "daemon request deadline expired".into(),
                )),
                error_data: Some(json!({"code": crate::client::core::DEADLINE_EXPIRED})),
            },
        )
        .unwrap();
    assert_eq!(
        feed.with_untracked(|feed| feed.commands["launch-pm"].status),
        CommandStatus::Unknown
    );
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].0, ACT_RESULT);
    assert_eq!(outgoing[0].1["cmd_id"], "launch-pm");
}
