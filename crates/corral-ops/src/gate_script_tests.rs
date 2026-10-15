use super::*;
use std::process::{Command, Stdio};

#[test]
fn script_executes_receipted_environment_instead_of_pane_environment() {
    // Given a captured environment with quoting and an explicitly absent variable.
    let tmp = tempfile::tempdir().unwrap();
    let paths = GatePaths {
        script: tmp.path().join("gate.sh"),
        result: tmp.path().join("gate.json"),
        failed: tmp.path().join("gate.failed"),
        log: tmp.path().join("gate.log"),
    };
    let path = format!("/captured-only:{}", std::env::var("PATH").unwrap());
    let rustc = "compiler with 'quotes'\nand a newline";
    let identity = CandidateIdentity {
        version: 2,
        task: "T-001".into(),
        run: "R1".into(),
        attempt: "attempt".into(),
        head: "head".into(),
        tree: "tree".into(),
        integration_base: "base".into(),
        dispatch_base: "base".into(),
        spec_digest: "spec".into(),
        policy_digest: "policy".into(),
        inputs_digest: "inputs".into(),
        environment: vec![
            ("PATH".into(), Some(path.clone())),
            ("RUSTC".into(), Some(rustc.into())),
            ("RUSTFLAGS".into(), None),
        ],
    };
    let commands =
        vec![r#"printf '%s\0%s\0%s' "$PATH" "$RUSTC" "${RUSTFLAGS+set}" > observed"#.into()];
    std::fs::write(paths.result.with_extension("owner"), "private-owner").unwrap();
    std::fs::write(
        &paths.script,
        render(&identity, tmp.path(), &commands, &paths).unwrap(),
    )
    .unwrap();
    // When a different pane environment executes the already-rendered script.
    let output = Command::new("/bin/bash")
        .arg(&paths.script)
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("RUSTC", "pane-compiler")
        .env("RUSTFLAGS", "pane-flags")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    // Then executed values and the machine receipt both match the captured inputs.
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read(tmp.path().join("observed")).unwrap(),
        format!("{path}\0{rustc}\0").as_bytes()
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&paths.result).unwrap()).unwrap();
    assert_eq!(
        serde_json::from_value::<CandidateIdentity>(receipt["identity"].clone()).unwrap(),
        identity
    );
    assert_eq!(receipt["result"], "pass");
}
