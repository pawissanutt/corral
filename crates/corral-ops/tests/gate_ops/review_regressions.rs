use super::*;
use std::collections::BTreeMap;
use std::process::Stdio;

fn run_local_contract(ctx: &Ctx, id: &str, worktree: &Path) {
    let task = ctx.store.get(id).unwrap();
    let mut run = task.current_run().unwrap().clone();
    // This local-contract fixture supplies its template as the run's bound input.
    run.execution.as_mut().unwrap().prompt_template = Some("{{gate}}\nEND_GATE\n".into());
    let prompt = corral_ops::task_prompt(ctx, &task, &run).unwrap();
    let contract = prompt.split("\nEND_GATE\n").next().unwrap();
    let commands = contract.split_once("\n\n").unwrap().1;
    let output = support::command("bash", ctx.store.root())
        .current_dir(worktree)
        .args(["-o", "pipefail", "-c", commands])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn runner_bookkeeping_names_match_the_local_contract_in_every_command() {
    // Given accepted variable names that collide with runner scalars and arrays.
    let names = [
        "c",
        "log",
        "result",
        "failed",
        "identity",
        "receipts",
        "cmds",
        "json_cmds",
        "i",
        "start",
        "code",
        "codes",
        "end",
        "receipt",
        "owner_token",
        "command_environment",
    ];
    let arguments = names
        .iter()
        .map(|name| format!("\"${name}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let commands = [
        format!("printf '%s\\0' {arguments} > first.env"),
        format!("printf '%s\\0' {arguments} > second.env"),
    ];
    let (_tmp, mut ctx, id, worktree) =
        running_task(&commands.iter().map(String::as_str).collect::<Vec<_>>());
    let environment: BTreeMap<String, String> = names
        .iter()
        .map(|name| ((*name).into(), format!("configured '{name}'\n$value")))
        .collect();
    ctx.store.config.projects.get_mut("app").unwrap().review.env = Some(environment.clone());
    ctx.store.save_config().unwrap();
    std::fs::write(worktree.join(".gitignore"), "first.env\nsecond.env\n").unwrap();
    git(&worktree, &["add", ".gitignore"]);
    git(
        &worktree,
        &["commit", "-qm", "ignore environment observations"],
    );
    run_local_contract(&ctx, &id, &worktree);
    let expected: Vec<u8> = names
        .iter()
        .flat_map(|name| environment[*name].bytes().chain(std::iter::once(0)))
        .collect();
    let local: Vec<_> = ["first.env", "second.env"]
        .iter()
        .map(|name| std::fs::read(worktree.join(name)).unwrap())
        .collect();
    assert!(local.iter().all(|bytes| bytes == &expected));

    // When the same configured commands execute through the automated gate.
    corral_ops::gate_start(&ctx, &id).unwrap();
    let record = corral_ops::gate_wait(&ctx, &id).unwrap();

    // Then every command observes the configured bytes, not bookkeeping values.
    assert_eq!(record.result, GateResult::Pass, "{record:?}");
    for (name, local_bytes) in ["first.env", "second.env"].iter().zip(local) {
        assert_eq!(
            std::fs::read(worktree.join(name)).unwrap(),
            local_bytes,
            "{name}"
        );
    }
    let captured: BTreeMap<_, _> = record.identity.unwrap().environment.into_iter().collect();
    for (name, value) in environment {
        assert_eq!(captured[&name].as_ref(), Some(&value), "{name}");
    }
}

#[test]
fn thousand_separated_guidance_reaches_local_and_automated_gate_environments() {
    // Given the actual Mitigraph rule in a hermetic repository with no size override.
    let (_tmp, ctx, id, worktree) =
        running_task(&[r#"printf '%s' "$CORRAL_SIZE_LIMITS" > limits.json"#]);
    std::fs::write(
        worktree.join("AGENTS.md"),
        include_str!("../fixtures/mitigraph-size-policy.md"),
    )
    .unwrap();
    std::fs::write(worktree.join(".gitignore"), "limits.json\n").unwrap();
    git(&worktree, &["add", "AGENTS.md", ".gitignore"]);
    git(&worktree, &["commit", "-qm", "Mitigraph ceiling guidance"]);

    // When both local and automated gates resolve the repository's default.
    run_local_contract(&ctx, &id, &worktree);
    let local: serde_json::Value =
        serde_json::from_slice(&std::fs::read(worktree.join("limits.json")).unwrap()).unwrap();
    corral_ops::gate_start(&ctx, &id).unwrap();
    let record = corral_ops::gate_wait(&ctx, &id).unwrap();

    // Then both export 1000, and the machine receipt captures that exact map.
    assert_eq!(record.result, GateResult::Pass);
    let automated: serde_json::Value =
        serde_json::from_slice(&std::fs::read(worktree.join("limits.json")).unwrap()).unwrap();
    assert_eq!(local, serde_json::json!({"rs": 1000}));
    assert_eq!(automated, local);
    let captured = record
        .identity
        .unwrap()
        .environment
        .into_iter()
        .find(|(name, _)| name == "CORRAL_SIZE_LIMITS")
        .unwrap()
        .1
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&captured).unwrap(),
        local
    );
}
