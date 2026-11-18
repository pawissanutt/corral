use super::native::{NativeAction, authenticate, binding, consume};
use super::native_local::LocalChoice;
use super::{Goal, context::ContextMode, git, tests};
use crate::agent::test_support as ts;
use corral_core::{
    assistant::{NewOwnerInput, OwnerInputSource},
    blob_ref::BlobHash,
};

fn shown(f: &ts::Fx, session: &str) -> String {
    let c = f.ctx.store.get_assistant_container(session).unwrap();
    super::native_view::view(&f.ctx, session, &c)
        .unwrap()
        .digest
}

fn choose(f: &ts::Fx, session: &str, action: &NativeAction, shown: &str) -> String {
    let c = f.ctx.store.get_assistant_container(session).unwrap();
    let (_, digest) = binding(&c, session, action, shown).unwrap();
    let text = action.option().unwrap();
    f.ctx
        .store
        .update_assistant_container(session, |c| {
            Ok(c.journal_owner_input(
                session,
                NewOwnerInput {
                    generation: c.state.binding.generation,
                    turn: None,
                    source: OwnerInputSource::Choice,
                    sha256: BlobHash::of(text.as_bytes()).to_string(),
                    text,
                    truncated: false,
                    command: None,
                    answers: None,
                    request: format!("ev-1-{}", c.owner_inputs.len() + 20),
                    binding: Some(digest),
                },
            )
            .id)
        })
        .unwrap()
}

fn local(entry: Option<String>, context: ContextMode) -> LocalChoice {
    LocalChoice {
        project: "app".into(),
        goal: Goal::ImplementationPrepare,
        context,
        save: true,
        title: "Preserve local work".into(),
        entry,
        history: None,
    }
}

#[test]
fn native_local_entry_without_scope_refuses_before_entering_a_workflow() {
    let f = ts::fx();
    let session = f
        .ctx
        .store
        .create_assistant("pi", None, None, None)
        .unwrap()
        .id;
    let entry = f
        .ctx
        .store
        .update_assistant_container(&session, |c| {
            let text = "fix the cache";
            Ok(c.journal_owner_input(
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
                    binding: None,
                },
            )
            .id)
        })
        .unwrap();
    let action = NativeAction::Local {
        local: local(Some(entry), ContextMode::Task),
    };
    let input = choose(&f, &session, &action, &shown(&f, &session));
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let generation = c.state.binding.generation;
    let evidence = authenticate(
        &c,
        &session,
        generation,
        &input,
        &action,
        &shown(&f, &session),
    )
    .unwrap();
    let error = consume(&f.ctx, evidence, action, &shown(&f, &session)).unwrap_err();
    assert!(format!("{error:#}").contains("handoff_scope"), "{error:#}");
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    assert!(c.workflow.is_none(), "a refused entry leaves no workflow");
    assert!(f.ctx.store.list().unwrap().is_empty());
}

#[test]
fn native_local_save_commits_one_held_task_through_the_shared_transfer() {
    let mut f = ts::fx();
    let (session, _, _) = tests::planning(&mut f);
    let source = f._tmp.path().join("source");
    let base = tests::repo(&source);
    // Snapshots are fetched into the registered project repository.
    f.ctx.store.config.projects.insert(
        "app".into(),
        serde_json::from_value(serde_json::json!({"path":"source"})).unwrap(),
    );
    f.ctx.store.save_config().unwrap();
    tests::record_source(&f, &session, &source, &base);
    std::fs::write(source.join("file"), b"changed\n").unwrap();
    git::git(&source, &["add", "file"], None).unwrap();

    // Conversation context without its selected range refuses before consuming.
    let conversation = NativeAction::Local {
        local: local(None, ContextMode::Conversation),
    };
    let input = choose(&f, &session, &conversation, &shown(&f, &session));
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let generation = c.state.binding.generation;
    let evidence = authenticate(
        &c,
        &session,
        generation,
        &input,
        &conversation,
        &shown(&f, &session),
    )
    .unwrap();
    let error = consume(&f.ctx, evidence, conversation, &shown(&f, &session)).unwrap_err();
    assert!(
        format!("{error:#}").contains("context_incomplete"),
        "{error:#}"
    );
    assert!(super::native::consumed(&f.ctx, &input).unwrap().is_none());

    // A source change after the sheet showed it refuses as stale and consumes nothing.
    let stale = NativeAction::Local {
        local: local(None, ContextMode::Task),
    };
    let seen = shown(&f, &session);
    let input = choose(&f, &session, &stale, &seen);
    std::fs::write(source.join("file"), b"changed after display\n").unwrap();
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let evidence = authenticate(&c, &session, generation, &input, &stale, &seen).unwrap();
    let error = consume(&f.ctx, evidence, stale, &seen).unwrap_err();
    assert!(
        format!("{error:#}").contains("handoff_stale_view"),
        "{error:#}"
    );
    assert!(super::native::consumed(&f.ctx, &input).unwrap().is_none());
    assert!(f.ctx.store.list().unwrap().is_empty());

    let action = NativeAction::Local {
        local: local(None, ContextMode::Task),
    };
    let input = choose(&f, &session, &action, &shown(&f, &session));
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let evidence = authenticate(
        &c,
        &session,
        generation,
        &input,
        &action,
        &shown(&f, &session),
    )
    .unwrap();
    let result = consume(
        &f.ctx,
        evidence.clone(),
        action.clone(),
        &shown(&f, &session),
    )
    .unwrap();
    let tasks = f.ctx.store.list().unwrap();
    assert_eq!(tasks.len(), 1, "{result}");
    assert!(tasks[0].dispatch_hold && tasks[0].runs.is_empty());
    let error = consume(&f.ctx, evidence, action, &shown(&f, &session)).unwrap_err();
    assert!(format!("{error:#}").contains("handoff_choice_consumed"));
    assert_eq!(f.ctx.store.list().unwrap().len(), 1, "no second task");
}

#[test]
fn displayed_source_pins_the_index_layer_and_refuses_unsafe_untracked_entries() {
    let mut f = ts::fx();
    let (session, _, _) = tests::planning(&mut f);
    let source = f._tmp.path().join("source");
    let base = tests::repo(&source);
    tests::record_source(&f, &session, &source, &base);
    std::fs::write(source.join("file"), b"changed\n").unwrap();
    let unstaged = shown(&f, &session);
    // Staging changes only the index: the net working-tree diff against base is identical.
    git::git(&source, &["add", "file"], None).unwrap();
    assert_ne!(
        shown(&f, &session),
        unstaged,
        "stage-only change must be visible"
    );

    // An untracked symlink is never followed; the read refuses with a named cause.
    let outside = f._tmp.path().join("outside");
    std::fs::write(&outside, b"secret\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, source.join("link")).unwrap();
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let error = match super::native_view::view(&f.ctx, &session, &c) {
        Ok(_) => panic!("symlink was fingerprinted"),
        Err(e) => format!("{e:#}"),
    };
    assert!(error.contains("handoff_source_unsupported"), "{error}");
}

#[test]
fn native_preview_shows_the_same_operational_facts_as_the_committed_package() {
    let mut f = ts::fx();
    let (session, _, _) = tests::planning(&mut f);
    let source = f._tmp.path().join("source");
    let base = tests::repo(&source);
    f.ctx.store.config.projects.insert(
        "app".into(),
        serde_json::from_value(serde_json::json!({"path":"source"})).unwrap(),
    );
    f.ctx.store.save_config().unwrap();
    tests::record_source(&f, &session, &source, &base);
    std::fs::write(source.join("file"), b"changed\n").unwrap();
    git::git(&source, &["add", "file"], None).unwrap();
    let choice = local(None, ContextMode::Task);
    let shown_preview = super::native_local::preview(&f.ctx, &session, &choice).unwrap();
    let action = NativeAction::Local { local: choice };
    let seen = shown(&f, &session);
    let input = choose(&f, &session, &action, &seen);
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let generation = c.state.binding.generation;
    let evidence = authenticate(&c, &session, generation, &input, &action, &seen).unwrap();
    let result = consume(&f.ctx, evidence, action, &seen).unwrap();
    let (_, _, packaged) =
        super::read_package(&f.ctx.store, result["package"].as_str().unwrap()).unwrap();
    // Every operational fact matches; only the bundle hash is unknown before capture.
    let facts = |prompt: &str| -> Vec<String> {
        prompt
            .lines()
            .take_while(|l| !l.is_empty())
            .map(|l| l.split("; bundle ").next().unwrap().to_owned())
            .collect()
    };
    let native = facts(shown_preview["prompt"].as_str().unwrap());
    assert!(native.iter().any(|l| l.starts_with("Plan: ")), "{native:?}");
    assert_eq!(native, facts(&packaged.prompt));
}
