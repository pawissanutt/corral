//! Random differential of the ledger cleanup against the db316644 cleanup (plan T-253 section
//! 4, "Pane references and cleanup"). Random tasks get runs, replaced gates whose earlier panes
//! only creation events retain, PM and pane reviewers, earlier ended and nested review
//! children, fallback tool workspaces and panes closed behind corral's back, through head and
//! full writes alike. Round and then Task cleanup close exactly the existing panes and
//! workspaces the old walk over the full task and its creation events closed, and a repeated
//! Task cleanup closes nothing.
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use corral_core::{Event, Run, Status, Store, Task};
use corral_herdr::Herdr;
use serde_json::{Value, json};

use super::{Cleanup, cleanup_head, cleanup_report};
use crate::Ctx;

const ENGINE: &str = r#"#!/usr/bin/env python3
import json, pathlib, sys
root = pathlib.Path(__file__).parent
path = root / 'state.json'
s = json.loads(path.read_text())
a = sys.argv[1:]
if a[:1] == ['--session']:
    a = a[2:]
def finish(result=None):
    path.write_text(json.dumps(s))
    print(json.dumps({'id': 'fake', 'result': result or {}}))
    sys.exit(0)
op = a[:2]
if op == ['pane', 'list']:
    finish({'panes': [{'pane_id': p, 'workspace_id': w, 'tab_id': w + ':t',
                       'agent_status': 'idle', 'cwd': '/'} for p, w in s['panes'].items()]})
if op == ['workspace', 'list']:
    finish({'workspaces': [{'workspace_id': w, 'label': w} for w in s['workspaces']]})
if op == ['pane', 'close']:
    s['calls'].append(['pane', a[2]])
    s['panes'].pop(a[2], None)
    finish()
if op == ['workspace', 'close']:
    s['calls'].append(['workspace', a[2]])
    s['workspaces'] = [w for w in s['workspaces'] if w != a[2]]
    s['panes'] = {p: w for p, w in s['panes'].items() if w != a[2]}
    finish()
finish()
"#;

struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }
}

struct World {
    _dir: tempfile::TempDir,
    ctx: Ctx,
    state: PathBuf,
    id: String,
    next: u64,
    socket: String,
}

const T: &str = "2026-10-18T12:00:00Z";

impl World {
    fn new() -> World {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let bin = dir.path().join("herdr");
        std::fs::write(&bin, ENGINE).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let state = dir.path().join("state.json");
        std::fs::write(&state, r#"{"panes":{},"workspaces":[],"calls":[]}"#).unwrap();
        let herdr = Herdr::with_bin(bin, Some("corral-test".into()));
        let socket = herdr.socket_path().unwrap().to_string_lossy().into_owned();
        let id = store
            .create("Ledger fixture", None, Status::Running, "")
            .unwrap()
            .id;
        World {
            _dir: dir,
            ctx: Ctx {
                store,
                herdr,
                resource_probe: crate::resources::FixedResourceProbe::arc(),
            },
            state,
            id,
            next: 0,
            socket,
        }
    }

    fn read(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.state).unwrap()).unwrap()
    }

    fn write(&self, value: &Value) {
        std::fs::write(&self.state, value.to_string()).unwrap();
    }

    /// A new live pane in `workspace`, which is listed from now on.
    fn pane(&mut self, workspace: &str) -> String {
        self.next += 1;
        let pane = format!("{workspace}:p{}", self.next);
        let mut s = self.read();
        s["panes"][&pane] = json!(workspace);
        let listed = s["workspaces"].as_array().unwrap();
        if !listed.iter().any(|w| w == workspace) {
            s["workspaces"]
                .as_array_mut()
                .unwrap()
                .push(json!(workspace));
        }
        self.write(&s);
        pane
    }

    fn workspace(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    /// A pane closed behind corral's back: no longer listed, its workspace kept.
    fn vanish(&self, rng: &mut Lcg) {
        let mut s = self.read();
        let panes: Vec<String> = s["panes"].as_object().unwrap().keys().cloned().collect();
        if let Some(pane) =
            panes.get(usize::try_from(rng.below(panes.len().max(1) as u64)).unwrap())
        {
            s["panes"].as_object_mut().unwrap().remove(pane);
            self.write(&s);
        }
    }

    fn existing(&self) -> (BTreeMap<String, String>, BTreeSet<String>) {
        let s = self.read();
        let panes = s["panes"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(p, w)| (p.clone(), w.as_str().unwrap().to_owned()))
            .collect();
        let workspaces = s["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap().to_owned())
            .collect();
        (panes, workspaces)
    }

    fn calls(&self) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut s = self.read();
        let mut out = (BTreeSet::new(), BTreeSet::new());
        for call in s["calls"].as_array().unwrap() {
            let target = call[1].as_str().unwrap().to_owned();
            if call[0] == "pane" {
                out.0.insert(target);
            } else {
                out.1.insert(target);
            }
        }
        s["calls"] = json!([]);
        self.write(&s);
        out
    }

    fn current_run(&self) -> Option<String> {
        let head = self.ctx.store.head(&self.id).unwrap();
        head.current_run.as_ref().map(|r| r.id.clone())
    }

    fn log(&self, kind: &str, detail: Value) {
        self.ctx
            .store
            .log_event(Some(&self.id), kind, detail)
            .unwrap();
    }
}

fn gate(pane: &str, socket: &str) -> Value {
    json!({"result": "running", "at": T, "sha": "0".repeat(40), "pane": pane, "socket": socket})
}

fn review(reviewer: &str, pane: &str) -> Value {
    json!({"reviewer": reviewer, "sha": "0".repeat(40), "at": T, "pane": pane})
}

fn step(world: &mut World, rng: &mut Lcg, runs: &mut u64) {
    let head_write = rng.below(2) == 0;
    let Some(run) = world.current_run().filter(|_| rng.below(6) != 0) else {
        *runs += 1;
        let workspace = world.workspace("wr");
        let pane = world.pane(&workspace);
        let run: Run = serde_json::from_value(json!({
            "id": format!("R{runs}"), "agent": "fake", "worktree": "/", "branch": "b",
            "base": "main", "base_sha": "0".repeat(40), "started": T,
            "herdr": {"workspace_id": workspace, "pane_id": pane, "socket": world.socket},
        }))
        .unwrap();
        world
            .ctx
            .store
            .update(&world.id, |t| {
                if let Some(previous) = t.runs.last_mut() {
                    previous.outcome = Some(corral_core::RunOutcome::Superseded);
                    previous.ended = Some(T.parse().unwrap());
                }
                t.runs.push(run);
                Ok(())
            })
            .unwrap();
        return;
    };
    let id = world.id.clone();
    match rng.below(5) {
        // A gate, replacing the previous one; sometimes in a fallback workspace.
        0 | 1 => {
            let workspace = if rng.below(3) == 0 {
                let workspace = world.workspace("wf");
                let pane = world.pane(&workspace);
                let socket = world.socket.clone();
                world
                    .ctx
                    .store
                    .update_head(&id, |h| {
                        h.record_run_workspace(&run, &pane, &workspace, &socket)?;
                        world.ctx.store.log_event(
                            Some(&id),
                            "run_workspace_created",
                            json!({"run": run, "workspace": workspace, "pane": pane}),
                        )
                    })
                    .unwrap();
                Some(pane)
            } else {
                None
            };
            let pane = match workspace {
                Some(pane) => pane,
                None => {
                    let head = world.ctx.store.head(&id).unwrap();
                    let ws = head.current_run.as_ref().unwrap().herdr.as_ref().unwrap();
                    let ws = ws.workspace_id.clone();
                    world.pane(&ws)
                }
            };
            let record = serde_json::from_value(gate(&pane, &world.socket)).unwrap();
            if head_write {
                world
                    .ctx
                    .store
                    .update_head(&id, |h| {
                        h.current_run_mut().unwrap().gate = Some(record);
                        Ok(())
                    })
                    .unwrap();
            } else {
                world
                    .ctx
                    .store
                    .update(&id, |t| {
                        t.current_run_mut().unwrap().gate = Some(record);
                        Ok(())
                    })
                    .unwrap();
            }
            world.log("gate_started", json!({"run": run, "pane": pane}));
        }
        // A reviewer on the run itself: a pane reviewer or the PM.
        2 => {
            let reviewer = if rng.below(3) == 0 { "pm" } else { "rev" };
            let pane = world.pane("wpm");
            let record = serde_json::from_value(review(reviewer, &pane)).unwrap();
            if head_write {
                world
                    .ctx
                    .store
                    .update_head(&id, |h| {
                        h.current_run_mut().unwrap().review = Some(record);
                        Ok(())
                    })
                    .unwrap();
            } else {
                world
                    .ctx
                    .store
                    .update(&id, |t| {
                        t.current_run_mut().unwrap().review = Some(record);
                        Ok(())
                    })
                    .unwrap();
            }
            world.log(
                "review_started",
                json!({"run": run, "reviewer": reviewer, "pane": pane}),
            );
        }
        // A review child with its own gate and reviewer, ended or not.
        3 => {
            let reviewer = if rng.below(3) == 0 { "pm" } else { "rev" };
            let (gate_pane, review_pane) = (world.pane("wc"), world.pane("wc"));
            let ended = rng.below(2) == 0;
            let socket = world.socket.clone();
            world
                .ctx
                .store
                .update(&id, |t| {
                    let parent = t.current_run_mut().unwrap();
                    let n = parent.review_runs.len() + 1;
                    let mut child: Run = serde_json::from_value(json!({
                        "id": format!("{run}-review{n}"), "agent": "rev", "worktree": "/",
                        "branch": "b", "base": "main", "base_sha": "0".repeat(40),
                        "started": T, "gate": gate(&gate_pane, &socket),
                        "review": review(reviewer, &review_pane),
                    }))?;
                    if ended {
                        child.ended = Some(T.parse()?);
                        child.outcome = Some(corral_core::RunOutcome::Done);
                    }
                    parent.review_runs.push(child);
                    Ok(())
                })
                .unwrap();
        }
        _ => world.vanish(rng),
    }
}

// The db316644 cleanup's pane walk and creation events, verbatim in substance.
fn review_panes<'a>(run: &'a Run, panes: &mut BTreeSet<&'a str>) {
    if let Some(pane) = run.gate.as_ref().and_then(|g| g.pane.as_deref()) {
        panes.insert(pane);
    }
    if let Some(review) = &run.review
        && review.reviewer != "pm"
        && let Some(pane) = review.pane.as_deref()
    {
        panes.insert(pane);
    }
    for child in &run.review_runs {
        review_panes(child, panes);
    }
}

/// What the db316644 cleanup would close: (panes, workspaces) of its snapshot.
fn oracle(task: &Task, events: &[Event], scope: Cleanup) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut workspaces = BTreeSet::new();
    let runs = match scope {
        Cleanup::Task => task.runs.as_slice(),
        Cleanup::Round => task.runs.last().map(std::slice::from_ref).unwrap_or(&[]),
    };
    let mut panes = BTreeSet::new();
    for run in runs {
        review_panes(run, &mut panes);
        if scope == Cleanup::Task
            && let Some(h) = &run.herdr
        {
            panes.insert(h.pane_id.as_str());
            workspaces.insert(h.workspace_id.clone());
        }
    }
    for event in events {
        if !runs
            .iter()
            .any(|run| event.detail["run"].as_str() == Some(&run.id))
        {
            continue;
        }
        let is_tool = event.kind == "gate_started"
            || (event.kind == "review_started"
                && event.detail["reviewer"].as_str().is_some_and(|r| r != "pm"));
        if (is_tool || event.kind == "run_workspace_created")
            && let Some(pane) = event.detail["pane"].as_str()
        {
            panes.insert(pane);
        }
        if scope == Cleanup::Task
            && event.kind == "run_workspace_created"
            && let Some(workspace) = event.detail["workspace"].as_str()
        {
            workspaces.insert(workspace.to_owned());
        }
    }
    (panes.into_iter().map(str::to_owned).collect(), workspaces)
}

/// The oracle's close calls on what exists: owned listed panes, then owned listed workspaces
/// left without a pane.
fn expected(world: &World, scope: Cleanup) -> (BTreeSet<String>, BTreeSet<String>) {
    let task = world.ctx.store.get(&world.id).unwrap();
    let events = world
        .ctx
        .store
        .events_of(
            Some(&world.id),
            &["gate_started", "review_started", "run_workspace_created"],
        )
        .unwrap();
    let (panes, workspaces) = oracle(&task, &events, scope);
    let (existing, listed) = world.existing();
    let closed: BTreeSet<String> = panes
        .into_iter()
        .filter(|p| existing.contains_key(p))
        .collect();
    let closed_workspaces = workspaces
        .into_iter()
        .filter(|w| listed.contains(w))
        .filter(|w| {
            !existing
                .iter()
                .any(|(p, ws)| ws == w && !closed.contains(p))
        })
        .collect();
    (closed, closed_workspaces)
}

#[test]
fn ledger_cleanup_closes_what_the_history_walk_closed() {
    for seed in 0..40_u64 {
        let mut world = World::new();
        let mut rng = Lcg(seed);
        let mut runs = 0;
        for _ in 0..4 + rng.below(12) {
            step(&mut world, &mut rng, &mut runs);
        }
        // Round: the send-back cleanup of the current run, through either entry.
        let (panes, _) = expected(&world, Cleanup::Round);
        if seed % 2 == 0 {
            let head = world.ctx.store.head(&world.id).unwrap();
            cleanup_head(&world.ctx, &head, Cleanup::Round);
        } else {
            let task = world.ctx.store.get(&world.id).unwrap();
            cleanup_report(&world.ctx, &task, Cleanup::Round);
        }
        assert_eq!(world.calls(), (panes, BTreeSet::new()), "seed {seed} round");
        // Task: the terminal cleanup of every run.
        let want = expected(&world, Cleanup::Task);
        let task = world.ctx.store.get(&world.id).unwrap();
        cleanup_report(&world.ctx, &task, Cleanup::Task);
        assert_eq!(world.calls(), want, "seed {seed} task");
        // Nothing stays owed that still exists, and nothing is closed twice.
        cleanup_report(&world.ctx, &task, Cleanup::Task);
        assert_eq!(world.calls().0, BTreeSet::new(), "seed {seed} repeat");
        let head = world.ctx.store.head(&world.id).unwrap();
        let (existing, _) = world.existing();
        for pane in head.unreleased_panes(false).unwrap() {
            assert!(
                existing.contains_key(&pane.pane) || pane.socket != world.socket,
                "seed {seed}: {pane:?} stays owed though closed"
            );
        }
    }
}

/// A first coding run R1 with its own pane.
fn start_run(world: &mut World) {
    let workspace = world.workspace("wr");
    let pane = world.pane(&workspace);
    let run: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "fake", "worktree": "/", "branch": "b",
        "base": "main", "base_sha": "0".repeat(40), "started": T,
        "herdr": {"workspace_id": workspace, "pane_id": pane, "socket": world.socket},
    }))
    .unwrap();
    world
        .ctx
        .store
        .update(&world.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
}

/// A gate opened in its own fallback workspace on the current run, replacing the previous one.
fn fallback_gate(world: &mut World) -> (String, String) {
    let run = world.current_run().unwrap();
    let id = world.id.clone();
    let workspace = world.workspace("wf");
    let pane = world.pane(&workspace);
    let socket = world.socket.clone();
    world
        .ctx
        .store
        .update_head(&id, |h| {
            h.record_run_workspace(&run, &pane, &workspace, &socket)?;
            h.current_run_mut().unwrap().gate = Some(serde_json::from_value(gate(&pane, &socket))?);
            Ok(())
        })
        .unwrap();
    (pane, workspace)
}

#[test]
fn repeated_round_cleanup_releases_closed_fallback_panes() {
    use corral_core::work::{self, Counter, HotScope};
    let mut measured = Vec::new();
    for n in [1, 10] {
        // Given a current run whose n earlier fallback gates were each cleaned by a Round.
        let mut world = World::new();
        start_run(&mut world);
        let mut workspaces = BTreeSet::new();
        for _ in 0..n {
            let (pane, workspace) = fallback_gate(&mut world);
            let head = world.ctx.store.head(&world.id).unwrap();
            cleanup_head(&world.ctx, &head, Cleanup::Round);
            assert_eq!(
                world.calls(),
                (BTreeSet::from([pane]), BTreeSet::new()),
                "x{n}"
            );
            workspaces.insert(workspace);
        }
        // When one more fallback gate is cleaned by a Round.
        let (pane, workspace) = fallback_gate(&mut world);
        workspaces.insert(workspace);
        let head = world.ctx.store.head(&world.id).unwrap();
        let delta = {
            let _hot = HotScope::enter();
            let before = work::snapshot();
            cleanup_head(&world.ctx, &head, Cleanup::Round);
            work::snapshot().since(&before)
        };
        // Then it closes only that pane, no closed pane stays open in the run's ledger, and
        // exactly the fallback workspaces stay owed to the Task cleanup.
        assert_eq!(
            world.calls(),
            (BTreeSet::from([pane]), BTreeSet::new()),
            "x{n}"
        );
        let head = world.ctx.store.head(&world.id).unwrap();
        let round: Vec<String> = head
            .unreleased_panes(true)
            .unwrap()
            .into_iter()
            .map(|p| p.kind)
            .collect();
        assert_eq!(
            round,
            ["run"],
            "x{n}: only the worker's own pane stays open"
        );
        let owed: BTreeSet<String> = head
            .unreleased_panes(false)
            .unwrap()
            .into_iter()
            .filter(|p| p.kind == "workspace_debt")
            .filter_map(|p| p.workspace)
            .collect();
        assert_eq!(owed, workspaces, "x{n}");
        measured.push(delta);
        // And the Task cleanup still closes every emptied fallback workspace and settles it.
        let task = world.ctx.store.get(&world.id).unwrap();
        cleanup_report(&world.ctx, &task, Cleanup::Task);
        let (_, closed) = world.calls();
        assert!(workspaces.is_subset(&closed), "x{n}: {closed:?}");
        let head = world.ctx.store.head(&world.id).unwrap();
        assert!(
            head.unreleased_panes(false)
                .unwrap()
                .iter()
                .all(|p| p.kind != "workspace_debt"),
            "x{n}"
        );
    }
    // And a Round reads no more as closed fallback panes accumulate on the run.
    for counter in Counter::DECODES {
        assert_eq!(
            measured[0].get(counter),
            measured[1].get(counter),
            "{counter:?}"
        );
    }
}
