use super::*;
use std::process::{Command, Stdio};

#[test]
fn native_generated_gate_preserves_captured_inputs_and_receipts() {
    // Given a private gate with captured values different from its pane environment.
    let tmp = tempfile::tempdir().unwrap();
    let paths = GatePaths {
        script: tmp.path().join("gate with spaces.sh"),
        result: tmp.path().join("gate.json"),
        failed: tmp.path().join("gate.failed"),
        log: tmp.path().join("gate.log"),
    };
    let literal = "value with 'quotes'\nand a newline";
    let identity = CandidateIdentity {
        version: 2,
        task: "T-001".into(),
        run: "R1".into(),
        attempt: "native-attempt".into(),
        head: "head".into(),
        tree: "tree".into(),
        integration_base: "base".into(),
        dispatch_base: "base".into(),
        spec_digest: "spec".into(),
        policy_digest: "policy".into(),
        inputs_digest: "inputs".into(),
        environment: vec![
            ("PATH".into(), Some(std::env::var("PATH").unwrap())),
            ("CORRAL_GATE_LITERAL".into(), Some(literal.into())),
            ("CORRAL_GATE_ABSENT".into(), None),
        ],
        gate_tier: None,
    };
    let commands = vec![
        r#"printf '%s\0%s' "$CORRAL_GATE_LITERAL" "${CORRAL_GATE_ABSENT+set}${CORRAL_GATE_TIER+tier}${CORRAL_GATE_CONTEXT+context}" > observed"#.into(),
    ];
    let script = render(&identity, tmp.path(), &commands, &paths, None).unwrap();
    #[cfg(not(target_os = "linux"))]
    let script = {
        // The library test executable is not the CLI. Substitute only that external
        // probe boundary; the native CLI protocol has its own executable-level test.
        let probe = tmp.path().join("probe fixture.sh");
        std::fs::write(
            &probe,
            "#!/usr/bin/env bash\ncase \"$*\" in\nruntime-probe) echo 1000;;\n\"runtime-probe --pid \"*) echo fixture-identity;;\n*) exit 2;;\nesac\n",
        )
        .unwrap();
        let from = qp(&crate::prompt::corral_bin().unwrap());
        let replacement = format!("bash {}", qp(&probe));
        script.replace(
            &format!("probe={from}"),
            &format!("probe() {{ {replacement} \"$@\"; }}\nprobe=probe"),
        )
    };
    std::fs::write(paths.result.with_extension("owner"), "native-owner").unwrap();
    std::fs::write(&paths.script, script).unwrap();
    let capture = tmp.path().join("capture.log");
    let output = std::fs::File::create(&capture).unwrap();
    #[cfg(windows)]
    let shell = {
        // A bare bash.exe may resolve to Windows' WSL launcher, not Git Bash.
        let git = Command::new("git")
            .arg("--exec-path")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(git.status.success(), "{git:?}");
        let exec_path = String::from_utf8(git.stdout).unwrap();
        let root = std::path::Path::new(exec_path.trim())
            .ancestors()
            .nth(3)
            .unwrap();
        let bash = root.join("bin/bash.exe");
        assert!(bash.is_file(), "Git for Windows with Git Bash is required");
        bash
    };
    #[cfg(not(windows))]
    let shell = std::path::PathBuf::from("bash");
    let mut command = Command::new(shell);
    command
        .args([
            "--noprofile",
            "--norc",
            "-c",
            &format!(
                "PATH=\"/usr/bin:/bin:$PATH\" exec bash {}",
                qp(&paths.script)
            ),
        ])
        .env_remove("BASH_ENV")
        .env("CORRAL_GATE_LITERAL", "pane-value")
        .env("CORRAL_GATE_ABSENT", "pane-value")
        // An enclosing quick-tier gate's exports; a legacy (no-context) run must drop them.
        .env("CORRAL_GATE_TIER", "quick")
        .env("CORRAL_GATE_CONTEXT", "/stale/context.json")
        .stdin(Stdio::null())
        .stdout(output.try_clone().unwrap())
        .stderr(output);
    // When the actual generated runner executes under the documented POSIX shell.
    let (status, timed_out) = crate::hook_process::HookChild::spawn(command)
        .unwrap()
        .wait(30)
        .unwrap();
    // Then quoted paths/values, absent variables, and machine receipts all survive.
    assert!(
        status.success() && !timed_out,
        "{}",
        std::fs::read_to_string(capture).unwrap()
    );
    assert_eq!(
        std::fs::read(tmp.path().join("observed")).unwrap(),
        format!("{literal}\0").as_bytes()
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(paths.result).unwrap()).unwrap();
    assert_eq!(receipt["owner_token"], "native-owner");
    assert_eq!(receipt["result"], "pass");
    assert_eq!(receipt["commands"][0]["exit_code"], 0);
    let command: corral_core::CommandReceipt =
        serde_json::from_value(receipt["commands"][0].clone()).unwrap();
    assert!(command.started.unwrap() <= command.ended.unwrap());
    assert_eq!(
        serde_json::from_value::<CandidateIdentity>(receipt["identity"].clone()).unwrap(),
        identity
    );
}

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
        gate_tier: None,
    };
    let commands =
        vec![r#"printf '%s\0%s\0%s' "$PATH" "$RUSTC" "${RUSTFLAGS+set}" > observed"#.into()];
    std::fs::write(paths.result.with_extension("owner"), "private-owner").unwrap();
    std::fs::write(
        &paths.script,
        render(&identity, tmp.path(), &commands, &paths, None).unwrap(),
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

#[test]
fn command_duration_survives_a_backward_wall_clock_step() {
    // Given deterministic monotonic readings and a wall clock stepping over midnight backwards.
    let tmp = tempfile::tempdir().unwrap();
    let paths = GatePaths {
        script: tmp.path().join("gate.sh"),
        result: tmp.path().join("gate.json"),
        failed: tmp.path().join("gate.failed"),
        log: tmp.path().join("gate.log"),
    };
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
        environment: vec![],
        gate_tier: None,
    };
    let clock = tmp.path().join("clock.sh");
    let stepped = q(&tmp.path().join("stepped").display().to_string());
    std::fs::write(
        &clock,
        format!(
            r#"unset EPOCHREALTIME
EPOCHREALTIME=0
date() {{
  if [ -f {stepped} ]; then printf '%s\n' 2026-10-15T23:59:59Z
  else printf '%s\n' 2026-10-16T00:00:01Z; fi
}}
awk() {{
  if [ "${{@: -1}}" = /proc/uptime ]; then
    if [ -f {stepped} ]; then printf '1000250\n'; else printf '1000000\n'; fi
  else command awk "$@"; fi
}}
"#
        ),
    )
    .unwrap();
    std::fs::write(paths.result.with_extension("owner"), "private-owner").unwrap();
    std::fs::write(
        &paths.script,
        render(
            &identity,
            tmp.path(),
            &[format!("touch {stepped}")],
            &paths,
            None,
        )
        .unwrap(),
    )
    .unwrap();
    // When the real runner executes a command between those readings.
    let output = Command::new("/bin/bash")
        .arg(&paths.script)
        .env("BASH_ENV", clock)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    // Then the typed receipt retains elapsed monotonic time despite reversed timestamps.
    assert!(output.status.success(), "{output:?}");
    let receipt: crate::watch::CommandProgress =
        serde_json::from_slice(&std::fs::read(paths.log.with_extension("command-0.json")).unwrap())
            .unwrap();
    assert!(receipt.ended.unwrap() < receipt.started.unwrap());
    assert_eq!(receipt.duration_ms, Some(250));
    assert_eq!(receipt.exit_code, Some(0));
}
