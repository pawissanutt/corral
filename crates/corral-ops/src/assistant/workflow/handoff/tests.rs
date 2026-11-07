use super::*;
use crate::agent::test_support as ts;
use context::{Brief, ContextMode, HistorySelection, VisibleMessage, VisibleRole};
use corral_core::{
    assistant::{NewOwnerInput, OwnerInputSource},
    blob_ref::BlobHash,
};
use git::git;

fn context(mode: ContextMode) -> Context {
    Context {
        mode,
        brief: Brief {
            goal: "repair cache".into(),
            constraints: vec!["local only".into()],
            decisions: vec!["keep format".into()],
            completed: vec!["reproduced".into()],
            remaining: vec!["fix invalidation".into()],
            findings: vec!["cache stale".into()],
            failed_approaches: vec!["TTL alone".into()],
            assumptions: vec!["one writer".into()],
            evidence: vec!["evidence/failure".into()],
            questions: vec!["which bound?".into()],
            mandatory_blockers: vec!["required review finding F1".into()],
            next_action: "repair F1".into(),
        },
        history: None,
    }
}

#[test]
fn handoff_context_modes_select_only_visible_history_and_disclose_paging() {
    let task = context(ContextMode::Task);
    let p = task.preview().unwrap();
    for fact in [
        "repair cache",
        "keep format",
        "reproduced",
        "cache stale",
        "repair F1",
    ] {
        assert!(p.prompt.contains(fact));
    }
    let minimal = context(ContextMode::Minimal).preview().unwrap();
    assert!(minimal.prompt.contains("required review finding F1"));
    assert!(!minimal.prompt.contains("TTL alone"));
    let mut conversation = context(ContextMode::Conversation);
    conversation.history = Some(HistorySelection {
        branch: "branch-a".into(),
        checkpoint: "checkpoint-7".into(),
        messages: vec![
            VisibleMessage {
                ordinal: 4,
                role: VisibleRole::User,
                text: "selected visible text".into(),
            },
            VisibleMessage {
                ordinal: 5,
                role: VisibleRole::Assistant,
                text: "x".repeat(context::PROMPT_MAX),
            },
        ],
        excluded: vec!["messages 1-3, other branches, private reasoning, tool invocations".into()],
    });
    let p = conversation.preview().unwrap();
    assert_eq!(p.range, Some((4, 5)));
    assert_eq!(p.loaded_messages, 1);
    assert_eq!(p.archived_messages, 2);
    assert!(!p.native_fork);
    assert!(p.archived_bytes > p.loaded_bytes);
    assert!(!p.prompt.contains(&"x".repeat(100)));
    let mut invalid = serde_json::to_value(&conversation).unwrap();
    invalid["history"]["messages"][0]["credentials"] = serde_json::json!({"token":"secret"});
    assert!(serde_json::from_value::<Context>(invalid).is_err());
    conversation.history.as_mut().unwrap().messages[1].ordinal = 6;
    assert!(
        conversation
            .preview()
            .unwrap_err()
            .to_string()
            .contains("context_range")
    );
    assert!(task.history.is_none());
}

fn repo(path: &Path) -> String {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "--template=", "-b", "work"], None).unwrap();
    std::fs::write(path.join("file"), b"base\r\n").unwrap();
    std::fs::write(path.join("deleted"), b"delete me").unwrap();
    std::fs::write(path.join(".gitignore"), b"ignored\n").unwrap();
    git(path, &["add", "."], None).unwrap();
    git(path, &["commit", "-m", "base"], None).unwrap();
    String::from_utf8(git(path, &["rev-parse", "HEAD"], None).unwrap())
        .unwrap()
        .trim()
        .into()
}

#[test]
fn handoff_snapshot_restores_commits_raw_index_binary_work_and_untracked_without_source_writes() {
    let f = ts::fx();
    let store = &f.ctx.store;
    let source = f._tmp.path().join("source");
    let base = repo(&source);
    std::fs::write(source.join("committed"), b"a commit\0\xff").unwrap();
    git(&source, &["add", "."], None).unwrap();
    git(&source, &["commit", "-m", "work"], None).unwrap();
    std::fs::write(source.join("file"), b"staged\0\xff\r\n").unwrap();
    std::fs::write(source.join("staged-only"), b"index addition\r\n").unwrap();
    git(&source, &["add", "."], None).unwrap();
    std::fs::write(source.join("file"), b"unstaged\0\xfe\r\n").unwrap();
    std::fs::remove_file(source.join("staged-only")).unwrap();
    std::fs::remove_file(source.join("deleted")).unwrap();
    std::fs::write(source.join("untracked"), b"raw bytes\r\n\0\xff").unwrap();
    std::fs::write(source.join("ignored"), b"not transferred").unwrap();
    // An inherited credential/retargeting configuration is not copied into the continuation.
    git(
        &source,
        &["config", "credential.helper", "do-not-copy"],
        None,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            source.join("untracked"),
            std::fs::Permissions::from_mode(0o751),
        )
        .unwrap();
    }
    // HEAD can move below the branch tip without discarding its commits or index layers.
    git(&source, &["update-ref", "--no-deref", "HEAD", &base], None).unwrap();
    let before = git::scan(&source, &base, "work").unwrap().state;
    let snapshot = Snapshot::capture(store, &source, &base, "work").unwrap();
    assert_eq!(snapshot.state, before);
    let target = f._tmp.path().join("target");
    snapshot.restore_new(store, &target).unwrap();
    assert_eq!(git::scan(&target, &base, "work").unwrap().state, before);
    assert_eq!(git::scan(&source, &base, "work").unwrap().state, before);
    assert!(!target.join("ignored").exists());
    assert_eq!(
        std::fs::read(source.join("ignored")).unwrap(),
        b"not transferred"
    );
    assert!(snapshot.excluded.contains(&"ignored".to_owned()));
    assert!(git(&target, &["config", "--get", "credential.helper"], None).is_err());
    assert_eq!(
        std::fs::read(target.join("untracked")).unwrap(),
        b"raw bytes\r\n\0\xff"
    );
    std::fs::write(source.join("untracked"), "changed after preview").unwrap();
    assert!(
        snapshot
            .validate_source(&source)
            .unwrap_err()
            .to_string()
            .contains("snapshot_changed")
    );
    assert!(
        snapshot
            .restore_new(store, &target)
            .unwrap_err()
            .to_string()
            .contains("destination_exists")
    );
}

#[cfg(unix)]
#[test]
fn handoff_snapshot_refuses_named_oversized_missing_special_and_changing_content() {
    let f = ts::fx();
    let source = f._tmp.path().join("source");
    let base = repo(&source);
    let huge = std::fs::File::create(source.join("huge")).unwrap();
    huge.set_len(content::FILE_MAX as u64 + 1).unwrap();
    let error = Snapshot::capture(&f.ctx.store, &source, &base, "work")
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("snapshot_oversized") && error.contains("huge"),
        "{error}"
    );
    std::fs::remove_file(source.join("huge")).unwrap();
    std::os::unix::fs::symlink("/dev/zero", source.join("special")).unwrap();
    let error = Snapshot::capture(&f.ctx.store, &source, &base, "work")
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("special"));
    std::fs::remove_file(source.join("special")).unwrap();
    let snapshot = Snapshot::capture(&f.ctx.store, &source, &base, "work").unwrap();
    let mut broken = snapshot.clone();
    broken.bundle.blob = BlobHash::of(b"missing required bundle").to_ref();
    let target = f._tmp.path().join("target");
    assert!(
        broken
            .restore_new(&f.ctx.store, &target)
            .unwrap_err()
            .to_string()
            .contains("snapshot_missing")
    );
    assert!(!target.exists());
    let path = source.join("file");
    crate::agent::FAILPOINT.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move |point| {
            if point == "handoff_snapshot_captured" {
                std::fs::write(&path, "concurrent edit").unwrap();
            }
        }))
    });
    let result = Snapshot::capture(&f.ctx.store, &source, &base, "work");
    crate::agent::FAILPOINT.with_borrow_mut(|hook| *hook = None);
    assert!(result.unwrap_err().to_string().contains("snapshot_changed"));
    // A resolved-but-uncommitted merge has no unmerged index rows. Its administrative
    // state must not vanish into an apparently complete code-only checkpoint.
    std::fs::write(source.join(".git/MERGE_HEAD"), format!("{base}\n")).unwrap();
    let active_merge = Snapshot::capture(&f.ctx.store, &source, &base, "work");
    assert!(
        active_merge.is_err(),
        "merge administration was silently omitted"
    );
    let error = active_merge.unwrap_err().to_string();
    assert!(
        error.contains("snapshot_unsupported") && error.contains("MERGE_HEAD"),
        "{error}"
    );
    assert!(source.join(".git/MERGE_HEAD").is_file());
    assert!(f.ctx.store.list().unwrap().is_empty());
}

pub(super) fn planning(f: &mut ts::Fx) -> (String, AssistantContainer, Expected) {
    f.ctx.store.config.projects.insert(
        "app".into(),
        serde_json::from_value(serde_json::json!({"path":"."})).unwrap(),
    );
    let session = f
        .ctx
        .store
        .create_assistant("pi", None, Some("fixture"), None)
        .unwrap()
        .id;
    f.ctx
        .store
        .update_assistant_container(&session, |c| {
            let text = "plan cache repair";
            let input = c.journal_owner_input(
                &session,
                NewOwnerInput {
                    generation: c.state.binding.generation,
                    turn: None,
                    source: OwnerInputSource::Pane,
                    sha256: BlobHash::of(text.as_bytes()).to_string(),
                    text: text.into(),
                    truncated: false,
                    command: None,
                    answers: None,
                    request: "ev-1-1".into(),
                },
            );
            super::super::prepare_entry(&f.ctx.store, &session, c, &input.id)?;
            c.workflow.as_mut().unwrap().current.phase = WorkflowPhase::Planning;
            Ok(())
        })
        .unwrap();
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let w = &c.workflow.as_ref().unwrap().current;
    let expected = Expected {
        workflow: w.id.clone(),
        state_revision: w.state_revision,
        authority_epoch: w.authority_epoch,
        plan_revision: None,
    };
    (session, c, expected)
}

pub(super) fn request() -> Request {
    Request {
        project: "app".into(),
        scope: vec!["file".into()],
        goal: Goal::PlanContinue,
        context: context(ContextMode::Task),
    }
}

#[test]
fn handoff_preview_reservation_readback_is_idempotent_and_keeps_local_source_ownership() {
    let mut f = ts::fx();
    let (session, container, expected) = planning(&mut f);
    let prepared = prepare(
        &f.ctx.store,
        &session,
        &container,
        &expected,
        request(),
        None,
    )
    .unwrap();
    assert_eq!(
        f.ctx.store.get_assistant_container(&session).unwrap(),
        container,
        "preview has no ownership effect"
    );
    let id = prepared.journal.id.clone();
    let loaded = read_package(&f.ctx.store, &id).unwrap();
    assert_eq!(loaded.1, prepared.package);
    assert_eq!(loaded.2, prepared.preview);
    let reserve_once = || {
        f.ctx
            .store
            .update_assistant_container(&session, |c| reserve(&f.ctx.store, &session, c, &prepared))
    };
    let first = reserve_once().unwrap();
    let after = f.ctx.store.get_assistant_container(&session).unwrap();
    assert_eq!(reserve_once().unwrap(), first);
    assert_eq!(
        after,
        f.ctx.store.get_assistant_container(&session).unwrap()
    );
    let w = &after.workflow.as_ref().unwrap().current;
    assert_eq!(w.owner, ScopeOwner::Local);
    assert_eq!(w.phase, WorkflowPhase::HandoffPreparing);
    assert!(first.reserved && first.source_retained);
    assert!(super::super::boundary::require_ordinary(&f.ctx.store, &session, &after).is_err());
    assert_eq!(read_package(&f.ctx.store, &id).unwrap().0, first);
    let recovered = recover(&f.ctx.store, &id).unwrap();
    assert_eq!(recovered.id(), id);
    assert_eq!(recovered.package(), prepared.package());
    assert!(f.ctx.store.list().unwrap().is_empty());
}

#[test]
fn handoff_plan_stage_artifacts_and_late_findings_remain_exact_without_importing_approval() {
    for stage in [None, Some(false), Some(true)] {
        let mut f = ts::fx();
        let (session, _, _) = planning(&mut f);
        f.ctx
            .store
            .update_assistant_container(&session, |c| {
                let w = &mut c.workflow.as_mut().unwrap().current;
                if let Some(reviewed) = stage {
                    let artifact = super::super::publication::artifact(
                        &f.ctx.store,
                        &session,
                        "W1",
                        b"exact draft\r\n",
                    )?;
                    w.current_plan = Some(PlanRevision {
                        revision: 1,
                        artifact,
                        stage: corral_core::assistant_workflow::DesignStage::Draft,
                        assumptions: None,
                        execution_scope: None,
                        published_at: corral_core::clock::now(),
                        supersedes: None,
                        scope_changes: String::new(),
                    });
                    if reviewed {
                        w.reviews.push(super::super::publication::artifact(
                            &f.ctx.store,
                            &session,
                            "W1",
                            b"r1 findings: unresolved; advisory only",
                        )?);
                    }
                }
                Ok(())
            })
            .unwrap();
        let c = f.ctx.store.get_assistant_container(&session).unwrap();
        let w = &c.workflow.as_ref().unwrap().current;
        let e = Expected {
            workflow: w.id.clone(),
            state_revision: w.state_revision,
            authority_epoch: w.authority_epoch,
            plan_revision: w.current_plan.as_ref().map(|p| p.revision),
        };
        let p = prepare(&f.ctx.store, &session, &c, &e, request(), None).unwrap();
        let (_, loaded, _) = read_package(&f.ctx.store, p.id()).unwrap();
        assert_eq!(loaded.plan, w.current_plan);
        assert_eq!(loaded.reviews, w.reviews);
        f.ctx
            .store
            .update_assistant_container(&session, |c| {
                c.workflow.as_mut().unwrap().current.state_revision += 1;
                Ok(())
            })
            .unwrap();
        assert!(
            f.ctx
                .store
                .update_assistant_container(&session, |c| reserve(&f.ctx.store, &session, c, &p))
                .is_err(),
            "late results must refresh selection"
        );
        assert!(f.ctx.store.list().unwrap().is_empty());
    }
}

#[test]
fn handoff_code_presence_and_retention_cannot_be_silently_bypassed() {
    let mut f = ts::fx();
    let (session, c, expected) = planning(&mut f);
    let source = f._tmp.path().join("source");
    let base = repo(&source);
    f.ctx
        .store
        .update_assistant(&session, |s| {
            s.scratch.push(corral_core::assistant::AssistantScratch {
                slug: "work".into(),
                project: "app".into(),
                path: source.clone(),
                branch: "work".into(),
                base: base.clone(),
                created: corral_core::clock::now(),
                kind: "spike".into(),
                bytes: None,
                measured_at: None,
            });
            Ok(())
        })
        .unwrap();
    assert!(
        prepare(&f.ctx.store, &session, &c, &expected, request(), None)
            .err()
            .unwrap()
            .to_string()
            .contains("handoff_missing")
    );
    let mut r = request();
    r.context.mode = ContextMode::Minimal;
    let p = prepare(
        &f.ctx.store,
        &session,
        &c,
        &expected,
        r,
        Some((&source, &base, "work")),
    )
    .unwrap();
    assert!(
        p.package().snapshot.is_some(),
        "minimal cannot discard code"
    );
    f.ctx
        .store
        .update_assistant_container(&session, |c| reserve(&f.ctx.store, &session, c, &p))
        .unwrap();
    assert!(
        require_source_releasable(&f.ctx.store, &session, &source)
            .unwrap_err()
            .to_string()
            .contains("handoff_source_retained")
    );
    assert!(source.join("file").is_file());
    let recovered = recover(&f.ctx.store, p.id()).unwrap();
    assert_eq!(recovered.package(), p.package());
    assert!(f.ctx.store.list().unwrap().is_empty());
}
