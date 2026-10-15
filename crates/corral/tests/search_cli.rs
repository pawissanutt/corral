use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{Run, Status, Store};
use serde_json::{Value, json};

fn invoke(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
            command.env_remove(name);
        }
    }
    command
        .env("CORRAL_ROOT", root)
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .expect("corral runs")
}

fn success(root: &Path, args: &[&str]) -> Output {
    let out = invoke(root, args);
    assert!(out.status.success(), "{args:?}: {out:?}");
    out
}

fn rows(root: &Path, args: &[&str]) -> Vec<Value> {
    serde_json::from_slice(&success(root, args).stdout).expect("JSON task array")
}

fn ids(rows: &[Value]) -> Vec<&str> {
    rows.iter().map(|t| t["id"].as_str().unwrap()).collect()
}

fn workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    Store::init(tmp.path()).unwrap();
    tmp
}

fn run(root: &Path, summary: &str) -> Run {
    serde_json::from_value(json!({
        "id": "R1", "agent": "reviewer", "model": "test-model", "harness": "omo",
        "worktree": root.join("missing-worktree"), "branch": "corral/test",
        "base": "main", "base_sha": "0123456789abcdef",
        "started": "2026-01-01T00:00:00Z", "summary": summary,
        "usage": corral_core::Usage {
            cost_usd: Some(12.0),
            input: 2000,
            ..Default::default()
        }
    }))
    .unwrap()
}

#[test]
fn search_includes_every_status_in_json_and_plain_output() {
    // Given one matching task in every lifecycle state.
    let tmp = workspace();
    let store = Store::open(tmp.path()).unwrap();
    let statuses = [
        Status::Idea,
        Status::Backlog,
        Status::Ready,
        Status::Running,
        Status::NeedsMe,
        Status::Review,
        Status::Done,
        Status::Dropped,
    ];
    for status in statuses {
        store.create("Needle task", None, status, "").unwrap();
    }

    // When searching through both output formats.
    let json = rows(tmp.path(), &["search", "needle", "--json"]);
    let plain = String::from_utf8(success(tmp.path(), &["search", "needle"]).stdout).unwrap();

    // Then both contain every status, and JSON preserves the existing task shape.
    assert_eq!(json.len(), statuses.len());
    for status in statuses {
        assert!(json.iter().any(|t| t["status"] == status.as_str()));
    }
    assert_eq!(plain.lines().count(), json.len());
    for task in &json {
        assert!(plain.contains(task["id"].as_str().unwrap()));
        assert!(
            task["path"]
                .as_str()
                .unwrap()
                .starts_with(tmp.path().to_str().unwrap())
        );
        assert_eq!(task["title"], "Needle task");
    }
}

#[test]
fn comment_only_matches_and_negation_agree_in_search_and_list() {
    // Given durable comments on an idea and both terminal statuses.
    let tmp = workspace();
    let store = Store::open(tmp.path()).unwrap();
    let needle = "zqxcommentonly047";
    let mut expected = Vec::new();
    for status in [Status::Idea, Status::Done, Status::Dropped] {
        let task = store.create("Unrelated task", None, status, "").unwrap();
        success(tmp.path(), &["comment", &task.id, needle]);
        expected.push(task.id);
    }

    // When querying through both CLI entry points.
    for prefix in [vec!["search"], vec!["list", "--query"]] {
        let mut args = prefix.clone();
        args.extend([needle, "--json"]);
        let found = rows(tmp.path(), &args);
        let mut found_ids: Vec<_> = ids(&found).into_iter().map(str::to_string).collect();
        found_ids.sort();
        let mut negative = prefix;
        negative.extend(["id:* -zqxcommentonly047", "--json"]);
        let excluded = rows(tmp.path(), &negative);

        // Then comment-only matches include all statuses and negation excludes them.
        assert_eq!(found_ids, expected);
        assert!(excluded.is_empty());
    }
}

#[test]
fn search_ranks_title_ahead_of_body_summary_and_artifact_title() {
    // Given lower-ranked matches created before the title match.
    let tmp = workspace();
    let store = Store::open(tmp.path()).unwrap();
    let body = store
        .create("Body match", None, Status::Backlog, "needle")
        .unwrap();
    let summary = store
        .create("Summary match", None, Status::Done, "")
        .unwrap();
    store
        .update(&summary.id, |t| {
            t.runs.push(run(tmp.path(), "needle"));
            Ok(())
        })
        .unwrap();
    let artifact = store
        .create("Artifact match", None, Status::Dropped, "")
        .unwrap();
    let file = tmp.path().join("evidence.txt");
    std::fs::write(&file, "unrelated artifact content").unwrap();
    success(
        tmp.path(),
        &[
            "artifact",
            "add",
            &artifact.id,
            file.to_str().unwrap(),
            "--title",
            "needle",
        ],
    );
    let title = store.create("needle", None, Status::Ready, "").unwrap();
    store.create("No match", None, Status::Backlog, "").unwrap();

    // When searching and requesting the same query through list.
    let found = rows(tmp.path(), &["search", "needle", "--json"]);
    let listed = rows(tmp.path(), &["list", "--query", "needle", "--json"]);

    // Then title is first, all searchable sources match, and list retains rank.
    assert_eq!(found[0]["id"], title.id);
    assert_eq!(found.len(), 4);
    for id in [&body.id, &summary.id, &artifact.id] {
        assert!(ids(&found).contains(&id.as_str()));
    }
    assert_eq!(ids(&listed), ids(&found));
}

#[test]
fn list_query_intersects_explicit_status_and_project_filters() {
    // Given matching tasks across projects and finished states.
    let tmp = workspace();
    let store = Store::open(tmp.path()).unwrap();
    store
        .create("needle", Some("other"), Status::Done, "")
        .unwrap();
    let wanted = store
        .create("needle", Some("app"), Status::Done, "")
        .unwrap();
    store
        .create("needle", Some("app"), Status::Dropped, "")
        .unwrap();
    store
        .create("needle", Some("app"), Status::Ready, "")
        .unwrap();

    // When explicit list filters narrow the all-status search.
    let found = rows(
        tmp.path(),
        &[
            "list",
            "--query",
            "needle",
            "--status",
            "done",
            "--project",
            "app",
            "--json",
        ],
    );

    // Then only their intersection remains.
    assert_eq!(ids(&found), [wanted.id]);
}

#[test]
fn list_defaults_still_hide_finished_tasks_without_query() {
    // Given active, Done and Dropped tasks.
    let tmp = workspace();
    let store = Store::open(tmp.path()).unwrap();
    let active = store.create("needle", None, Status::Ready, "").unwrap();
    store.create("needle", None, Status::Done, "").unwrap();
    store.create("needle", None, Status::Dropped, "").unwrap();

    // When listing without query.
    let found = rows(tmp.path(), &["list", "--json"]);

    // Then the original active-only default holds.
    assert_eq!(ids(&found), [active.id]);
}

#[test]
fn list_all_still_includes_finished_tasks_without_query() {
    // Given active and finished tasks.
    let tmp = workspace();
    let store = Store::open(tmp.path()).unwrap();
    store.create("needle", None, Status::Ready, "").unwrap();
    store.create("needle", None, Status::Done, "").unwrap();
    store.create("needle", None, Status::Dropped, "").unwrap();

    // When opting into all statuses without a query.
    let found = rows(tmp.path(), &["list", "--all", "--json"]);

    // Then all three tasks remain visible.
    assert_eq!(found.len(), 3);
}

#[test]
fn done_query_preserves_finish_order_and_history_json() {
    // Given a newer body match, an older title match, and a dropped match.
    let tmp = workspace();
    let store = Store::open(tmp.path()).unwrap();
    let older = store
        .create("needle", Some("app"), Status::Done, "")
        .unwrap();
    let newer = store
        .create("Recent task", Some("app"), Status::Done, "needle")
        .unwrap();
    store
        .create("needle", Some("app"), Status::Dropped, "")
        .unwrap();
    for (id, at) in [
        (&older.id, "2026-01-01T00:00:00Z"),
        (&newer.id, "2026-02-01T00:00:00Z"),
    ] {
        let mut task = store.get(id).unwrap();
        task.updated = at.parse().unwrap();
        std::fs::write(&task.path, corral_core::render_task(&task)).unwrap();
    }

    // When searching the Done list.
    let found = rows(
        tmp.path(),
        &[
            "list",
            "--done",
            "--query",
            "needle",
            "--project",
            "app",
            "--json",
        ],
    );

    // Then finish order takes precedence over relevance, retaining history fields.
    assert_eq!(ids(&found), [newer.id, older.id]);
    assert_eq!(found[0]["finished"], "2026-02-01T00:00:00Z");
    assert!(found[0].get("landed").is_some());
}

#[test]
fn structured_query_filters_are_shared_by_search_and_list() {
    // Given one task with run metadata and an artifact, plus a nonmatching task.
    let tmp = workspace();
    let store = Store::open(tmp.path()).unwrap();
    let wanted = store
        .create("Needle", Some("app"), Status::Done, "")
        .unwrap();
    store
        .update(&wanted.id, |t| {
            t.kind = Some("fix".into());
            t.rounds = 3;
            t.runs.push(run(tmp.path(), "completed"));
            Ok(())
        })
        .unwrap();
    let file = tmp.path().join("report.md");
    std::fs::write(&file, "report").unwrap();
    success(
        tmp.path(),
        &["artifact", "add", &wanted.id, file.to_str().unwrap()],
    );
    success(tmp.path(), &["pin", &wanted.id]);
    store
        .log_event(
            Some(&wanted.id),
            "approved",
            json!({"sha": "0123456789abcdef", "strategy": "rebase"}),
        )
        .unwrap();
    store
        .create("Needle", Some("other"), Status::Ready, "")
        .unwrap();
    let mut task = store.get(&wanted.id).unwrap();
    task.updated = "2000-01-01T00:00:00Z".parse().unwrap();
    std::fs::write(&task.path, corral_core::render_task(&task)).unwrap();

    // When each supported field narrows the same query through both entry points.
    for filter in [
        "status:done",
        "project:app",
        "kind:fix",
        "profile:reviewer",
        "model:test-model",
        "harness:omo",
        "id:T-001",
        "id:*-001",
        "pinned",
        "before:2020-01-01",
        "cost>5",
        "tokens>1000",
        "rounds>2",
        "has:artifact",
        "-status:ready",
        "sha:0123456",
    ] {
        let query = format!("needle {filter}");
        let found = rows(tmp.path(), &["search", &query, "--json"]);
        let listed = rows(tmp.path(), &["list", "--query", &query, "--json"]);

        // Then the parser and filters agree, including archived tasks.
        assert_eq!(ids(&found), [wanted.id.as_str()], "{filter}");
        assert_eq!(ids(&listed), ids(&found), "{filter}");
    }
}

#[test]
fn since_query_excludes_old_tasks_without_a_boundary_race() {
    // Given an ancient task and one with the current update time.
    let tmp = workspace();
    let store = Store::open(tmp.path()).unwrap();
    let old = store.create("needle", None, Status::Done, "").unwrap();
    let recent = store.create("needle", None, Status::Dropped, "").unwrap();
    let mut task = store.get(&old.id).unwrap();
    task.updated = "2000-01-01T00:00:00Z".parse().unwrap();
    std::fs::write(&task.path, corral_core::render_task(&task)).unwrap();

    // When searching a seven-day window far from either fixture's boundary.
    let found = rows(tmp.path(), &["search", "needle since:7d", "--json"]);

    // Then only the recent archived task matches.
    assert_eq!(ids(&found), [recent.id]);
}

#[test]
fn unknown_fields_fail_clearly_in_both_entry_points() {
    // Given an initialized isolated store.
    let tmp = workspace();

    // When either command receives an unknown structured field.
    for args in [
        vec!["search", "bogus:value", "--json"],
        vec!["list", "--query", "bogus:value", "--json"],
        vec!["list", "--done", "--query", "bogus:value", "--json"],
    ] {
        let out = invoke(tmp.path(), &args);

        // Then it fails without emitting a success-shaped JSON result.
        assert!(!out.status.success(), "{args:?}");
        assert!(out.stdout.is_empty());
        let error = String::from_utf8(out.stderr).unwrap();
        assert!(
            error.contains("unknown") && error.contains("bogus"),
            "{error}"
        );
    }
}

#[test]
fn list_query_supports_selected_columns_and_empty_json_results() {
    // Given an archived task.
    let tmp = workspace();
    let store = Store::open(tmp.path()).unwrap();
    let task = store.create("needle", None, Status::Dropped, "").unwrap();

    // When using selected columns or searching for absent text.
    let out = success(
        tmp.path(),
        &["list", "--query", "needle", "--columns", "id,title"],
    );
    let empty = rows(tmp.path(), &["search", "absentword", "--json"]);

    // Then the existing list renderer works and no matches is an empty array.
    let plain = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        plain.split_whitespace().collect::<Vec<_>>(),
        [task.id.as_str(), "needle"]
    );
    assert!(empty.is_empty());
}
