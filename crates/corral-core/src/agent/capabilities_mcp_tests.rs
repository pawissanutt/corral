use super::*;
use crate::config::agent_capabilities::AgentCapabilities;

#[test]
fn mcp_config_enforces_host_name_and_tool_policy() {
    // Given server names at and beyond the host boundary.
    for (name, allowed) in [
        ("docs_1-local", true),
        ("bad.name", false),
        ("", false),
        ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", false),
    ] {
        // When parsing, then the Rust policy matches the host name constraints.
        let text = format!("[worker.mcp.servers.'{name}']\nprojects = ['corral']\ncommand = 'mcp'");
        assert_eq!(toml::from_str::<AgentCapabilities>(&text).is_ok(), allowed);
    }
    // Given tools including mislabeled execution proxies and Corral namespace tokens.
    for role in ["worker", "reviewer", "pm", "planner", "plan_reviewer"] {
        for tool in [
            "search",
            "run_command",
            "docs/EDIT",
            "corral_done",
            "docs.Corral.read",
        ] {
            let text = format!(
                "[{role}.mcp.servers.docs]\nprojects = ['corral']\ncommand = 'mcp'\n\
                 tools = {{ '{tool}' = 'read' }}"
            );
            // When parsing, then Corral tokens are never allowed and proxies require worker.
            let allowed = tool == "search"
                || (role == "worker" && ["run_command", "docs/EDIT"].contains(&tool));
            assert_eq!(
                toml::from_str::<AgentCapabilities>(&text).is_ok(),
                allowed,
                "{role} {tool}"
            );
        }
    }
    // Omitted tool and resource allowlists default empty.
    let config: AgentCapabilities =
        toml::from_str("[worker.mcp.servers.docs]\nprojects = ['corral']\ncommand = 'mcp'")
            .unwrap();
    assert!(
        config.resolve(AgentRole::Worker, None).mcp.unwrap().servers["docs"]
            .tools
            .is_empty()
    );
}

#[test]
fn mcp_policy_changes_digest_transition_and_round_trips() {
    // Given explicit settings with independent project, transport, tool and resource changes.
    let policy = "[worker.mcp.servers.docs]\nprojects = ['corral']\n\
                  command = 'docs-mcp'\ntools = { search = 'read' }\nresources = ['docs://a']";
    let resolve = |text: &str| {
        toml::from_str::<AgentCapabilities>(text)
            .unwrap()
            .resolve(AgentRole::Worker, None)
    };
    let previous = resolve(policy);
    let old_json = serde_json::to_string(&previous).unwrap();
    for changed in [
        policy.replace("corral", "other"),
        policy.replace("docs-mcp", "other-mcp"),
        policy.replace("'read'", "'write'"),
        policy.replace("docs://a", "docs://b"),
        format!("{policy}\nargs = ['--local']"),
        policy.replace("command = 'docs-mcp'", "url = 'https://example.org/mcp'"),
    ] {
        // When resolving a new generation.
        let current = resolve(&changed);
        let transition = current.transition_from(2, &previous).unwrap();
        // Then deterministic identity and concrete policy readback reflect that change.
        assert_eq!(current, resolve(&changed));
        assert_ne!(current.digest, previous.digest);
        assert!(transition.changes.is_empty());
        let change = transition.mcp_change.unwrap();
        assert_eq!(change.from, previous.mcp);
        assert_eq!(change.to, current.mcp);
        assert_eq!(
            serde_json::from_str::<CapabilitySnapshot>(&serde_json::to_string(&current).unwrap())
                .unwrap(),
            current
        );
    }
    assert_eq!(serde_json::to_string(&previous).unwrap(), old_json);
    assert_eq!(
        previous.mcp.as_ref().unwrap().servers["docs"].projects,
        ["corral"]
    );
}

#[test]
fn disabled_or_unsupported_mcp_filters_settings_without_disabling_search() {
    // Given explicit server settings and either a disabled feature or an older host.
    let config: AgentCapabilities = toml::from_str(
        "[worker.mcp]\nenabled = false\n[worker.mcp.servers.docs]\n\
         projects = ['corral']\ncommand = 'docs-mcp'\ntools = {}",
    )
    .unwrap();
    let supported = ["tool_search".to_owned()].into_iter().collect();
    // When resolving a generation.
    for snapshot in [
        config.resolve(AgentRole::Worker, None),
        AgentCapabilities::default().resolve(AgentRole::Worker, Some(&supported)),
    ] {
        // Then MCP settings cannot reach launch, but local search remains independent.
        assert!(!snapshot.enabled("mcp.enabled"));
        assert!(snapshot.mcp.is_none());
        assert!(snapshot.enabled("tool_search"));
    }
}

#[test]
fn pre_mcp_snapshots_and_transitions_remain_unchanged_when_parsed() {
    // Given recorded pre-MCP wire records with their original digests.
    let snapshot = r#"{"catalog":2,"role":"worker","preset":"standard","digest":"recorded"}"#;
    let transition = r#"{"from_generation":1,"from_digest":"old","from_preset":"strict"}"#;
    // When reading existing records.
    let old: CapabilitySnapshot = serde_json::from_str(snapshot).unwrap();
    let old_transition: CapabilityTransition = serde_json::from_str(transition).unwrap();
    // Then new fields remain absent and the recorded identity is never recomputed.
    assert!(old.mcp.is_none());
    assert_eq!(old.catalog, 2);
    assert_eq!(old.digest, "recorded");
    assert_eq!(
        serde_json::to_value(&old).unwrap(),
        serde_json::from_str::<serde_json::Value>(snapshot).unwrap()
    );
    assert!(old_transition.mcp_change.is_none());
}
