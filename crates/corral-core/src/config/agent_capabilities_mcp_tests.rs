use super::*;

#[test]
fn mcp_and_tool_search_are_independent_for_every_role_and_preset() {
    // Given every role and preset, including PM.
    for preset in [Preset::Strict, Preset::Standard, Preset::Full] {
        for role in ROLES {
            let mut config = AgentCapabilities::default();
            config.set_preset(preset);
            config.set(role, "codemode", false).unwrap();
            let default = preset != Preset::Strict;
            assert_eq!(config.resolve(role, None).enabled("mcp.enabled"), default);
            assert_eq!(config.resolve(role, None).enabled("tool_search"), default);
            // When either boolean is independently overridden.
            config.set(role, "mcp.enabled", !default).unwrap();
            // Then the other boolean is unchanged; MCP never implies codemode.
            let snapshot = config.resolve(role, None);
            assert_eq!(snapshot.enabled("mcp.enabled"), !default);
            assert_eq!(snapshot.enabled("tool_search"), default);
            assert!(!snapshot.enabled("codemode"));
            config.set(role, "tool_search", !default).unwrap();
            config.set(role, "mcp.enabled", default).unwrap();
            let snapshot = config.resolve(role, None);
            assert_eq!(snapshot.enabled("tool_search"), !default);
            assert_eq!(snapshot.enabled("mcp.enabled"), default);
        }
    }
}

#[test]
fn mcp_transports_and_project_policy_parse_with_defaults_and_round_trip() {
    // Given both supported transport forms and explicit project enablement.
    for transport in ["command = 'docs-mcp'", "url = 'http://[::1]:8080/mcp'"] {
        let text = format!(
            "[worker.mcp.servers.docs]\nprojects = ['corral']\n\
             tools = {{ search = 'read' }}\n{transport}"
        );
        // When parsing the workspace policy.
        let config: AgentCapabilities = toml::from_str(&text).unwrap();
        let encoded = toml::to_string(&config).unwrap();
        // Then absent args/resources default empty and explicit bindings survive.
        assert_eq!(
            toml::from_str::<AgentCapabilities>(&encoded).unwrap(),
            config
        );
        let server = &config.resolve(AgentRole::Worker, None).mcp.unwrap().servers["docs"];
        assert_eq!(server.projects, ["corral"]);
        assert!(server.args.is_empty() && server.resources.is_empty());
    }
}

#[test]
fn mcp_rejects_invalid_transports_secrets_and_project_wildcards() {
    // Given invalid server settings at the configuration boundary.
    for settings in [
        "command = ''",
        "url = 'ftp://example.org'",
        "url = 'https://user:pass@example.org'",
        "url = 'https://example.org/#fragment'",
        "url = 'https://example..org'",
        "url = 'http://[:::1]'",
        "url = 'http://localhost:99999'",
        "url = 'https://example.org/%ZZ'",
        "url = 'http://999.1.1.1'",
        "url = 'http://example.org'\nargs = ['--stdio']",
        "command = 'mcp'\nurl = 'https://example.org'",
        "args = []",
        "command = 'mcp'\nenv = { TOKEN = 'secret' }",
        "command = 'mcp'\nheaders = { Authorization = 'secret' }",
        "command = 'mcp'\nunknown = true",
    ] {
        // When parsing, then malformed or ambiguous transport and secret fields are rejected.
        assert!(
            toml::from_str::<AgentCapabilities>(&format!(
                "[worker.mcp.servers.docs]\nprojects = ['corral']\ntools = {{}}\n{settings}"
            ))
            .is_err(),
            "{settings}"
        );
    }
    for projects in [
        "",
        "projects = []",
        "projects = ['*']",
        "projects = ['cor?al']",
        "projects = ['']",
    ] {
        assert!(
            toml::from_str::<AgentCapabilities>(&format!(
                "[worker.mcp.servers.docs]\ncommand = 'mcp'\ntools = {{}}\n{projects}"
            ))
            .is_err(),
            "{projects}"
        );
    }
    assert!(
        toml::from_str::<AgentCapabilities>(
            "[worker]\n'mcp.enabled' = true\n[worker.mcp]\nenabled = false"
        )
        .is_err()
    );
}

#[test]
fn mcp_write_policy_is_refused_for_all_read_only_roles_even_when_disabled() {
    // Given a server whose configured ceiling permits writes.
    for role in ROLES {
        // When parsing each role, then only workers may retain this policy.
        let text = format!(
            "[{}.mcp]\nenabled = false\n[{}.mcp.servers.docs]\n\
             projects = ['corral']\ncommand = 'mcp'\ntools = {{ edit = 'write' }}",
            role.as_str(),
            role.as_str()
        );
        assert_eq!(
            toml::from_str::<AgentCapabilities>(&text).is_ok(),
            role == AgentRole::Worker
        );
    }
}
