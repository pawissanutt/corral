use std::collections::BTreeMap;

use corral_core::{ConfigDoc, HookIdentity, PostLandHook, Store};

fn hook() -> PostLandHook {
    PostLandHook {
        name: "check_1-ok".into(),
        argv: vec!["/usr/bin/true".into(), "literal ; argument".into()],
        cwd: ".".into(),
        env: BTreeMap::new(),
        timeout_secs: 1,
        max_attempts: 1,
        identity: None,
    }
}

#[test]
fn hooks_are_opt_in_and_round_trip_complete_policy() {
    // Given an old config and a complete ordered hook policy.
    let old = ConfigDoc::parse("[projects.app]\npath = \".\"\n[agents.omo]\ncommand = \"omo\"\n")
        .unwrap()
        .config()
        .unwrap();
    assert!(old.projects["app"].post_land.is_empty());
    let mut config = old;
    let mut install = hook();
    install.name = "install".into();
    install.cwd = "tools/bin".into();
    install.timeout_secs = 86_400;
    install.max_attempts = 10;
    install
        .env
        .insert("PREFIX".into(), "/isolated prefix".into());
    install.identity = Some(HookIdentity {
        binary: "/isolated prefix/corral".into(),
        args: vec!["--build-sha".into()],
        expected_stdout: "{sha}".into(),
    });
    config.projects.get_mut("app").unwrap().post_land = vec![hook(), install];
    // When serialized and parsed through the public config surface.
    let parsed = ConfigDoc::parse(&config.to_toml())
        .unwrap()
        .config()
        .unwrap();
    // Then every machine-consumed field and the ordering survive.
    assert_eq!(
        parsed.projects["app"].post_land,
        config.projects["app"].post_land
    );
    let root = tempfile::tempdir().unwrap();
    parsed.validate(root.path()).unwrap();
}

#[test]
fn invalid_hook_policies_are_rejected_without_writing_config() {
    // Given valid config on disk and one invalid policy per case.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let before = std::fs::read(store.config_path()).unwrap();
    let mut cases = Vec::new();
    for name in ["", "has space", "../escape", "dot.name", "nonascii-\u{e9}"] {
        let mut h = hook();
        h.name = name.into();
        cases.push(vec![h]);
    }
    cases.push(vec![hook(), hook()]);
    for timeout in [0, 86_401] {
        let mut h = hook();
        h.timeout_secs = timeout;
        cases.push(vec![h]);
    }
    for cwd in ["", "/absolute", "../outside", "tools/../../outside"] {
        let mut h = hook();
        h.cwd = cwd.into();
        cases.push(vec![h]);
    }
    for attempts in [0, 11] {
        let mut h = hook();
        h.max_attempts = attempts;
        cases.push(vec![h]);
    }
    for argv in [vec![], vec!["".into()], vec!["bad\0exe".into()]] {
        let mut h = hook();
        h.argv = argv;
        cases.push(vec![h]);
    }
    for (key, value) in [
        ("", "x"),
        ("BAD=KEY", "x"),
        ("BAD\0KEY", "x"),
        ("KEY", "bad\0value"),
        ("CORRAL_LANDED_SHA", "forged"),
    ] {
        let mut h = hook();
        h.env.insert(key.into(), value.into());
        cases.push(vec![h]);
    }
    for identity in [
        HookIdentity {
            binary: "/bin/tool".into(),
            args: vec![],
            expected_stdout: "static-version".into(),
        },
        HookIdentity {
            binary: "".into(),
            args: vec![],
            expected_stdout: "{sha}".into(),
        },
        HookIdentity {
            binary: "/bin/tool".into(),
            args: vec![],
            expected_stdout: "".into(),
        },
        HookIdentity {
            binary: "/bin/tool".into(),
            args: vec!["bad\0arg".into()],
            expected_stdout: "{sha}".into(),
        },
    ] {
        let mut h = hook();
        h.identity = Some(identity);
        cases.push(vec![h]);
    }
    for hooks in cases {
        // When an invalid policy is saved through the transactional editor.
        let result = store.edit_config(|doc| {
            let mut cfg = doc.config()?;
            let mut project = ConfigDoc::parse("[projects.app]\npath = \".\"\n")?
                .config()?
                .projects
                .remove("app")
                .unwrap();
            project.post_land = hooks.clone();
            cfg.projects.insert("app".into(), project);
            doc.sync_from(&cfg)
        });
        // Then validation rejects it without modifying disk.
        assert!(result.is_err(), "accepted {hooks:?}");
        assert_eq!(std::fs::read(store.config_path()).unwrap(), before);
    }
}
