use super::Repo;
use crate::{
    FactsCache, Revision, TreeFacts,
    lenses::{self, LensCoverage},
    provider::Reply,
};
use corral_types::lenses::LensStatus;
use serde_json::{Value, json};

const DIR: &str = ".corral/lenses";
const DATE: &str = "2026-11-23T12:00:00Z";
fn tree(repo: &Repo, rev: &Revision) -> TreeFacts {
    FactsCache::new(repo.path())
        .tree_facts(repo.path(), rev.tree())
        .unwrap()
}
fn load(repo: &Repo, rev: &Revision) -> lenses::LensSources {
    lenses::load(repo.path(), rev, DIR).unwrap()
}
fn recipe(id: &str, bindings: &[(&str, Value)]) -> Value {
    json!({"schema":"corral.repo-diagram","version":1,"id":id,"title":id,"layout":"graph",
        "nodes":bindings.iter().map(|(id,_)| json!({"id":id,"label":id})).collect::<Vec<_>>(),
        "evidence":bindings.iter().filter(|(_,v)|!v.is_null()).map(|(id,anchor)|json!({"element":id,"anchor":anchor})).collect::<Vec<_>>()})
}
fn put(repo: &Repo, file: &str, v: &Value) {
    repo.put(&format!("{DIR}/{file}.json"), &v.to_string());
}

#[test]
fn checks_pinned_git_source_uniqueness_selected_enum_and_real_inbound_references() {
    let repo = Repo::new();
    repo.put("Cargo.toml", "[package]\nname='sample'\nversion='0.1.0'\n");
    repo.put("src/lib.rs", "mod gate;\nuse crate::gate::Status;\n");
    let code = "pub enum Status { Ready, NotYet }\npub fn gate() { do_it(); }\n";
    repo.put("src/gate.rs", code);
    let mut v = recipe(
        "test",
        &[
            (
                "gate",
                json!({"kind":"symbol","path":"src/gate.rs","name":"gate"}),
            ),
            (
                "ready",
                json!({"kind":"symbol","path":"src/gate.rs","name":"Status::Ready"}),
            ),
            ("unused", json!(null)),
            (
                "call",
                json!({"kind":"text","path":"src/gate.rs","text":"do_it();"}),
            ),
            ("not_yet", json!(null)),
        ],
    );
    v["coverage"] = json!({"path":"src/gate.rs","name":"Status","nodes":["ready","not_yet"]});
    put(&repo, "workflow", &v);
    let old = repo.commit(DATE, "old");
    repo.put("src/gate.rs", &format!("// shifted\n{code}"));
    let shifted = repo.commit(DATE, "move code");
    repo.put("src/gate.rs","enum Status { Ready, Done }\nmod a { fn gate() { do_it(); } }\nmod b { fn gate() { do_it(); } }\n");
    let head = repo.commit(DATE, "ambiguous anchors");
    repo.put("src/gate.rs", "dirty");
    let check = |rev| {
        lenses::check(&load(&repo, rev), &tree(&repo, rev), None)
            .pop()
            .unwrap()
    };
    let r = check(&old);
    assert_eq!(r.status, LensStatus::Current, "{r:?}");
    assert!(
        r.locations
            .iter()
            .any(|l| l.element == "gate" && l.line == 2)
    );
    assert!(r.derived.iter().any(|r| r.element == "gate"
        && r.reference.from == "src/lib.rs"
        && r.reference.to == "src/gate.rs"));
    assert!(r.not_checked.iter().any(|d| d.element == "unused"));
    let r = check(&shifted);
    assert!(
        r.locations
            .iter()
            .any(|l| l.element == "call" && l.line == 3)
    );
    let r = check(&head);
    assert_eq!(r.status, LensStatus::Stale);
    for id in ["gate", "call"] {
        assert!(
            r.failures
                .iter()
                .any(|f| f.element == id && f.message.contains("ambiguous"))
        );
        assert!(!r.locations.iter().any(|l| l.element == id));
        assert!(!r.derived.iter().any(|l| l.element == id));
    }
    assert_eq!(r.missing_states, ["Done"]);
    assert_eq!(r.extra_states, ["not_yet"]);
    let impact = |a, b| {
        lenses::impact(
            repo.path(),
            a,
            b,
            &load(&repo, a),
            &load(&repo, b),
            &tree(&repo, a),
            &tree(&repo, b),
        )
        .unwrap()
    };
    assert!(impact(&old, &shifted).is_empty());
    let rows = impact(&shifted, &head);
    assert!(rows[0].touched.contains(&"gate".into()));
    assert!(rows[0].touched_base.contains(&"gate".into()));
}

#[test]
fn unsupported_coverage_partial_entry_facts_and_malformed_siblings() {
    let repo = Repo::new();
    repo.put("routes.txt", "/ok\n");
    let mut v = recipe(
        "routes",
        &[(
            "route",
            json!({"kind":"entry","entry_kind":"route","name":"GET /wanted"}),
        )],
    );
    v["coverage"] = json!({"path":"routes.txt","name":"Route","nodes":["route"]});
    put(&repo, "routes", &v);
    repo.put(&format!("{DIR}/old.toml"), "legacy");
    repo.put(&format!("{DIR}/broken.json"), "bad json");
    repo.put(&format!("{DIR}/sub/nested.json"), "not scanned");
    let duplicate = recipe("duplicate", &[("x", Value::Null)]);
    put(&repo, "a", &duplicate);
    put(&repo, "b", &duplicate);
    let rev = repo.commit(DATE, "recipes");
    let sources = load(&repo, &rev);
    let check = |name: &str, omitted: u64| {
        let mut facts = tree(&repo, &rev);
        let input = format!(
            r#"{{"type":"entry","path":"routes.txt","line":1,"kind":"route","name":{name:?}}}
{{"type":"done","provided":["entries"],"skipped":{{}},"omitted":{{"limit":{omitted}}}}}"#
        );
        let reply = Reply::parse(input.as_bytes(), &["entries"]).unwrap();
        let unread_files = reply.unread_files();
        let coverage = reply.merge_snapshot(&mut facts);
        let mut rows = lenses::check(
            &sources,
            &facts,
            Some(LensCoverage {
                provider: &coverage,
                unread_files: &unread_files,
            }),
        );
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows.iter()
                .filter(|r| r.status == LensStatus::Malformed)
                .count(),
            3
        );
        rows.pop().unwrap()
    };
    let r = check("GET /other", 1);
    assert!(r.failures.is_empty());
    assert!(r.not_checked.iter().any(|i| i.element == "route"));
    assert!(r.not_checked.iter().any(|i| i.element == "$coverage"));
    assert!(
        check("GET /wanted", 0)
            .locations
            .iter()
            .any(|i| i.element == "route")
    );
    assert!(check("GET /wanted", 1).locations.is_empty());
    assert_eq!(check("GET /other", 0).status, LensStatus::Stale);
}

#[test]
fn impact_keeps_stable_ids_across_label_edits_multiple_bindings_and_removed_recipes() {
    let repo = Repo::new();
    repo.put("first", "changed line\n");
    repo.put("second", "stable\n");
    let path = |p| json!({"kind":"path","path":p});
    let v = recipe("kept", &[("key", path("second")), ("del", path("first"))]);
    put(&repo, "kept", &v);
    put(
        &repo,
        "removed",
        &recipe("removed", &[("old", path("first"))]),
    );
    put(
        &repo,
        "replaced",
        &recipe("other", &[("same", path("second"))]),
    );
    let base = repo.commit(DATE, "base");
    let mut v = recipe("kept", &[("new", path("second")), ("key", path("second"))]);
    v["nodes"][1]["label"] = json!("new label");
    v["evidence"]
        .as_array_mut()
        .unwrap()
        .push(json!({"element":"key","anchor":path("first")}));
    put(&repo, "kept", &v);
    repo.put("first", "edited line\n");
    std::fs::remove_file(repo.path().join(format!("{DIR}/removed.json"))).unwrap();
    put(
        &repo,
        "replaced",
        &recipe("different", &[("same", path("second"))]),
    );
    let head = repo.commit(DATE, "head");
    let rows = lenses::impact(
        repo.path(),
        &base,
        &head,
        &load(&repo, &base),
        &load(&repo, &head),
        &tree(&repo, &base),
        &tree(&repo, &head),
    )
    .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].added, ["new"]);
    assert_eq!(rows[0].removed, ["del"]);
    assert_eq!(rows[0].touched, ["key"]);
    assert_eq!(rows[0].touched_base, ["del"]);
    assert_eq!(rows[1].removed, ["old"]);
    assert_eq!(rows[1].touched_base, ["old"]);
    assert_eq!(rows[2].removed, ["same"]);
    assert_eq!(rows[2].added, ["same"]);
}
