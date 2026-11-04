//! The existing gate runner and receipt checks, executed on the combined candidate.
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use corral_core::{AggregateAttempt, CandidateIdentity, GateRecord, GateResult, GateTierReceipt};

use crate::Ctx;
use crate::gate::evidence::{self, Start};
use crate::gate::{GatePaths, ResultFile, command_log_valid, prepare_owner};
use crate::gate_tier::GateContract;
use crate::verification::digest;

/// The `gate_attempts` run and scope of an aggregate attempt's gate.
pub(super) const AGGREGATE_SCOPE: &str = "aggregate";
/// The `gate_attempts` run and scope of an advisory checkpoint's gate.
pub(super) const CHECKPOINT_SCOPE: &str = "checkpoint";

fn stem(plan: &str, attempt: &str) -> String {
    format!("plan-{plan}-{attempt}")
}

pub(super) fn paths(ctx: &Ctx, plan: &str, attempt: &str) -> GatePaths {
    let stem = stem(plan, attempt);
    crate::gate::dir_paths(&evidence::gates_root(ctx).join(&stem), &stem)
}

/// Puts the evidence a lost attempt of `scope` left (a partial log, if any) for ending its row;
/// `None` when it has no open row.
pub(super) fn put_lost(
    ctx: &Ctx,
    plan: &str,
    scope: &str,
    attempt: &str,
) -> Result<Option<evidence::StagedResult>> {
    let stem = stem(plan, attempt);
    let paths = paths(ctx, plan, attempt);
    evidence::put_result(ctx, plan, scope, attempt, &stem, &paths)
}

/// One Plan gate run: the attempt it belongs to, its `gate_attempts` scope, and the previous
/// attempt of that scope whose scratch directory is reclaimed first.
pub(super) struct PlanGate<'a> {
    pub(super) attempt: &'a str,
    pub(super) scope: &'a str,
    pub(super) previous: Option<&'a str>,
}

/// A prepared Plan gate attempt: its control files in a registered scratch directory and its
/// started row's blobs, which the caller stages into the update that records the Running
/// attempt. The caller later stages [`Prepared::put_result`] into the update that saves the
/// attempt's final, validated outcome, so each marker commits with its row.
pub(super) struct Prepared {
    plan: String,
    scope: String,
    attempt: String,
    stem: String,
    pub(super) paths: GatePaths,
    owner: std::path::PathBuf,
    context: Option<crate::gate_tier::GateContext>,
    start: Option<evidence::StagedStart>,
}

/// Prepares the full tier's run in the verification checkout: reclaims the previous
/// attempt's directory, registers this attempt's, writes its owner token, context and script,
/// and puts its started row's blobs.
pub(super) fn prepare(
    ctx: &Ctx,
    identity: &CandidateIdentity,
    gate: PlanGate<'_>,
    checkout: &Path,
    contract: &GateContract,
) -> Result<Prepared> {
    let plan = identity.task.as_str();
    if let Some(previous) = gate.previous
        && let Err(error) = evidence::reclaim(ctx, &stem(plan, previous))
    {
        crate::messages::report(
            Some(plan),
            "gate",
            format!("reclaiming Plan gate scratch {previous} failed: {error:#}"),
        );
    }
    let stem = stem(plan, gate.attempt);
    let dir = evidence::open_dir(ctx, plan, gate.attempt, &stem)?;
    let paths = crate::gate::dir_paths(&dir, &stem);
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
    let commands = &contract.commands;
    let script =
        crate::gate_script::render(identity, checkout, commands, &paths, context.as_ref())?;
    std::fs::write(&paths.script, script)
        .with_context(|| format!("writing {}", paths.script.display()))?;
    let start = Start {
        task: plan,
        run: gate.scope,
        attempt: gate.attempt,
        scope: gate.scope,
        sha: &identity.head,
        tier: context.as_ref().map(|context| context.tier.as_str()),
        identity,
        commands,
        context: context.as_ref().map(|context| context.path.as_path()),
    };
    let start = evidence::put_start(ctx, &start)?;
    Ok(Prepared {
        plan: plan.to_owned(),
        scope: gate.scope.to_owned(),
        attempt: gate.attempt.to_owned(),
        stem,
        paths,
        owner,
        context,
        start,
    })
}

impl Prepared {
    /// Stages the started row into the open update that records the Running attempt.
    pub(super) fn stage_start(&self, store: &corral_core::Store) -> Result<()> {
        self.start
            .as_ref()
            .map_or(Ok(()), |start| start.stage(store))
    }

    /// Runs the commands and validates the receipts. The result file and command log, not
    /// the runner's exit status, are the authority.
    pub(super) fn execute(
        &self,
        ctx: &Ctx,
        identity: &CandidateIdentity,
        commands: &[String],
    ) -> Result<GateRecord> {
        execute(
            ctx,
            identity,
            commands,
            &self.paths,
            &self.owner,
            self.context.clone(),
        )
    }

    /// Puts the log and result blobs for the update that saves the attempt's final outcome;
    /// `None` when the attempt has no open row.
    pub(super) fn put_result(&self, ctx: &Ctx) -> Result<Option<evidence::StagedResult>> {
        evidence::put_result(
            ctx,
            &self.plan,
            &self.scope,
            &self.attempt,
            &self.stem,
            &self.paths,
        )
    }
}

fn execute(
    ctx: &Ctx,
    identity: &CandidateIdentity,
    commands: &[String],
    paths: &GatePaths,
    owner: &Path,
    context: Option<crate::gate_tier::GateContext>,
) -> Result<GateRecord> {
    let mut child = ctx.store.read_locked_head(&identity.task, |task| {
        crate::plan_recovery::require_active(&ctx.store, task)?;
        crate::hold::require_clear(&ctx.store, task)?;
        Command::new("bash")
            .arg(&paths.script)
            .env(
                "CORRAL_GATE_PROCESS_OWNER",
                crate::prompt::shell_path(&paths.script),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("starting Plan gate")
    })?;
    child.wait().context("running Plan gate")?;
    let text = std::fs::read_to_string(&paths.result)
        .with_context(|| format!("Plan gate produced no result {}", paths.result.display()))?;
    let parsed: ResultFile = serde_json::from_str(text.trim())
        .with_context(|| format!("parsing {}", paths.result.display()))?;
    if parsed.schema_version != 2 {
        bail!("unsupported gate result schema {}", parsed.schema_version);
    }
    let token = std::fs::read_to_string(owner).context("reading Plan gate ownership")?;
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

    use corral_core::{ExpectedFailure, GateResult, MemberGateAmendment, ScopeGate, TierPolicy};

    use crate::plan_acceptance::plan_verify;
    use crate::plan_acceptance::tests::{Delivery, Fixture};

    /// A gate command that copies its context and tier into `dir` and fails while
    /// `dir/declared-failure` exists.
    pub(in crate::plan_acceptance) fn observing_gate(dir: &std::path::Path) -> String {
        let dir = crate::gate_script::qp(dir);
        format!(
            r#"cp "$CORRAL_GATE_CONTEXT" {dir}/seen.json && echo "$CORRAL_GATE_TIER" > {dir}/seen.tier && test ! -e {dir}/declared-failure"#
        )
    }

    #[test]
    fn plan_verify_runs_full_with_an_empty_expected_failure_set() {
        // Given a tiered project whose members declared an expected failure in their gates.
        let seen = tempfile::tempdir().unwrap();
        let gate_command = observing_gate(seen.path());
        let f = Fixture::new(Delivery::Integration, &[&gate_command]);
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
        for member in &f.members {
            let amendment = MemberGateAmendment {
                member: member.clone(),
                expected_revision: 0,
                gate: gate.clone(),
                reason: "declared".into(),
                by: "owner".into(),
            };
            f.ctx
                .store
                .amend_member_gate(&f.plan, &amendment, |_, _| Ok(()), |_, _| Ok(None))
                .unwrap();
        }
        let dir = seen.path();
        std::fs::write(dir.join("declared-failure"), "").unwrap();
        // When plan verify runs while the declared test still fails.
        let attempt = plan_verify(&f.ctx, &f.plan).unwrap();
        // Then the full commands ran with an empty set, and the attempt fails.
        let record = attempt.gate.unwrap();
        assert_eq!(record.result, GateResult::Fail);
        assert_eq!(record.failed_cmd.as_deref(), Some(gate_command.as_str()));
        let commands: Vec<_> = record.commands.iter().map(|r| r.command.as_str()).collect();
        assert_eq!(commands, [gate_command.as_str()]);
        assert_eq!(
            std::fs::read_to_string(dir.join("seen.tier")).unwrap(),
            "full\n"
        );
        let bytes = std::fs::read(dir.join("seen.json")).unwrap();
        let context: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(context["tier"], "full");
        assert_eq!(context["plan_verify"], true);
        assert_eq!(context["tests"], serde_json::json!([]));
        assert_eq!(context["expected_failures"], serde_json::json!([]));
        let tier = record.tier.unwrap();
        assert_eq!(tier.tier, "full");
        assert_eq!(tier.context_sha256, crate::verification::digest(&bytes));
        // And the attempt's row holds the exact context bytes, and the log the record hashes.
        let row = f
            .ctx
            .store
            .gate_attempt(&f.plan, super::AGGREGATE_SCOPE, &attempt.id)
            .unwrap()
            .unwrap();
        assert_eq!(
            (row.scope.as_str(), row.tier.as_deref()),
            ("aggregate", Some("full"))
        );
        assert_eq!(row.result.as_deref(), Some("fail"));
        let blob = |blob| f.ctx.store.blob_bytes(blob).unwrap().unwrap();
        assert_eq!(blob(row.context.as_ref().unwrap()), bytes);
        let log = blob(row.log.as_ref().unwrap());
        assert_eq!(record.log_sha256, Some(crate::verification::digest(&log)));
        // Once a member fixes the test, the aggregate passes, and no plan-gates area exists.
        std::fs::remove_file(dir.join("declared-failure")).unwrap();
        let attempt = plan_verify(&f.ctx, &f.plan).unwrap();
        assert_eq!(attempt.gate.unwrap().result, GateResult::Pass);
        assert!(!f.ctx.store.dir().join("state/plan-gates").exists());
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
    let plan = &attempt.identity.task;
    let Some(log) = evidence::attempt_log(ctx, plan, AGGREGATE_SCOPE, &attempt.id)? else {
        return Ok(false);
    };
    Ok(gate.identity.as_ref() == Some(&attempt.identity)
        && gate.log_sha256.as_deref() == Some(digest(&log).as_str())
        && command_log_valid(&log, commands, &gate.commands, GateResult::Pass)
        && checkout_changed(checkout, &attempt.identity.head)?.is_none())
}
