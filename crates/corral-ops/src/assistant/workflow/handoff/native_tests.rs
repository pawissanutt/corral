use super::native::{NativeAction, authenticate, binding, consume, consumed};
use super::tests::planning;
use crate::agent::test_support as ts;
use corral_core::{
    assistant::{NewOwnerInput, OwnerInputSource},
    blob_ref::BlobHash,
};

fn journal(
    f: &ts::Fx,
    session: &str,
    source: OwnerInputSource,
    text: &str,
    bound: Option<String>,
) -> String {
    f.ctx
        .store
        .update_assistant_container(session, |c| {
            Ok(c.journal_owner_input(
                session,
                NewOwnerInput {
                    generation: c.state.binding.generation,
                    turn: None,
                    source,
                    sha256: BlobHash::of(text.as_bytes()).to_string(),
                    text: text.into(),
                    truncated: false,
                    command: None,
                    answers: None,
                    request: format!("ev-1-{}", c.owner_inputs.len() + 10),
                    binding: bound,
                },
            )
            .id)
        })
        .unwrap()
}

#[test]
fn native_choice_needs_exact_binding_and_is_consumed_once() {
    let mut f = ts::fx();
    let (session, container, _) = planning(&mut f);
    let generation = container.state.binding.generation;
    let shown = super::native_view::view(&f.ctx, &session, &container)
        .unwrap()
        .digest;
    let action = NativeAction::Save {
        task: "T-999".into(),
        contract_revision: "c".into(),
    };
    let (_, digest) = binding(&container, &session, &action, &shown).unwrap();

    // Typed pane text is never transfer consent, even when it names the option.
    let pane = journal(&f, &session, OwnerInputSource::Pane, "save_for_later", None);
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let error = authenticate(&c, &session, generation, &pane, &action, &shown).unwrap_err();
    assert!(
        format!("{error:#}").contains("native keyboard/pointer"),
        "{error:#}"
    );

    // A selection bound to a different action payload is stale, never widened.
    let other = NativeAction::Save {
        task: "T-998".into(),
        contract_revision: "c".into(),
    };
    let (_, other_digest) = binding(&c, &session, &other, &shown).unwrap();
    let stale = journal(
        &f,
        &session,
        OwnerInputSource::Choice,
        "save_for_later",
        Some(other_digest),
    );
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let error = authenticate(&c, &session, generation, &stale, &action, &shown).unwrap_err();
    assert!(
        format!("{error:#}").contains("handoff_stale_choice"),
        "{error:#}"
    );

    // The exact selection authenticates, is consumed durably, and never repeats.
    let chosen = journal(
        &f,
        &session,
        OwnerInputSource::Choice,
        "save_for_later",
        Some(digest),
    );
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let evidence = authenticate(&c, &session, generation, &chosen, &action, &shown).unwrap();
    assert_eq!(evidence.selected_option, "save_for_later");
    // The service refuses (no such task); the choice stays consumed: choose again.
    assert!(consume(&f.ctx, evidence.clone(), action.clone(), &shown).is_err());
    assert_eq!(consumed(&f.ctx, &chosen).unwrap(), Some(evidence.clone()));
    let error = consume(&f.ctx, evidence, action, &shown).unwrap_err();
    assert!(
        format!("{error:#}").contains("handoff_choice_consumed"),
        "{error:#}"
    );
    assert!(f.ctx.store.list().unwrap().is_empty(), "no task created");
}
