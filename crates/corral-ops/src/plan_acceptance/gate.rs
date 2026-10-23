//! The existing gate runner and receipt checks, executed on the combined candidate.
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use corral_core::{AggregateAttempt, GateRecord, GateResult, GateTierReceipt};

use crate::Ctx;
use crate::gate::{GatePaths, ResultFile, command_log_valid, prepare_owner};
use crate::gate_tier::GateContract;
use crate::verification::digest;

pub(super) fn paths(ctx: &Ctx, plan: &str, attempt: &str) -> GatePaths {
    let dir = ctx.store.state_dir().join("plan-gates");
    let file = |ext: &str| dir.join(format!("{plan}-{attempt}.{ext}"));
    GatePaths {
        script: file("sh"),
        result: file("json"),
        failed: file("failed"),
        log: file("log"),
    }
}

/// Run the full tier's commands in the verification checkout and validate its receipts. In a
/// tiered project the tier command also sees the contract's empty expected-failure set.
/// The result file and command log, not the runner's exit status, are the authority.
pub(super) fn run(
    ctx: &Ctx,
    attempt: &AggregateAttempt,
    checkout: &Path,
    contract: &GateContract,
) -> Result<GateRecord> {
    let commands = &contract.commands;
    let identity = &attempt.identity;
    let paths = paths(ctx, &identity.task, &attempt.id);
    let dir = paths.log.parent().context("plan gate has no directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let context_path = crate::gate_tier::context_path(&paths.log);
    for path in [&paths.result, &paths.failed, &paths.log, &context_path] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("removing {}", path.display()));
            }
        }
    }
    let owner = paths.result.with_extension("owner");
    prepare_owner(&owner)?;
    let context = (!contract.rules.is_legacy())
        .then(|| crate::gate_tier::write_context(&context_path, contract, identity, checkout))
        .transpose()?;
    let script =
        crate::gate_script::render(identity, checkout, commands, &paths, context.as_ref())?;
    std::fs::write(&paths.script, script)
        .with_context(|| format!("writing {}", paths.script.display()))?;
    Command::new("bash")
        .arg(&paths.script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("running Plan gate")?;
    let text = std::fs::read_to_string(&paths.result)
        .with_context(|| format!("Plan gate produced no result {}", paths.result.display()))?;
    let parsed: ResultFile = serde_json::from_str(text.trim())
        .with_context(|| format!("parsing {}", paths.result.display()))?;
    if parsed.schema_version != 2 {
        bail!("unsupported gate result schema {}", parsed.schema_version);
    }
    let token = std::fs::read_to_string(&owner).context("reading Plan gate ownership")?;
    if token.is_empty() || parsed.owner_token != token {
        bail!("gate result ownership token does not match this attempt");
    }
    let log =
        std::fs::read(&paths.log).with_context(|| format!("reading {}", paths.log.display()))?;
    let reported = match parsed.result.as_str() {
        "pass" => GateResult::Pass,
        "fail" => GateResult::Fail,
        other => bail!("unknown gate result {other:?}"),
    };
    let receipts_valid = command_log_valid(&log, commands, &parsed.commands, reported);
    let (result, failed_cmd) = if parsed.identity != *identity {
        (
            GateResult::Fail,
            Some("stale_result: gate identity differs from the Plan attempt".into()),
        )
    } else if !receipts_valid
        || (reported == GateResult::Pass
            && (parsed.exit_code != 0 || parsed.commands.len() != commands.len()))
    {
        (GateResult::Fail, Some("invalid_command_receipts".into()))
    } else if reported == GateResult::Fail {
        let failed = std::fs::read_to_string(&paths.failed)
            .with_context(|| format!("reading {}", paths.failed.display()))?;
        (GateResult::Fail, Some(failed))
    } else {
        (GateResult::Pass, None)
    };
    Ok(GateRecord {
        identity: Some(identity.clone()),
        commands: parsed.commands,
        log_sha256: Some(digest(&log)),
        result,
        process_lost: false,
        failed_cmd,
        at: corral_core::clock::now(),
        sha: identity.head.clone(),
        pane: None,
        socket: None,
        tier: context.map(|context| {
            Box::new(GateTierReceipt {
                tier: context.tier,
                context_sha256: context.sha256,
                carried: None,
                promoted_from: None,
            })
        }),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::BTreeMap;

    use corral_core::{ExpectedFailure, GateResult, MemberGateRecord, ScopeGate, TierPolicy};

    use crate::plan_acceptance::plan_verify;
    use crate::plan_acceptance::tests::{Delivery, Fixture};

    const GATE: &str = r#"dir=$(dirname "$CORRAL_GATE_CONTEXT") && cp "$CORRAL_GATE_CONTEXT" "$dir/seen.json" && echo "$CORRAL_GATE_TIER" > "$dir/seen.tier" && test ! -e "$dir/declared-failure""#;

    #[test]
    fn plan_verify_runs_full_with_an_empty_expected_failure_set() {
        // Given a tiered project whose members declared an expected failure in their gates.
        let f = Fixture::new(Delivery::Integration, &[GATE]);
        let mut store = f.ctx.store.clone();
        store.config.review.gate_tiers = Some(BTreeMap::from([(
            "quick".to_owned(),
            vec!["true".to_owned()],
        )]));
        store.config.review.tier_policy = Some(TierPolicy {
            plan_member: Some("quick".into()),
            review_rounds: None,
        });
        store.save_config().unwrap();
        let gate = ScopeGate {
            tier: "quick".into(),
            tests: Vec::new(),
            expected_failures: vec![ExpectedFailure {
                test: "app::suite still_failing".into(),
                reason: "restored by second".into(),
            }],
        };
        f.ctx
            .store
            .update(&f.plan, |tracker| {
                for member in &f.members {
                    tracker.member_gates.insert(
                        member.clone(),
                        MemberGateRecord {
                            revision: 1,
                            gate: gate.clone(),
                            reason: "declared".into(),
                            by: "owner".into(),
                            at: corral_core::clock::now(),
                            history: Vec::new(),
                        },
                    );
                }
                Ok(())
            })
            .unwrap();
        let dir = f.ctx.store.state_dir().join("plan-gates");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("declared-failure"), "").unwrap();
        // When plan verify runs while the declared test still fails.
        let attempt = plan_verify(&f.ctx, &f.plan).unwrap();
        // Then the full commands ran with an empty set, and the attempt fails.
        let record = attempt.gate.unwrap();
        assert_eq!(record.result, GateResult::Fail);
        assert_eq!(record.failed_cmd.as_deref(), Some(GATE));
        let commands: Vec<_> = record.commands.iter().map(|r| r.command.as_str()).collect();
        assert_eq!(commands, [GATE]);
        assert_eq!(
            std::fs::read_to_string(dir.join("seen.tier")).unwrap(),
            "full\n"
        );
        let bytes = std::fs::read(dir.join("seen.json")).unwrap();
        let context: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(context["tier"], "full");
        assert_eq!(context["tests"], serde_json::json!([]));
        assert_eq!(context["expected_failures"], serde_json::json!([]));
        let tier = record.tier.unwrap();
        assert_eq!(tier.tier, "full");
        assert_eq!(tier.context_sha256, crate::verification::digest(&bytes));
        // Once a member fixes the test, the aggregate passes.
        std::fs::remove_file(dir.join("declared-failure")).unwrap();
        let attempt = plan_verify(&f.ctx, &f.plan).unwrap();
        assert_eq!(attempt.gate.unwrap().result, GateResult::Pass);
    }
}

/// Why the checkout no longer holds exactly `head`: a moved HEAD or tracked/untracked changes.
/// Ignored build output is permitted, as for task worktrees.
pub(super) fn checkout_changed(checkout: &Path, head: &str) -> Result<Option<String>> {
    if !checkout.is_dir() {
        return Ok(Some(format!("{} is missing", checkout.display())));
    }
    let actual = corral_git::rev_sha(checkout, "HEAD")?;
    if actual != head {
        return Ok(Some(format!("HEAD is {actual}, not the candidate {head}")));
    }
    if !corral_git::is_clean(checkout)? {
        return Ok(Some("tracked or untracked files changed".into()));
    }
    Ok(None)
}

/// The recorded pass still matches its unchanged log, the current gate commands and the
/// exact clean candidate in the checkout where it ran.
pub(super) fn passed(
    ctx: &Ctx,
    attempt: &AggregateAttempt,
    commands: &[String],
    checkout: &Path,
) -> Result<bool> {
    let Some(gate) = attempt
        .gate
        .as_ref()
        .filter(|gate| gate.result == GateResult::Pass)
    else {
        return Ok(false);
    };
    let path = paths(ctx, &attempt.identity.task, &attempt.id).log;
    let log = match std::fs::read(&path) {
        Ok(log) => log,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    Ok(gate.identity.as_ref() == Some(&attempt.identity)
        && gate.log_sha256.as_deref() == Some(digest(&log).as_str())
        && command_log_valid(&log, commands, &gate.commands, GateResult::Pass)
        && checkout_changed(checkout, &attempt.identity.head)?.is_none())
}
