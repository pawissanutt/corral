use super::*;

const CREATE: &str = r#"{"id":"cli:workspace:create","result":{"root_pane":{"agent_status":"unknown","cwd":"/tmp","pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2"},"tab":{"tab_id":"w2:t1"},"type":"workspace_created","workspace":{"label":"probe2","workspace_id":"w2"}}}"#;
const AGENTS: &str = r#"{"id":"x","result":{"agents":[{"agent":"fake","agent_status":"blocked","cwd":"/tmp","pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","title":"T"}],"type":"agent_list"}}"#;
const PANES: &str = r#"{"id":"x","result":{"panes":[{"agent_status":"working","cwd":"/tmp","pane_id":"w1:p1","tab_id":"w1:t1","terminal_title_stripped":"/tmp","workspace_id":"w1"},{"pane_id":"w2:p1"}],"type":"pane_list"}}"#;
const WS: &str = r#"{"id":"x","result":{"type":"workspace_list","workspaces":[{"label":"probe","workspace_id":"w1"}]}}"#;

fn result(text: &str) -> Value {
    let v: Value = serde_json::from_str(text).unwrap();
    v["result"].clone()
}

#[test]
fn parses_created() {
    let c = parse_created(&result(CREATE)).unwrap();
    assert_eq!(
        c,
        Created {
            workspace_id: "w2".into(),
            tab_id: "w2:t1".into(),
            pane_id: "w2:p1".into()
        }
    );
}

#[test]
fn parses_agents_and_panes() {
    let a = parse_panes(&result(AGENTS), "agents");
    assert_eq!(a[0].status, AgentStatus::Blocked);
    assert_eq!(a[0].agent.as_deref(), Some("fake"));
    assert_eq!(a[0].title.as_deref(), Some("T"));
    let p = parse_panes(&result(PANES), "panes");
    assert_eq!(p.len(), 2);
    assert_eq!(p[0].title.as_deref(), Some("/tmp"));
    assert_eq!(p[0].status, AgentStatus::Working);
    assert_eq!(p[1].status, AgentStatus::Unknown);
    assert_eq!(p[1].cwd, None);
}

#[test]
fn parses_workspaces() {
    assert_eq!(
        parse_workspaces(&result(WS)),
        vec![("w1".to_string(), "probe".to_string())]
    );
}

#[test]
fn process_info_keeps_pids_when_argv_is_missing() {
    // Given the live Herdr shape when one process has no argument metadata.
    let payload = serde_json::json!({
        "shell_pid": 10,
        "foreground_processes": [
            {"pid": 10, "argv": ["fish"]},
            {"pid": 11, "name": "claude"}
        ]
    });
    // When decoding the process-info boundary.
    let info: ProcessInfo = serde_json::from_value(payload).unwrap();
    // Then launch observation retains both process identities.
    assert_eq!(info.shell_pid, 10);
    assert_eq!(
        info.foreground_processes,
        vec![
            ForegroundProcess {
                pid: 10,
                argv: vec!["fish".into()]
            },
            ForegroundProcess {
                pid: 11,
                argv: Vec::new()
            }
        ]
    );
}

#[test]
fn error_objects_and_codes() {
    let (code, msg) =
        find_error(r#"{"error":{"code":"agent_blocked","message":"m"},"id":"x"}"#).unwrap();
    assert_eq!((code.as_str(), msg.as_str()), ("agent_blocked", "m"));
    assert!(find_error(WS).is_none());
    assert!(is_no_agent_code("agent_blocked"));
    assert!(is_no_agent_code("agent_not_ready"));
    assert!(!is_no_agent_code("pane_not_found"));
    let e: anyhow::Error = HerdrError {
        args: "pane get x".into(),
        code: "c".into(),
        message: "m".into(),
    }
    .into();
    assert_eq!(e.to_string(), "herdr pane get x: c: m");
}

#[test]
fn read_text_plain_and_wrapped() {
    assert_eq!(extract_read_text("echo hi\n"), "echo hi\n");
    assert_eq!(extract_read_text(r#"{"result":{"text":"abc"}}"#), "abc");
}

#[test]
fn status_roundtrip() {
    assert_eq!(AgentStatus::parse("blocked"), AgentStatus::Blocked);
    assert_eq!(AgentStatus::parse("zzz"), AgentStatus::Unknown);
}
