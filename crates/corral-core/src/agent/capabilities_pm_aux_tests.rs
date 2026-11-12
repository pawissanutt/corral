use super::super::*;

#[test]
fn pm_catalog_matches_assistant_except_pm_specific_capabilities() {
    for def in CATALOG {
        assert_eq!(
            (def.defaults)(AgentRole::Pm),
            match def.key {
                "write_permissions" => None,
                "shadow" | "expert" => Some([false; 3]),
                _ => (def.defaults)(AgentRole::Assistant),
            },
            "{}",
            def.key
        );
    }
}
