//! Native "Implement here": only the current ready revision's choice opens scratch work.
use super::*;
use crate::assistant::workflow::handoff::{
    native::{NativeAction, authenticate, binding, consume},
    native_implement::ImplementChoice,
    native_view,
};

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=F", "-c", "user.email=f@invalid"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn choose(f: &Fixture, action: &NativeAction, shown: &str) -> String {
    let c = f.container();
    let (_, digest) = binding(&c, &f.session, action, shown).unwrap();
    let text = action.option().unwrap();
    f.fx.ctx
        .store
        .update_assistant_container(&f.session, |c| {
            Ok(c.journal_owner_input(
                &f.session,
                NewOwnerInput {
                    generation: f.host.identity.generation,
                    turn: None,
                    source: OwnerInputSource::Choice,
                    sha256: BlobHash::of(text.as_bytes()).to_string(),
                    text,
                    truncated: false,
                    command: None,
                    answers: None,
                    request: format!("ev-1-{}", c.owner_inputs.len() + 300),
                    binding: Some(digest),
                },
            )
            .id)
        })
        .unwrap()
}

fn select(f: &Fixture, action: NativeAction) -> anyhow::Result<Value> {
    let shown = native_view::view(&f.fx.ctx, &f.session, &f.container())?.digest;
    let input = choose(f, &action, &shown);
    let evidence = authenticate(
        &f.container(),
        &f.session,
        f.host.identity.generation,
        &input,
        &action,
        &shown,
    )?;
    consume(&f.fx.ctx, evidence, action, &shown)
}

#[test]
fn implement_here_needs_the_current_ready_revision_and_opens_scratch_once_without_shipping() {
    let mut f = Fixture::new();
    f.planning();
    let repo = f.fx._tmp.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), ".worktrees/\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "fixture"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    f.fx.ctx.store.config.projects.get_mut("app").unwrap().path =
        repo.to_string_lossy().into_owned();
    // A draft is not implementable: nothing to offer, no scratch.
    let expected = f.expected();
    ts::committed(&f.stage("s1", &expected, 0, "# draft\n"));
    ts::committed(&f.send(
        "p1",
        "assistant_plan",
        f.publication(&expected, "# draft\n"),
    ));
    let implement = |plan_revision| NativeAction::Implement {
        implement: ImplementChoice {
            project: "app".into(),
            plan_revision,
        },
    };
    assert!(select(&f, implement(1)).is_err());
    let expected = f.expected();
    let text = "# Ready\nChange cache.rs.\n";
    ts::committed(&f.stage("s2", &expected, 0, text));
    let mut p = f.publication(&expected, text);
    p["stage"] = json!("ready");
    p["execution_scope"] = json!({"project":"app","paths":["cache.rs"],"actions":["evict"]});
    p["assumptions"] = json!({"project":"app","source_head":head,"base_head":head,
        "files":[{"path":"cache.rs","sha256":null}]});
    ts::committed(&f.send("p2", "assistant_plan", p));

    // A choice naming the earlier revision refuses as stale and opens nothing.
    let error = select(&f, implement(1)).unwrap_err();
    assert!(
        format!("{error:#}").contains("handoff_stale_view"),
        "{error:#}"
    );
    assert!(
        f.fx.ctx
            .store
            .get_assistant(&f.session)
            .unwrap()
            .scratch
            .is_empty()
    );

    let result = select(&f, implement(2)).unwrap();
    assert_eq!(result["ships"], false);
    let after = f.container();
    let w = &after.workflow.as_ref().unwrap().current;
    assert_eq!(w.phase, WorkflowPhase::Executing);
    assert!(w.execution.is_some());
    assert!(
        boundary::require_ordinary(&f.fx.ctx.store, &f.session, &after).is_ok(),
        "a validated grant with its scratch is local authority"
    );
    let scratch = f.fx.ctx.store.get_assistant(&f.session).unwrap().scratch;
    assert_eq!(scratch.len(), 1);
    f.no_tasks();
    // Planning has ended: a second selection has nothing current to consume.
    assert!(select(&f, implement(2)).is_err());
    assert_eq!(
        f.fx.ctx
            .store
            .get_assistant(&f.session)
            .unwrap()
            .scratch
            .len(),
        1
    );
    // Restart: the reopened store keeps the mode and the single grant.
    let reopened = corral_core::Store::open(f.fx._tmp.path()).unwrap();
    let c = reopened.get_assistant_container(&f.session).unwrap();
    assert_eq!(c.workflow.unwrap().current.phase, WorkflowPhase::Executing);
}

/// A ready r2 plan observing `cache.rs` as absent at the fixture repository's HEAD.
fn ready_plan() -> (Fixture, std::path::PathBuf) {
    let f = Fixture::new();
    f.planning();
    let repo = f.fx._tmp.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), ".worktrees/\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "fixture"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let mut f = f;
    f.fx.ctx.store.config.projects.get_mut("app").unwrap().path =
        repo.to_string_lossy().into_owned();
    let expected = f.expected();
    let text = "# Ready\nChange cache.rs.\n";
    ts::committed(&f.stage("s2", &expected, 0, text));
    let mut p = f.publication(&expected, text);
    p["stage"] = json!("ready");
    p["execution_scope"] = json!({"project":"app","paths":["cache.rs"],"actions":["evict"]});
    p["assumptions"] = json!({"project":"app","source_head":head,"base_head":head,
        "files":[{"path":"cache.rs","sha256":null}]});
    ts::committed(&f.send("p2", "assistant_plan", p));
    (f, repo)
}

#[test]
fn implement_here_revalidates_source_and_reconciles_one_granted_scratch_after_interruption() {
    let (f, repo) = ready_plan();
    let implement = NativeAction::Implement {
        implement: ImplementChoice {
            project: "app".into(),
            plan_revision: 1,
        },
    };
    let implement = match implement {
        NativeAction::Implement { mut implement } => {
            implement.plan_revision = f
                .container()
                .workflow
                .unwrap()
                .current
                .current_plan
                .unwrap()
                .revision;
            NativeAction::Implement { implement }
        }
        other => other,
    };
    let scratches = |f: &Fixture| f.fx.ctx.store.get_assistant(&f.session).unwrap().scratch;

    // The plan observed cache.rs absent; it now exists, so the choice refuses unconsumed.
    std::fs::write(repo.join("cache.rs"), b"new\n").unwrap();
    let error = select(&f, implement.clone()).unwrap_err();
    assert!(
        format!("{error:#}").contains("implement_stale_source"),
        "{error:#}"
    );
    assert!(scratches(&f).is_empty());
    assert_eq!(
        f.container().workflow.unwrap().current.phase,
        WorkflowPhase::Planning
    );
    std::fs::remove_file(repo.join("cache.rs")).unwrap();

    // Interrupt after the grant settles: scratch creation fails at the granted identity.
    let blocker = repo.join(".worktrees");
    let hook_path = blocker.clone();
    crate::agent::FAILPOINT.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |name| {
            if name == "native_implement_granted" {
                std::fs::write(&hook_path, b"not a directory\n").unwrap();
            }
        }))
    });
    let interrupted = select(&f, implement.clone());
    crate::agent::FAILPOINT.with(|slot| *slot.borrow_mut() = None);
    assert!(interrupted.is_err(), "{interrupted:?}");
    let c = f.container();
    assert_eq!(
        c.workflow.as_ref().unwrap().current.phase,
        WorkflowPhase::Executing
    );
    assert!(scratches(&f).is_empty(), "no unintended scratch");
    // Without its recorded scratch the grant is not authority, for effects or the host.
    assert!(boundary::require_ordinary(&f.fx.ctx.store, &f.session, &c).is_err());
    let state = read_state(&f.fx.ctx.store, &f.session, &c, 0, 1).unwrap();
    assert_eq!(state["execution_open"], false);
    f.no_tasks();

    // Recovery creates exactly the granted scratch, once.
    std::fs::remove_file(&blocker).unwrap();
    let w = c.workflow.unwrap().current;
    let grant =
        crate::assistant::workflow::handoff::native_implement::grant(&f.fx.ctx.store, &w).unwrap();
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "branch",
            "-D",
            &format!("corral/{}/{}", f.session, grant.scratch),
        ])
        .output();
    let ctx = &f.fx.ctx;
    let first = crate::assistant::workflow::handoff::native_implement::reconcile(ctx, &f.session)
        .unwrap()
        .unwrap();
    let again = crate::assistant::workflow::handoff::native_implement::reconcile(ctx, &f.session)
        .unwrap()
        .unwrap();
    assert_eq!(first.slug, grant.scratch);
    assert_eq!(again.slug, first.slug);
    assert_eq!(scratches(&f).len(), 1);
    let c = f.container();
    assert!(boundary::require_ordinary(&f.fx.ctx.store, &f.session, &c).is_ok());
    assert_eq!(
        read_state(&f.fx.ctx.store, &f.session, &c, 0, 1).unwrap()["execution_open"],
        true
    );
    f.no_tasks();
}
