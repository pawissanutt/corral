use super::*;
use crate::agent::app_runtime::AuthenticatedDevice;
use corral_protocol::cmd::{Command, config::ConfigCommand};

/// Wire identity and mutable public Caller fields must not mint or alter local app authority.
#[test]
fn wire_app_claims_do_not_grant_owner_or_admin_authority() {
    let caller = Caller {
        principal: Principal::Device {
            id: "device-one".into(),
        },
        client: ClientKind::App,
        actor: Some(ActorName::Owner),
        via: Via::App,
        assistant: None,
        planning_safe: false,
        cwd: None,
        env: CallerEnv::default(),
    };
    let wire: Caller = serde_json::from_slice(&serde_json::to_vec(&caller).unwrap()).unwrap();
    assert!(Invoker::from_caller(wire).device().is_err());
    let device = AuthenticatedDevice {
        id: "device-one".into(),
        name: "phone".into(),
        assistant: true,
        assistant_act: false,
        reply: true,
        act: true,
    };
    let mut web = Invoker::authenticated_device(caller.clone(), device.clone(), false).unwrap();
    let commands = [
        ConfigCommand::Set {
            key: "ui.mouse".into(),
            value: "false".into(),
            expected_hash: "hash".into(),
        },
        ConfigCommand::ProfileAdd {
            name: "clone".into(),
            from: "omp".into(),
            model: None,
            effort: None,
            expected_hash: "hash".into(),
        },
        ConfigCommand::ProfileRemove {
            name: "clone".into(),
            expected_hash: "hash".into(),
        },
    ];
    let native = Invoker::authenticated_device(caller, device, true).unwrap();
    for config in commands.map(Command::Config) {
        assert_eq!(
            crate::command::app_class(&config),
            corral_protocol::app::ActClass::Admin
        );
        assert!(crate::command::policy::admit(&web, &config).is_err());
        assert!(crate::command::policy::admit(&native, &config).is_ok());
    }
    web.caller.principal = Principal::Device {
        id: "another-device".into(),
    };
    assert!(web.device().is_err());
}
