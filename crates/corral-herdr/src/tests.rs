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
fn agent_prompt_falls_back_to_send_text_when_agent_not_ready() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("corral-herdr-fake-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("herdr");
    std::fs::write(
        &bin,
        r#"#!/usr/bin/env bash
case "$1 $2" in
  "agent prompt") echo '{"id":"x","error":{"code":"agent_not_ready","message":"pane w1:p1 is not an active named agent"}}'; exit 1 ;;
  "pane send-text"|"pane send-keys") echo "$*" >> "$(dirname "$0")/calls.log"; echo '{"id":"x","result":{"type":"ok"}}' ;;
  *) echo '{"id":"x","error":{"code":"unexpected","message":"unexpected"}}'; exit 1 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let result = Herdr::with_bin(bin, None).agent_prompt("w1:p1", "please fix");
    let calls = std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    result.unwrap();
    assert_eq!(
        calls,
        "pane send-text w1:p1 please fix\npane send-keys w1:p1 enter\n"
    );
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
