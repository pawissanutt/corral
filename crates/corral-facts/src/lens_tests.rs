use super::Repo;
use crate::{
    DefinitionFact, EntryDetector, EntryKind, EntryPoint, FactsCache, ProviderCoverage, ReadStatus,
    Revision, RuntimeSource, TreeFacts,
    lenses::{self, LensCoverage},
    provider::Reply,
};
use corral_types::lenses::{LensResult, LensStatus};

const DATE: &str = "2026-11-23T12:00:00Z";
const DIR: &str = "diagrams/lenses";
const CODE: &str = r#"pub enum Status {
    Ready,
}
fn run() {
    execute();
}
fn other() { execute(); }
"#;

fn write(repo: &Repo, file: &str, kind: &str, id: &str, at: &Revision, body: &str) {
    repo.put(
        &format!("{DIR}/{file}.toml"),
        &format!(
            "id = {id:?}\ntitle = {id:?}\nkind = {kind:?}\nread_at = {:?}\n{body}",
            at.commit()
        ),
    );
}

fn tree(repo: &Repo, rev: &Revision) -> TreeFacts {
    FactsCache::new(repo.path())
        .tree_facts(repo.path(), rev.tree())
        .unwrap()
}

fn load(repo: &Repo, rev: &Revision) -> lenses::LensSources {
    lenses::load(repo.path(), rev, DIR).unwrap()
}

fn definitions(tree: &mut TreeFacts, start: u64, end: u64) {
    let file = tree
        .files
        .iter_mut()
        .find(|file| file.path == "code.rs")
        .unwrap();
    file.definitions.push(DefinitionFact {
        name: "run".into(),
        line: start,
        end_line: end,
        kind: "function".into(),
    });
}

fn by_id<'a>(rows: &'a [LensResult], id: &str) -> &'a LensResult {
    rows.iter()
        .find(|r| r.lens.as_ref().is_some_and(|lens| lens.id() == id))
        .unwrap()
}

const STATE: &str = r#"
states_from = { path = "code.rs", name = "Status" }
[[states]]
id = "ready"
label = "Ready"
anchor = { path = "code.rs", name = "Status::Ready" }
"#;

const SEQUENCE: &str = r#"
entry = { kind = "command", name = "dispatch" }
[[participants]]
id = "cli"
label = "CLI"
anchor = { path = "package.json" }
[[participants]]
id = "engine"
label = "Engine"
anchor = { path = "code.rs" }
[[messages]]
from = "cli"
to = "engine"
label = "execute"
anchor = { path = "code.rs", name = "run", text = "execute();" }
"#;

#[test]
fn checks_committed_anchors_scoped_text_and_variant_coverage() {
    let repo = Repo::new();
    repo.put("code.rs", CODE);
    repo.put(
        "package.json",
        r#"{"name":"fixture","bin":{"dispatch":"./dispatch.js"}}"#,
    );
    let code = repo.commit(DATE, "source");
    write(&repo, "state", "state", "lifecycle", &code, STATE);
    write(&repo, "flow", "sequence", "dispatch", &code, SEQUENCE);
    let confirmed = repo.commit(DATE, "lenses");

    // A dirty worktree neither supplies nor invalidates any checked facts.
    repo.put("code.rs", "dirty");
    repo.put(&format!("{DIR}/flow.toml"), "also dirty");
    let mut current_tree = tree(&repo, &confirmed);
    let sources = load(&repo, &confirmed);
    definitions(&mut current_tree, 4, 6);
    let rows = lenses::check(&sources, &current_tree, None);
    let state = by_id(&rows, "lifecycle");
    let flow = by_id(&rows, "dispatch");
    assert_eq!(state.status, LensStatus::Current);
    assert_eq!(flow.status, LensStatus::Current);
    assert!(
        state
            .not_checked
            .iter()
            .any(|issue| issue.element == "state ready")
    );
    assert_eq!(
        state
            .locations
            .iter()
            .find(|loc| loc.element == "state ready")
            .unwrap()
            .line,
        2
    );
    assert!(
        flow.locations
            .iter()
            .any(|loc| loc.element == "entry" && loc.path == "package.json")
    );
    let coverage = ProviderCoverage {
        provided: vec!["files".into(), "definitions".into()],
        ..ProviderCoverage::unavailable(&[])
    };
    let no_file_rows = LensCoverage {
        provider: &coverage,
        unread_files: &Default::default(),
    };
    let rows = lenses::check(&sources, &current_tree, Some(no_file_rows));
    let flow = by_id(&rows, "dispatch");
    assert_eq!(flow.status, LensStatus::Current);
    assert!(flow.not_checked.is_empty());

    let file = current_tree
        .files
        .iter_mut()
        .find(|file| file.path == "code.rs")
        .unwrap();
    file.definitions.clear();
    file.read_status = ReadStatus::Unsupported;
    let rows = lenses::check(&sources, &current_tree, Some(no_file_rows));
    assert_eq!(by_id(&rows, "dispatch").status, LensStatus::Stale);
    let reply = Reply::parse(
        br#"{"type":"file","path":"code.rs","read":"unsupported"}
{"type":"done","provided":["files","definitions"],"skipped":{},"omitted":{}}
"#,
        &["files", "definitions"],
    )
    .unwrap();
    let unread_files = reply.unread_files();
    let partial = reply.merge_snapshot(&mut current_tree);
    let rows = lenses::check(
        &sources,
        &current_tree,
        Some(LensCoverage {
            provider: &partial,
            unread_files: &unread_files,
        }),
    );
    let flow = by_id(&rows, "dispatch");
    assert_eq!(flow.status, LensStatus::Current);
    assert!(
        flow.not_checked
            .iter()
            .any(|issue| issue.element.starts_with("message "))
    );

    current_tree.entry_points.coverage = [(".".into(), [EntryDetector::Extension].into())].into();
    current_tree.entry_points.entries = vec![EntryPoint {
        kind: EntryKind::Route,
        name: "GET /tasks".into(),
        path: "code.rs".into(),
        line: 4,
        owner: RuntimeSource::Container { path: ".".into() },
    }];
    let rows = lenses::check(&sources, &current_tree, None);
    let flow = by_id(&rows, "dispatch");
    assert_eq!(flow.status, LensStatus::Current);
    assert!(
        flow.not_checked
            .iter()
            .any(|issue| issue.element == "entry")
    );

    let edits = CODE
        .replace("    Ready,", "    Ready,\n    Waiting,")
        .replace("    execute();", "    queue();");
    repo.put("code.rs", &edits);
    repo.put("language.txt", &edits);
    write(&repo, "flow", "sequence", "dispatch", &code, SEQUENCE);
    let variants = repo.commit(DATE, "add variant and remove anchored call");
    let mut variants_tree = tree(&repo, &variants);
    definitions(&mut variants_tree, 5, 7);
    // There is still a matching literal in another function. Only the named
    // definition can establish whether the anchored call survived.
    let results = lenses::check(&load(&repo, &variants), &variants_tree, Some(no_file_rows));
    let sequence = by_id(&results, "dispatch");
    assert_eq!(sequence.status, LensStatus::Stale);
    assert_eq!(sequence.failures.len(), 1);
    assert!(sequence.failures[0].element.starts_with("message "));
    assert!(sequence.failures[0].message.contains("execute();"));
    let state = by_id(
        &lenses::check(&load(&repo, &variants), &variants_tree, None),
        "lifecycle",
    )
    .clone();
    assert_eq!(state.status, LensStatus::Incomplete);
    assert_eq!(state.missing_states, ["Waiting"]);
    assert!(state.extra_states.is_empty());

    write(
        &repo,
        "state",
        "state",
        "lifecycle",
        &code,
        &STATE.replace("code.rs", "language.txt"),
    );
    let unavailable = repo.commit(DATE, "unsupported enum source");
    let results = lenses::check(&load(&repo, &unavailable), &tree(&repo, &unavailable), None);
    let state = by_id(&results, "lifecycle");
    assert_eq!(state.status, LensStatus::Current);
    assert!(
        state
            .not_checked
            .iter()
            .any(|issue| issue.element == "states_from")
    );
}

#[test]
fn partial_entry_facts_cannot_rule_out_a_sequence_entry() {
    let repo = Repo::new();
    repo.put("routes.txt", "GET /wanted\n");
    let at = repo.commit(DATE, "source");
    write(
        &repo,
        "route",
        "sequence",
        "route",
        &at,
        r#"entry = { kind = "route", name = "GET /wanted" }
[[participants]]
id = "routes"
label = "Routes"
anchor = { path = "routes.txt", text = "GET /wanted" }
"#,
    );
    let rev = repo.commit(DATE, "route lens");
    let sources = load(&repo, &rev);
    let builtin = tree(&repo, &rev);
    let check = |name: &str, omitted: u64| {
        let mut facts = builtin.clone();
        let output = format!(
            r#"{{"type":"entry","path":"routes.txt","line":1,"kind":"route","name":{name:?}}}
{{"type":"done","provided":["entries"],"skipped":{{}},"omitted":{{"limit":{omitted}}}}}
"#,
        );
        let reply = Reply::parse(output.as_bytes(), &["entries"]).unwrap();
        let unread_files = reply.unread_files();
        let coverage = reply.merge_snapshot(&mut facts);
        lenses::check(
            &sources,
            &facts,
            Some(LensCoverage {
                provider: &coverage,
                unread_files: &unread_files,
            }),
        )
        .pop()
        .unwrap()
    };

    let unverified = check("GET /other", 1);
    assert_eq!(unverified.status, LensStatus::Current);
    assert!(unverified.failures.is_empty());
    assert!(
        unverified
            .not_checked
            .iter()
            .any(|issue| issue.element == "entry")
    );

    let matched = check("GET /wanted", 1);
    assert_eq!(matched.status, LensStatus::Current);
    assert!(matched.not_checked.is_empty());
    assert!(
        matched
            .locations
            .iter()
            .any(|location| location.element == "entry")
    );

    let unknown = check("GET /other", 0);
    assert_eq!(unknown.status, LensStatus::Stale);
    assert!(
        unknown
            .failures
            .iter()
            .any(|issue| issue.element == "entry")
    );
}

#[test]
fn malformed_files_do_not_hide_other_lenses() {
    let repo = Repo::new();
    repo.put("code.rs", CODE);
    let at = repo.commit(DATE, "source");
    write(&repo, "sub/good", "state", "good", &at, STATE);
    write(&repo, "kind", "flow", "unknown-kind", &at, "");
    write(
        &repo,
        "key",
        "state",
        "unknown-key",
        &at,
        &format!("unexpected = true\n{STATE}"),
    );
    write(
        &repo,
        "nested",
        "state",
        "unknown-nested-key",
        &at,
        &STATE.replace("Status::Ready\"", "Status::Ready\", line = 2"),
    );
    write(
        &repo,
        "endpoint",
        "sequence",
        "unknown-endpoint",
        &at,
        &SEQUENCE.replace("to = \"engine\"", "to = \"unknown\""),
    );
    write(
        &repo,
        "element",
        "sequence",
        "duplicate-element",
        &at,
        &SEQUENCE.replace("id = \"engine\"", "id = \"cli\""),
    );
    write(&repo, "duplicate/a", "sequence", "duplicate-lens", &at, "");
    write(&repo, "b", "sequence", "duplicate-lens", &at, "");
    write(
        &repo,
        "path",
        "state",
        "noncanonical-path",
        &at,
        &STATE.replace("code.rs", "sub/../code.rs"),
    );
    let rev = repo.commit(DATE, "one valid and several independent invalid lenses");
    let rows = lenses::check(&load(&repo, &rev), &tree(&repo, &rev), None);
    assert_eq!(rows.len(), 9);
    assert_eq!(by_id(&rows, "good").status, LensStatus::Current);
    for row in rows.iter().filter(|row| row.status != LensStatus::Current) {
        assert_eq!(row.status, LensStatus::Malformed);
        assert!(row.lens.is_none());
        assert_eq!(row.failures.len(), 1, "{}", row.file);
    }
}

#[test]
fn review_uses_edited_lines_in_each_snapshot_and_reports_element_changes() {
    let repo = Repo::new();
    repo.put("code.rs", CODE);
    repo.put("package.json", "{}");
    let at = repo.commit(DATE, "source");
    // Give the participant a specific source line as well: removing the
    // prepended comments would touch a path-only anchor at line one.
    let flow = SEQUENCE.replace(
        "anchor = { path = \"code.rs\" }",
        "anchor = { path = \"code.rs\", text = \"pub enum Status {\" }",
    );
    write(&repo, "flow", "sequence", "dispatch", &at, &flow);
    let base = repo.commit(DATE, "lens");
    let compare = |base: &Revision, head: &Revision, before_def, after_def| {
        let mut before = tree(&repo, base);
        let mut after = tree(&repo, head);
        if let Some((start, end)) = before_def {
            definitions(&mut before, start, end);
        }
        if let Some((start, end)) = after_def {
            definitions(&mut after, start, end);
        }
        lenses::impact(
            repo.path(),
            base,
            head,
            &load(&repo, base),
            &load(&repo, head),
            &before,
            &after,
        )
        .unwrap()
    };

    let shifted_code = format!("// one\n// two\n// three\n// four\n// five\n{CODE}");
    repo.put("code.rs", &shifted_code);
    let shifted = repo.commit(DATE, "insert lines above every anchor");
    assert!(compare(&base, &shifted, Some((4, 6)), Some((9, 11))).is_empty());
    assert!(compare(&shifted, &base, Some((9, 11)), Some((4, 6))).is_empty());

    let extended_code = shifted_code.replace("    execute();", "    debug();\n    execute();");
    repo.put("code.rs", &extended_code);
    let extended = repo.commit(DATE, "edit inside definition without editing anchor line");
    assert!(compare(&shifted, &extended, None, None).is_empty());
    let impact = compare(&shifted, &extended, Some((9, 11)), Some((9, 12)));
    assert_eq!(impact.len(), 1);
    assert_eq!(impact[0].touched.len(), 1);
    assert!(impact[0].touched[0].contains("execute"));

    // Delete the anchored literal (leave another occurrence elsewhere) and
    // replace one participant. The old coordinate proves the message was touched.
    repo.put("code.rs", &extended_code.replace("    execute();\n", ""));
    repo.put("new-path.txt", "newly added line\n");
    let edited_lens = flow
        .replace(
            "anchor = { path = \"package.json\" }",
            "anchor = { path = \"new-path.txt\" }",
        )
        .replace("id = \"cli\"", "id = \"driver\"")
        .replace("from = \"cli\"", "from = \"driver\"");
    let (_, message) = edited_lens.split_once("[[messages]]").unwrap();
    write(
        &repo,
        "flow",
        "sequence",
        "dispatch",
        &at,
        &format!("{edited_lens}\n[[messages]]{message}"),
    );
    let edited = repo.commit(DATE, "delete anchored call and switch lens participant");
    let impact = compare(&extended, &edited, None, None);
    assert_eq!(impact.len(), 1);
    let row = &impact[0];
    assert_eq!(row.path, format!("{DIR}/flow.toml"));
    assert!(row.touched.iter().any(|item| item == "participant driver"));
    assert!(row.touched.iter().any(|item| item.contains("execute")));
    assert!(row.added.iter().any(|item| item == "participant driver"));
    assert_eq!(
        row.added
            .iter()
            .filter(|item| item.starts_with("message "))
            .count(),
        2
    );
    assert!(row.removed.iter().any(|item| item == "participant cli"));

    repo.put("new-path.txt", "\0binary replacement");
    let binary = repo.commit(DATE, "replace anchored source with binary");
    let impact = compare(&edited, &binary, None, None);
    assert_eq!(impact.len(), 1);
    assert_eq!(impact[0].touched, ["participant driver"]);
}
