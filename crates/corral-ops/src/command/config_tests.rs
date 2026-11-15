use super::*;
use crate::command::{CmdState, Command, Invoker, execute_in};
use corral_protocol::cmd::{ActorName, Caller, CallerEnv, ClientKind, Principal, Via};

/// An intervening canonical write must win; stale settings may not silently overwrite it.
#[test]
fn stale_settings_hash_refuses_without_overwriting_current_document() {
    let root = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(root.path()).unwrap();
    let hash = config_hash(
        &store
            .config_layers()
            .unwrap()
            .doc(ConfigLayer::Workspace)
            .unwrap(),
    );
    store
        .edit_config(|doc| doc.set_str("ui.done_max", "3"))
        .unwrap();
    let invoker = Invoker::from_caller(Caller {
        principal: Principal::Unverified,
        client: ClientKind::Cli,
        actor: Some(ActorName::Owner),
        via: Via::Cli,
        assistant: None,
        planning_safe: false,
        cwd: Some(root.path().to_string_lossy().into_owned()),
        env: CallerEnv::default(),
    });
    let result = execute_in(
        crate::Ctx::from_store(store.clone()),
        &invoker,
        Command::Config(ConfigCommand::Set {
            key: "ui.done_max".into(),
            value: "4".into(),
            expected_hash: hash,
        }),
    );
    assert_eq!(result.state, CmdState::Conflict);
    assert_eq!(
        store.config_layers().unwrap().config().unwrap().ui.done_max,
        3
    );
}

/// A native settings client may clone/edit/delete, but stale edits and newly used profiles win.
#[test]
fn profile_actions_preserve_stale_edits_and_new_task_references() {
    use crate::agent::app_runtime::AuthenticatedDevice;
    use corral_core::Status;
    let root = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(root.path()).unwrap();
    let invoker = Invoker::authenticated_device(
        Caller {
            principal: Principal::Device {
                id: "native-admin".into(),
            },
            client: ClientKind::App,
            actor: Some(ActorName::Owner),
            via: Via::App,
            assistant: None,
            planning_safe: false,
            cwd: None,
            env: CallerEnv::default(),
        },
        AuthenticatedDevice {
            id: "native-admin".into(),
            name: "native".into(),
            assistant: true,
            assistant_act: false,
            reply: true,
            act: true,
        },
        true,
    )
    .unwrap();
    let hash = || {
        config_hash(
            &store
                .config_layers()
                .unwrap()
                .doc(ConfigLayer::Workspace)
                .unwrap(),
        )
    };
    let run = |command| {
        execute_in(
            crate::Ctx::from_store(store.clone()),
            &invoker,
            Command::Config(command),
        )
    };
    let before = hash();
    let source = store.config_layers().unwrap().config().unwrap().profiles["omp"].clone();
    assert_eq!(
        run(ConfigCommand::ProfileAdd {
            name: "app-clone".into(),
            from: "omp".into(),
            model: Some("first-model".into()),
            effort: None,
            expected_hash: before.clone(),
        })
        .state,
        CmdState::Done
    );
    let config = store.config_layers().unwrap().config().unwrap();
    assert_eq!(config.profiles["app-clone"].harness, source.harness);
    assert_eq!(config.profiles["app-clone"].args, source.args);
    assert_eq!(
        config.profiles["app-clone"].model.as_deref(),
        Some("first-model")
    );
    assert_eq!(
        run(ConfigCommand::ProfileRemove {
            name: "app-clone".into(),
            expected_hash: before,
        })
        .state,
        CmdState::Conflict
    );
    assert_eq!(
        run(ConfigCommand::Set {
            key: "profiles.app-clone.model".into(),
            value: "second-model".into(),
            expected_hash: hash(),
        })
        .state,
        CmdState::Done
    );
    let layers = store.config_layers().unwrap();
    let config = layers.config().unwrap();
    let choice = super::super::profile_choices(&store, &layers, &config)
        .unwrap()
        .into_iter()
        .find(|profile| profile.name == "app-clone")
        .unwrap();
    assert_eq!(choice.model.as_deref(), Some("second-model"));
    assert!(choice.workspace && !choice.inherited && choice.remove_refusal.is_none());
    let settings_hash = hash();
    let task = store
        .create_initialized("profile consumer", None, Status::Backlog, "", |task| {
            task.profile = Some("app-clone".into());
            Ok(())
        })
        .unwrap();
    let refused = run(ConfigCommand::ProfileRemove {
        name: "app-clone".into(),
        expected_hash: settings_hash.clone(),
    });
    assert_eq!(refused.error.unwrap().code, "profile_in_use");
    assert_eq!(hash(), settings_hash);
    assert!(
        store
            .config_layers()
            .unwrap()
            .config()
            .unwrap()
            .profiles
            .contains_key("app-clone")
    );
    store
        .update(&task.id, |task| {
            task.profile = None;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        run(ConfigCommand::ProfileRemove {
            name: "app-clone".into(),
            expected_hash: hash(),
        })
        .state,
        CmdState::Done
    );
    assert!(
        !store
            .config_layers()
            .unwrap()
            .config()
            .unwrap()
            .profiles
            .contains_key("app-clone")
    );
}

#[test]
fn settings_can_select_and_clear_summary_profile_without_removing_a_referenced_profile() {
    use crate::agent::app_runtime::AuthenticatedDevice;
    let root = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(root.path()).unwrap();
    let invoker = Invoker::authenticated_device(
        Caller {
            principal: Principal::Device {
                id: "native-admin".into(),
            },
            client: ClientKind::App,
            actor: Some(ActorName::Owner),
            via: Via::App,
            assistant: None,
            planning_safe: false,
            cwd: None,
            env: CallerEnv::default(),
        },
        AuthenticatedDevice {
            id: "native-admin".into(),
            name: "native".into(),
            assistant: true,
            assistant_act: false,
            reply: true,
            act: true,
        },
        true,
    )
    .unwrap();
    let hash = || {
        config_hash(
            &store
                .config_layers()
                .unwrap()
                .doc(ConfigLayer::Workspace)
                .unwrap(),
        )
    };
    let set = |value: &str| {
        execute_in(
            crate::Ctx::from_store(store.clone()),
            &invoker,
            Command::Config(ConfigCommand::Set {
                key: "assistant.summary_profile".into(),
                value: value.into(),
                expected_hash: hash(),
            }),
        )
    };
    assert_eq!(set("omp").state, CmdState::Done);
    let ctx = crate::Ctx::from_store(store.clone());
    assert_eq!(
        crate::app_read::settings_rows(&ctx).unwrap()[0]
            .summary_profile
            .as_deref(),
        Some("omp")
    );
    let layers = store.config_layers().unwrap();
    let config = layers.config().unwrap();
    let profile = super::super::profile_choices(&store, &layers, &config)
        .unwrap()
        .into_iter()
        .find(|profile| profile.name == "omp")
        .unwrap();
    assert!(
        profile
            .references
            .contains(&"assistant.summary_profile".to_owned())
    );
    assert!(profile.remove_refusal.is_some());
    assert_eq!(set("").state, CmdState::Done);
    assert_eq!(
        store
            .config_layers()
            .unwrap()
            .config()
            .unwrap()
            .assistant
            .summary_profile,
        None
    );
}
