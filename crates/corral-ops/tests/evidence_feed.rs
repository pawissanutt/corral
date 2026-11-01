//! Round-2 regressions of T-325 (plan T-253 section 5): one artifact resolves as one keyed row
//! whatever its run, task and board hold; the search index follows the change feed from one
//! read view with durable positions, so a cold process reads only the new events and a change
//! is either indexed or stays beyond the published cursor.
use std::fs;
use std::process::Command;

use corral_core::work::{self, Counter, Work};
use corral_core::{ArtifactKind, Status, Store};
use corral_ops::{
    ContentSearch, Ctx, add_artifact, artifact_path, refresh_search_index,
    refresh_search_index_with_tasks, resolve_artifact, search_content,
};

type Counted = Vec<(Counter, u64)>;

fn ctx(root: &std::path::Path) -> Ctx {
    corral_ops::resources::FixedResourceProbe::ctx(Store::open(root).unwrap())
}

fn add(
    ctx: &Ctx,
    root: &std::path::Path,
    task: &str,
    name: &str,
    bytes: &[u8],
) -> corral_core::Artifact {
    let path = root.join("sources").join(task).join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    add_artifact(ctx, task, &path, Some(ArtifactKind::Report), None).unwrap()
}

fn counted(work: impl FnOnce()) -> (Counted, u64) {
    let lists = work::count(Work::TaskList);
    let before = work::snapshot();
    let scope = work::HotScope::enter();
    work();
    drop(scope);
    let after = work::snapshot();
    let counters = Counter::ALL
        .into_iter()
        .map(|c| (c, after.total(c) - before.total(c)))
        .collect();
    (counters, work::count(Work::TaskList) - lists)
}

/// Resolving one artifact by stored path, reference, numeric pin and followed pin, and
/// opening it, with `grow` more artifacts in its own run, `grow` in another run and `grow`
/// terminal tasks of `grow` artifacts.
fn single_lookups(grow: usize) -> (Counted, u64, Vec<String>) {
    let tmp = tempfile::tempdir().unwrap();
    Store::init(tmp.path()).unwrap();
    let ctx = ctx(tmp.path());
    ctx.store
        .fixture_pin_now("2026-01-01T00:00:00Z".parse().unwrap())
        .unwrap();
    let active = ctx
        .store
        .create("Active", None, Status::Ready, "")
        .unwrap()
        .id;
    let target = add(&ctx, tmp.path(), &active, "report.md", b"# report\n");
    let notes = add(&ctx, tmp.path(), &active, "notes.md", b"# notes\n");
    for i in 0..grow {
        add(
            &ctx,
            tmp.path(),
            &active,
            &format!("same-run-{i:03}.md"),
            format!("same {i}").as_bytes(),
        );
    }
    let fixed: jiff::Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
    ctx.store
        .update(&active, |t| {
            let mut other = Vec::new();
            for i in 0..grow {
                let mut a = t.artifacts[0].clone();
                a.run = "R7".into();
                a.path = format!("artifacts/{}/R7/{i:08x}-other.md", t.id);
                a.title = format!("other {i}");
                other.push(a);
            }
            t.artifacts.extend(other);
            t.artifacts[0].added = fixed;
            t.artifacts[1].added = fixed;
            Ok(())
        })
        .unwrap();
    for t in 0..grow {
        let task = ctx.store.create("Done", None, Status::Done, "").unwrap().id;
        for i in 0..grow {
            add(
                &ctx,
                tmp.path(),
                &task,
                &format!("a{i:03}.md"),
                format!("{t} {i}").as_bytes(),
            );
        }
    }
    let reference = corral_core::PinRef::of_artifact(&active, &target).to_string();
    ctx.store.pin(&reference, None, false).unwrap();
    let followed = corral_core::PinRef::of_artifact(&active, &notes).to_string();
    ctx.store.pin(&followed, None, true).unwrap();
    let mut seen = Vec::new();
    let (counters, lists) = counted(|| {
        let by_path = resolve_artifact(&ctx, &target.path).unwrap();
        let by_ref = resolve_artifact(&ctx, &reference).unwrap();
        let unprefixed = format!("{active}:R0:report.md");
        let by_name = resolve_artifact(&ctx, &unprefixed).unwrap();
        let by_pin = resolve_artifact(&ctx, "1").unwrap();
        let followed = resolve_artifact(&ctx, "2").unwrap();
        let opened = artifact_path(&ctx, &by_ref).unwrap();
        seen = vec![
            by_path.path,
            by_ref.path,
            by_name.path,
            by_pin.path,
            followed.path,
        ];
        seen.push(fs::read_to_string(opened).unwrap());
    });
    (counters, lists, seen)
}

#[test]
fn one_artifact_resolves_as_one_keyed_row_whatever_its_run_task_and_board_hold() {
    // Given the same artifact alone, then beside 10 more in its own run, 10 in another run
    // and 10 terminal tasks of 10 artifacts.
    let (base, base_lists, base_seen) = single_lookups(1);
    let (grown, grown_lists, grown_seen) = single_lookups(10);
    // Then path, reference, unprefixed-name, pin and followed-pin resolution and opening do
    // byte-identical work at both sizes, never hydrate a task or list the board, and agree.
    assert_eq!((base_lists, grown_lists), (0, 0));
    assert_eq!(base, grown);
    let hydrations = base.iter().find(|(c, _)| *c == Counter::FullTaskHydrations);
    assert_eq!(hydrations.map(|(_, n)| *n), Some(0));
    assert_eq!(base_seen, grown_seen);
    assert_eq!(base_seen[5], "# report\n");
}

const CHILD: &str = "CORRAL_T325_FOLLOW_CHILD";

/// Runs one index refresh of the workspace at `root` in a fresh process and returns its counters.
fn refresh_in_child(root: &std::path::Path) -> Counted {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "child_process_refresh_entry",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find_map(|l| l.split_once("COUNTERS ").map(|(_, rest)| rest))
        .unwrap_or_else(|| panic!("child printed no counters: {stdout}"));
    let values: Vec<u64> = serde_json::from_str(line).unwrap();
    Counter::ALL.into_iter().zip(values).collect()
}

/// The child half of [`refresh_in_child`]; without its environment variable it does nothing.
#[test]
fn child_process_refresh_entry() {
    let Some(root) = std::env::var_os(CHILD) else {
        return;
    };
    let ctx = ctx(std::path::Path::new(&root));
    let (counters, lists) = counted(|| refresh_search_index_with_tasks(&ctx, &[]).unwrap());
    assert_eq!(lists, 0);
    let values: Vec<u64> = counters.iter().map(|(_, n)| *n).collect();
    println!("COUNTERS {}", serde_json::to_string(&values).unwrap());
}

/// A cold-process refresh after one appended event and one body change of a task that holds
/// `history` comments and artifacts, beside `history` earlier events.
fn cold_follow(history: usize) -> (Counted, bool) {
    let tmp = tempfile::tempdir().unwrap();
    Store::init(tmp.path()).unwrap();
    let ctx = ctx(tmp.path());
    let active = ctx
        .store
        .create("Active", None, Status::Ready, "old")
        .unwrap()
        .id;
    for i in 0..history {
        ctx.store
            .log_event(
                None,
                "probe",
                serde_json::json!({ "text": format!("history {i:04}") }),
            )
            .unwrap();
        corral_ops::comment(
            &ctx,
            &active,
            &format!("comment {i:04}"),
            corral_core::CommentAuthor::Owner,
            false,
        )
        .unwrap();
        add(
            &ctx,
            tmp.path(),
            &active,
            &format!("h{i:04}.md"),
            format!("h {i:04}").as_bytes(),
        );
    }
    refresh_search_index(&ctx).unwrap();
    ctx.store
        .log_event(
            None,
            "probe",
            serde_json::json!({ "text": "coldeventprobe" }),
        )
        .unwrap();
    ctx.store
        .update(&active, |t| {
            t.body = "coldbodyprobe".into();
            Ok(())
        })
        .unwrap();
    let counters = refresh_in_child(tmp.path());
    let found = ["coldeventprobe", "coldbodyprobe"].iter().all(|token| {
        search_content(&ctx, token, &ContentSearch::default())
            .unwrap()
            .len()
            == 1
    });
    let counters = counters
        .into_iter()
        .filter(|(c, _)| *c != Counter::DecodedDocBytes)
        .collect();
    (counters, found)
}

#[test]
fn a_cold_process_follows_new_events_and_a_changed_task_without_rereading_history() {
    // Given an index built over 10, then 100, earlier events, comments and artifacts.
    let (small, small_found) = cold_follow(10);
    let (large, large_found) = cold_follow(100);
    // Then a fresh process following one new event and one body change does identical counted
    // work at both sizes, and both changes are searchable.
    assert_eq!(small, large);
    assert!(small_found && large_found);
}

#[test]
fn a_change_committed_after_the_listing_is_indexed_or_stays_beyond_the_cursor() {
    // Given an indexed task and a listing taken before a writer commits a new body.
    let tmp = tempfile::tempdir().unwrap();
    Store::init(tmp.path()).unwrap();
    let ctx = ctx(tmp.path());
    let id = ctx
        .store
        .create("Raced", None, Status::Ready, "old")
        .unwrap()
        .id;
    refresh_search_index(&ctx).unwrap();
    let stale = ctx.store.list().unwrap();
    ctx.store
        .update(&id, |t| {
            t.body = "newlycommittedtoken".into();
            Ok(())
        })
        .unwrap();
    // When refreshing with the stale listing, then again with a fresh one.
    refresh_search_index_with_tasks(&ctx, &stale).unwrap();
    refresh_search_index_with_tasks(&ctx, &ctx.store.list().unwrap()).unwrap();
    // Then the committed body is indexed: the cursor never acknowledged it unindexed.
    let hits = search_content(&ctx, "newlycommittedtoken", &ContentSearch::default()).unwrap();
    assert_eq!(hits.len(), 1);
    let cache = ctx.store.cache_root().unwrap().join("content-search.json");
    assert!(
        fs::read_to_string(cache)
            .unwrap()
            .contains("newlycommittedtoken")
    );
    // And an explicit reindex publishes its own view's position: a later change still follows.
    search_content(
        &ctx,
        "old",
        &ContentSearch {
            reindex: true,
            ..Default::default()
        },
    )
    .unwrap();
    ctx.store
        .update(&id, |t| {
            t.body = "afterreindextoken".into();
            Ok(())
        })
        .unwrap();
    refresh_search_index_with_tasks(&ctx, &stale).unwrap();
    let cache = ctx.store.cache_root().unwrap().join("content-search.json");
    assert!(
        fs::read_to_string(cache)
            .unwrap()
            .contains("afterreindextoken")
    );
}

/// Decoded bytes of reading `id`'s head now, to separate it from a measured step's own work.
fn head_bytes(ctx: &Ctx, id: &str) -> u64 {
    let (counters, _) = counted(|| {
        ctx.store.head(id).unwrap();
    });
    counters
        .iter()
        .find(|(c, _)| *c == Counter::DecodedDocBytes)
        .map_or(0, |(_, n)| *n)
}

/// Every counter of `counters`, with the changed task's head bytes taken out of the bytes.
fn beyond_head(mut counters: Counted, head: u64) -> Counted {
    for (c, n) in &mut counters {
        if *c == Counter::DecodedDocBytes {
            *n -= head;
        }
    }
    counters
}

fn run(id: &str, started: &str, ended: bool, summary: Option<&str>) -> corral_core::Run {
    let mut run = serde_json::json!({
        "id": id, "agent": "fake", "worktree": "/nonexistent", "branch": "b", "base": "main",
        "base_sha": "0".repeat(40), "started": started,
    });
    if ended {
        run["ended"] = serde_json::json!(started);
        run["outcome"] = serde_json::json!("superseded");
    }
    if let Some(summary) = summary {
        run["summary"] = serde_json::json!(summary);
    }
    serde_json::from_value(run).unwrap()
}

/// Counters of the public refresh and of sync's changed-stamp route on a board of `grow`
/// terminal tasks, and of a current-run scalar change and a current-run summary change on a
/// task holding `grow` comments and artifacts. Its two ended runs stay fixed: a run change
/// re-derives the task's run documents from their rows (per-row change records are a follow-up).
fn feed_steps(grow: usize) -> (Vec<Counted>, bool) {
    let tmp = tempfile::tempdir().unwrap();
    Store::init(tmp.path()).unwrap();
    let ctx = ctx(tmp.path());
    ctx.store
        .fixture_pin_now("2026-01-01T00:00:00Z".parse().unwrap())
        .unwrap();
    let active = ctx
        .store
        .create("Active", None, Status::Running, "body")
        .unwrap()
        .id;
    for i in 0..grow {
        corral_ops::comment(
            &ctx,
            &active,
            &format!("old {i:04}"),
            corral_core::CommentAuthor::Owner,
            false,
        )
        .unwrap();
        add(
            &ctx,
            tmp.path(),
            &active,
            &format!("old{i:04}.md"),
            format!("old {i:04}").as_bytes(),
        );
        let task = ctx
            .store
            .create("Done", None, Status::Done, "done")
            .unwrap()
            .id;
        add(
            &ctx,
            tmp.path(),
            &task,
            "a.md",
            format!("done {i:04}").as_bytes(),
        );
    }
    ctx.store
        .update(&active, |t| {
            for i in 0..2 {
                let id = format!("R{}", i + 1);
                t.runs
                    .push(run(&id, "2026-01-01T00:00:00Z", true, Some("old summary")));
            }
            let current = "R3".to_string();
            t.runs
                .push(run(&current, "2026-01-02T00:00:00Z", false, None));
            Ok(())
        })
        .unwrap();
    refresh_search_index(&ctx).unwrap();
    let mut steps = Vec::new();
    // An unchanged feed through the public wrapper reads nothing.
    steps.push(counted(|| refresh_search_index(&ctx).unwrap()).0);
    // A current-run scalar change, then a current-run summary change.
    for summary in [None, Some("currentsummaryprobe")] {
        ctx.store
            .update(&active, |t| {
                let run = t.current_run_mut().unwrap();
                run.agent_state = Some("working".into());
                run.summary = summary.map(str::to_string);
                Ok(())
            })
            .unwrap();
        let (counters, lists) = counted(|| refresh_search_index(&ctx).unwrap());
        assert_eq!(lists, 0);
        steps.push(beyond_head(counters, head_bytes(&ctx, &active)));
    }
    let found = search_content(&ctx, "currentsummaryprobe", &ContentSearch::default())
        .unwrap()
        .len()
        == 1;
    (steps, found)
}

#[test]
fn public_refresh_and_current_run_changes_do_work_independent_of_history() {
    // Given a board and an active task with 1, then 10, terminal tasks and ended runs,
    // comments and artifacts.
    let (small, small_found) = feed_steps(1);
    let (large, large_found) = feed_steps(10);
    // Then the unchanged public refresh, a scalar current-run change and a current-run summary
    // change do identical work at both sizes (bytes beyond the changed head included), never
    // hydrate a task, and the new summary is searchable.
    assert_eq!(small, large);
    for step in &small {
        let hydrations = step.iter().find(|(c, _)| *c == Counter::FullTaskHydrations);
        assert_eq!(hydrations.map(|(_, n)| *n), Some(0));
    }
    assert!(
        small[0]
            .iter()
            .all(|(c, n)| *n == 0 || *c == Counter::ViewsOpened)
    );
    assert!(small_found && large_found);
}

/// The indexed text of document `reference`, if the persisted index holds it.
fn indexed(ctx: &Ctx, reference: &str) -> Option<String> {
    let path = ctx.store.cache_root().unwrap().join("content-search.json");
    let cache: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    cache["documents"][reference]["text"]
        .as_str()
        .map(str::to_string)
}

#[test]
fn a_run_demoted_after_its_change_keeps_its_indexed_text_like_a_rebuild() {
    for same_write in [false, true] {
        // Given an indexed task whose current run R1 has no summary.
        let tmp = tempfile::tempdir().unwrap();
        Store::init(tmp.path()).unwrap();
        let ctx = ctx(tmp.path());
        let id = ctx
            .store
            .create("Demoted", None, Status::Running, "body")
            .unwrap()
            .id;
        ctx.store
            .update(&id, |t| {
                t.runs.push(run("R1", "2026-01-01T00:00:00Z", false, None));
                Ok(())
            })
            .unwrap();
        refresh_search_index(&ctx).unwrap();
        // When R1 gets its final summary and R2 starts, in separate commits or in one write.
        let finish = |t: &mut corral_core::Task| {
            let r1 = t.runs.last_mut().unwrap();
            r1.summary = Some("finalsummarytoken".into());
            r1.ended = Some("2026-01-01T01:00:00Z".parse().unwrap());
        };
        let start = |t: &mut corral_core::Task| {
            t.runs.push(run("R2", "2026-01-02T00:00:00Z", false, None));
        };
        if same_write {
            ctx.store
                .update(&id, |t| {
                    finish(t);
                    start(t);
                    Ok(())
                })
                .unwrap();
        } else {
            ctx.store
                .update(&id, |t| {
                    finish(t);
                    Ok(())
                })
                .unwrap();
            ctx.store
                .update(&id, |t| {
                    start(t);
                    Ok(())
                })
                .unwrap();
        }
        refresh_search_index(&ctx).unwrap();
        refresh_search_index(&ctx).unwrap();
        // Then the followed index holds R1's final summary, as a canonical rebuild does.
        let reference = format!("{id}:R1:summary");
        let followed = indexed(&ctx, &reference);
        let reindex = ContentSearch {
            reindex: true,
            ..Default::default()
        };
        search_content(&ctx, "x", &reindex).unwrap();
        assert_eq!(
            followed,
            indexed(&ctx, &reference),
            "same write: {same_write}"
        );
        assert_eq!(followed.as_deref(), Some("finalsummarytoken"));
    }
}
