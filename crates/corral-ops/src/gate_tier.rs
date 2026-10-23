//! One gate contract per run and phase. The gate runner, the completion audit, freshness,
//! dispatch and the readbacks all take their tier, commands and requirement from [`select`].
mod context;
mod instructions;
mod requirement;

use anyhow::{Context as _, Result};
use corral_core::{
    Config, ExpectedFailure, FULL_TIER, GatePromotion, GateRecord, GateRules, GateTierBinding,
    PolicySource, ReviewPolicy, Run, Store, Task, Verdict,
};
use serde::Serialize;

use crate::Ctx;

pub(crate) use context::{GateContext, path_for as context_path, write as write_context};
pub use instructions::{GateInstructionConflict, scan_gate_instructions};
pub(crate) use instructions::{dispatch_rules, report_blocked};
pub use requirement::{GateRequirement, MAIN_LANDING_TIER, satisfies};

#[cfg(test)]
#[path = "gate_tier_tests.rs"]
mod tests;

const REVIEW_ROUNDS_SOURCE: &str = "policy:review_rounds";

/// Review until the current candidate has an approval; landing afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GatePhase {
    Review,
    Landing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationKind {
    Main,
    PlanTarget,
    /// A queue base override or any other branch: today's full-gate rules.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateDestination {
    pub kind: DestinationKind,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateContract {
    pub tier: String,
    pub commands: Vec<String>,
    /// The task's write scope.
    pub scope: Vec<String>,
    pub tests: Vec<String>,
    pub expected_failures: Vec<ExpectedFailure>,
    pub destination: GateDestination,
    pub landing_tier: String,
    pub phase: GatePhase,
    pub source: String,
    /// Legacy when the run is legacy or its project defines no tiers.
    pub rules: GateRules,
    pub requirement: GateRequirement,
}

impl GateContract {
    pub(crate) fn binding(&self) -> Option<Box<GateTierBinding>> {
        self.requirement.binding.clone().map(Box::new)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateRef {
    pub attempt: String,
    pub tier: String,
    /// SHA-256 of the receipt identity's canonical JSON.
    pub receipt_sha256: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GateReadback {
    pub gate_contract: Option<GateContract>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_contract_error: Option<String>,
    pub gate_ref: Option<GateRef>,
}

pub(crate) fn current_phase(run: &Run) -> GatePhase {
    if run.review.as_ref().and_then(|review| review.verdict) == Some(Verdict::Approve) {
        GatePhase::Landing
    } else {
        GatePhase::Review
    }
}

/// The run's gate contract in `phase`, under freshly loaded policy.
///
/// # Errors
/// Invalid review configuration, or a tiered Plan member whose gate cannot be resolved.
pub(crate) fn select(ctx: &Ctx, task: &Task, run: &Run, phase: GatePhase) -> Result<GateContract> {
    let store = ctx.store.reopen()?;
    store.config.validate_review()?;
    let policy = store.config.task_review_policy(task);
    let destination = destination(&store.config, task, run);
    let contract = full(&store.config, &policy, task, destination, phase);
    if run.gate_rules.is_legacy() || policy.gate_tiers.is_empty() {
        return Ok(contract);
    }
    let contract = GateContract {
        rules: GateRules::Tiered,
        ..contract
    };
    match contract.destination.kind {
        DestinationKind::PlanTarget => member(&store, &policy, task, contract),
        DestinationKind::Main if phase == GatePhase::Review => review_rounds(&policy, contract),
        DestinationKind::Main | DestinationKind::Other => Ok(contract),
    }
}

/// Legacy runs need no policy read: they are full with full dominance in every phase.
pub(crate) fn requirement_for(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    phase: GatePhase,
) -> Result<GateRequirement> {
    if run.gate_rules.is_legacy() {
        return Ok(GateRequirement::full());
    }
    Ok(select(ctx, task, run, phase)?.requirement)
}

/// `plan verify` and `plan accept`: the full tier with an empty expected-failure set, whatever
/// the members declared.
pub(crate) fn aggregate(config: &Config, tracker: &Task, policy: &ReviewPolicy) -> GateContract {
    let destination = GateDestination {
        kind: DestinationKind::Main,
        branch: main_branch(config, tracker).to_owned(),
    };
    let rules = if policy.gate_tiers.is_empty() {
        GateRules::Legacy
    } else {
        GateRules::Tiered
    };
    GateContract {
        rules,
        ..full(config, policy, tracker, destination, GatePhase::Landing)
    }
}

/// The completion command snapshot of a new run: legacy runs keep today's full commands.
pub(crate) fn required_checks(ctx: &Ctx, task: &Task, run: &Run) -> Result<Vec<String>> {
    if run.gate_rules.is_legacy() {
        return Ok(ctx
            .store
            .config
            .review_policy(task.project.as_deref(), task.review)
            .gate);
    }
    Ok(select(ctx, task, run, GatePhase::Review)?.commands)
}

pub(crate) fn tier_commands(policy: &ReviewPolicy, tier: &str) -> Option<Vec<String>> {
    if tier == FULL_TIER {
        Some(policy.gate.clone())
    } else {
        policy.gate_tiers.get(tier).cloned()
    }
}

/// The commands a receipt must have run: its own tier's, from current policy.
pub(crate) fn receipt_commands(policy: &ReviewPolicy, gate: &GateRecord) -> Vec<String> {
    let tier = gate.tier.as_ref().map_or(FULL_TIER, |t| t.tier.as_str());
    tier_commands(policy, tier).unwrap_or_default()
}

/// What a gate start does with the run's promotion origin, decided before the identity is
/// captured and re-checked in the locked update that replaces `run.gate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OriginStep {
    Keep,
    /// The origin is current: copy it and capture the approved environment.
    Copy(GatePromotion),
    /// The first landing gate of a promotable approval whose quick receipt is still current.
    Record(GatePromotion),
    /// The approval changed, so the origin is inert.
    Clear,
}

impl OriginStep {
    pub(crate) fn promoted_from(&self) -> Option<&GatePromotion> {
        match self {
            Self::Copy(origin) | Self::Record(origin) => Some(origin),
            Self::Keep | Self::Clear => None,
        }
    }

    pub(crate) fn apply(&self, run: &mut Run) {
        match self {
            Self::Record(origin) => run.promotion_origin = Some(Box::new(origin.clone())),
            Self::Clear => run.promotion_origin = None,
            Self::Keep | Self::Copy(_) => {}
        }
    }
}

/// Only landing gates of tiered main-bound runs touch the origin. An approval is promotable
/// when it is bound to a reduced-tier identity.
pub(crate) fn origin_step(run: &Run, contract: &GateContract) -> OriginStep {
    if contract.rules.is_legacy()
        || contract.phase != GatePhase::Landing
        || contract.destination.kind != DestinationKind::Main
    {
        return OriginStep::Keep;
    }
    let approved = run
        .review
        .as_ref()
        .filter(|review| review.verdict == Some(Verdict::Approve))
        .and_then(|review| review.identity.as_ref())
        .filter(|identity| identity.gate_tier.is_some());
    let origin = run.promotion_origin.as_deref();
    if let Some(approved) = approved {
        if let Some(origin) = origin.filter(|origin| origin.identity == *approved) {
            return OriginStep::Copy(origin.clone());
        }
        if let Some(receipt) = run
            .gate
            .as_ref()
            .filter(|gate| gate.identity.as_ref() == Some(approved))
        {
            return OriginStep::Record(GatePromotion {
                identity: approved.clone(),
                gate_at: receipt.at,
            });
        }
    }
    if origin.is_some() {
        OriginStep::Clear
    } else {
        OriginStep::Keep
    }
}

/// `corral show --json`: the current run's contract and its receipt, from the same resolver.
///
/// # Errors
/// Only store errors; a contract that cannot be resolved is reported in `gate_contract_error`.
pub fn gate_readback(ctx: &Ctx, task: &Task) -> Result<GateReadback> {
    let Some(run) = task.current_run() else {
        return Ok(GateReadback::default());
    };
    let (gate_contract, gate_contract_error) = match select(ctx, task, run, current_phase(run)) {
        Ok(contract) => (Some(contract), None),
        Err(error) => (None, Some(format!("{error:#}"))),
    };
    let gate_ref = match run.gate.as_ref() {
        Some(gate) => match &gate.identity {
            Some(identity) => Some(GateRef {
                attempt: identity.attempt.clone(),
                tier: gate
                    .tier
                    .as_ref()
                    .map_or_else(|| FULL_TIER.to_owned(), |t| t.tier.clone()),
                receipt_sha256: crate::verification::digest(&serde_json::to_vec(identity)?),
            }),
            None => None,
        },
        None => None,
    };
    Ok(GateReadback {
        gate_contract,
        gate_contract_error,
        gate_ref,
    })
}

fn full(
    config: &Config,
    policy: &ReviewPolicy,
    task: &Task,
    destination: GateDestination,
    phase: GatePhase,
) -> GateContract {
    GateContract {
        tier: FULL_TIER.to_owned(),
        commands: policy.gate.clone(),
        scope: task.write_scope.clone(),
        tests: Vec::new(),
        expected_failures: Vec::new(),
        destination,
        landing_tier: MAIN_LANDING_TIER.to_owned(),
        phase,
        source: full_source(config, task),
        rules: GateRules::Legacy,
        requirement: GateRequirement::full(),
    }
}

/// A member's gate in every phase; it always carries a binding, also for tier full.
fn member(
    store: &Store,
    policy: &ReviewPolicy,
    task: &Task,
    contract: GateContract,
) -> Result<GateContract> {
    let member = crate::plan::scope_gate::resolve(store, task)?.with_context(|| {
        format!(
            "{} is bound to a Plan target but is not a member of an integration Plan; no gate starts",
            task.id
        )
    })?;
    let gate = member.gate;
    let commands = tier_commands(policy, &gate.tier).with_context(|| {
        format!(
            "member gate tier {:?} of {} is not defined for project {}; no gate starts",
            gate.tier,
            task.id,
            task.project.as_deref().unwrap_or("(none)")
        )
    })?;
    let binding = requirement::binding(
        &gate.tier,
        &commands,
        &gate.tests,
        &gate.expected_failures,
        &member.source,
    )?;
    Ok(GateContract {
        landing_tier: gate.tier.clone(),
        tier: gate.tier,
        commands,
        tests: gate.tests,
        expected_failures: gate.expected_failures,
        source: member.source,
        requirement: GateRequirement {
            binding: Some(binding),
            full_dominates: false,
        },
        ..contract
    })
}

fn review_rounds(policy: &ReviewPolicy, contract: GateContract) -> Result<GateContract> {
    let Some(tier) = policy.tier_policy.review_rounds.clone() else {
        return Ok(contract);
    };
    if tier == FULL_TIER {
        return Ok(GateContract {
            source: REVIEW_ROUNDS_SOURCE.to_owned(),
            ..contract
        });
    }
    let commands = tier_commands(policy, &tier)
        .with_context(|| format!("tier_policy.review_rounds names undefined tier {tier:?}"))?;
    let binding = requirement::binding(&tier, &commands, &[], &[], REVIEW_ROUNDS_SOURCE)?;
    Ok(GateContract {
        tier,
        commands,
        source: REVIEW_ROUNDS_SOURCE.to_owned(),
        requirement: GateRequirement {
            binding: Some(binding),
            full_dominates: true,
        },
        ..contract
    })
}

fn destination(config: &Config, task: &Task, run: &Run) -> GateDestination {
    let (kind, branch) = match &run.plan_target {
        Some(target) => (DestinationKind::PlanTarget, target.branch.clone()),
        None if run.base == main_branch(config, task) => (DestinationKind::Main, run.base.clone()),
        None => (DestinationKind::Other, run.base.clone()),
    };
    GateDestination { kind, branch }
}

fn main_branch<'a>(config: &'a Config, task: &Task) -> &'a str {
    task.project
        .as_deref()
        .and_then(|project| config.projects.get(project))
        .and_then(|project| project.base.as_deref())
        .unwrap_or("main")
}

fn full_source(config: &Config, task: &Task) -> String {
    match config
        .review_policy_sources(task.project.as_deref(), task.review)
        .gate
    {
        PolicySource::Project => format!(
            "config:projects.{}.review.gate",
            task.project.as_deref().unwrap_or_default()
        ),
        PolicySource::Global => "config:review.gate".to_owned(),
        PolicySource::Default | PolicySource::Task => "config:default".to_owned(),
    }
}
