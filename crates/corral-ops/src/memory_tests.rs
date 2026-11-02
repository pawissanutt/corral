use super::*;
use corral_core::memory::MemoryRole;

#[test]
fn memory_scope_matrix_and_project_text_trust() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.projects.insert(
        "corral".into(),
        serde_json::from_value(serde_json::json!({"path":tmp.path()})).unwrap(),
    );
    store.config.projects.insert(
        "other".into(),
        serde_json::from_value(serde_json::json!({"path":tmp.path()})).unwrap(),
    );
    let scopes = [
        MemoryScope::Workspace,
        MemoryScope::Project("corral".into()),
        MemoryScope::Project("other".into()),
        MemoryScope::Role(MemoryRole::Pm),
        MemoryScope::Role(MemoryRole::Assistant),
        MemoryScope::Role(MemoryRole::Worker),
    ];
    for (actor, reads, writes) in [
        (
            MemoryActor::Operator,
            [true, true, true, true, true, true],
            [true, true, true, true, true, true],
        ),
        (
            MemoryActor::Pm("corral"),
            [true, true, false, true, false, false],
            [true, true, false, true, false, false],
        ),
        (
            MemoryActor::Assistant,
            [true, true, true, false, true, false],
            [true, true, true, false, true, false],
        ),
        (
            MemoryActor::Worker("corral"),
            [true, true, false, false, false, true],
            [false, true, false, false, false, true],
        ),
    ] {
        for (i, scope) in scopes.iter().enumerate() {
            assert_eq!(authorize(&store, &actor, scope, false).is_ok(), reads[i]);
            assert_eq!(authorize(&store, &actor, scope, true).is_ok(), writes[i]);
        }
    }
    assert!(
        authorize(
            &store,
            &MemoryActor::Assistant,
            &MemoryScope::Project("missing".into()),
            true
        )
        .is_err()
    );
    store.config.agent.capabilities.set_trusted_projects(vec![]);
    assert!(authorize(&store, &MemoryActor::Worker("corral"), &scopes[1], false).is_err());
    assert!(authorize(&store, &MemoryActor::Assistant, &scopes[1], false).is_err());
}
