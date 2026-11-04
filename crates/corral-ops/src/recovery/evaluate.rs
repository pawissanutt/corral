use super::*;
use serde_json::json;

pub(super) fn task_revision(task: &Task) -> Result<String> {
    let mut task = task.clone();
    task.updated = task.created; // Recovery intent timestamps do not change the protected workflow.
    hash(&Store::render_task_markdown(&task))
}

struct Evaluation {
    checks: Vec<RecoveryCheckRecord>,
    blockers: Vec<RecoveryBlocker>,
}
impl Evaluation {
    fn hard(&mut self, code: &str, result: Result<()>) {
        if let Err(error) = result {
            self.blockers.push(RecoveryBlocker {
                code: code.into(),
                class: BlockerClass::Hard,
                observed: RecoveryObservation::Unverifiable {
                    reason: format!("{error:#}"),
                },
                evidence: vec![],
                proposed_disposition: None,
                ordinary_command: None,
            });
        }
    }
    fn inherit(&mut self, record: &RecoveryRecord) {
        self.checks.extend(
            record
                .preview
                .checks
                .iter()
                .filter(|c| {
                    matches!(
                        c.check,
                        RecoveryCheck::ReviewerFamilyPolicy
                            | RecoveryCheck::ConfiguredReviewerBinding
                    )
                })
                .cloned()
                .map(|mut c| {
                    c.evidence.push(format!(
                        "recovery:{}@{}",
                        record.id, record.preview.contract_digest
                    ));
                    c
                }),
        );
    }
    fn unavailable(&mut self, check: RecoveryCheck, observed: RecoveryObservation) {
        self.checks.push(RecoveryCheckRecord {
            check,
            observed: observed.clone(),
            disposition: CheckDisposition::NotApplicable,
            evidence: vec![],
        });
        self.blockers.push(RecoveryBlocker {
            code: format!("{check:?}-receipt"),
            class: BlockerClass::Hard,
            observed,
            evidence: vec![],
            proposed_disposition: None,
            ordinary_command: None,
        });
    }
    fn predicate(
        &mut self,
        check: RecoveryCheck,
        satisfied: bool,
        evidence: Vec<String>,
        waived: bool,
    ) {
        self.observation(
            check,
            RecoveryObservation::Predicate { satisfied },
            satisfied,
            evidence,
            waived,
        );
    }
    fn observation(
        &mut self,
        check: RecoveryCheck,
        observed: RecoveryObservation,
        satisfied: bool,
        evidence: Vec<String>,
        waived: bool,
    ) {
        let disposition = if satisfied {
            CheckDisposition::Satisfied
        } else if waived {
            CheckDisposition::Waived
        } else {
            CheckDisposition::Overridden
        };
        if !satisfied {
            self.blockers.push(RecoveryBlocker {
                code: serde_json::to_value(check)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
                class: BlockerClass::Recoverable,
                observed: observed.clone(),
                evidence: evidence.clone(),
                proposed_disposition: Some(disposition),
                ordinary_command: None,
            });
        }
        self.checks.push(RecoveryCheckRecord {
            check,
            observed,
            disposition,
            evidence,
        });
    }
}

pub(super) fn preview(ctx: &Ctx, task: &Task, input: &RecoveryInput) -> Result<RecoveryPreview> {
    let mut e = Evaluation {
        checks: vec![],
        blockers: vec![],
    };
    e.hard(
        "configuration",
        Config::load(&ctx.store.config_path()).and_then(|config| {
            config
                .validate(ctx.store.root())
                .map_err(anyhow::Error::new)
        }),
    );
    e.hard(
        "session-ownership",
        crate::session_scope::owns_task(ctx, task).and_then(|owns| {
            ensure!(owns, "unknown or foreign session ownership");
            Ok(())
        }),
    );
    e.hard("task-hold", crate::hold::require_clear(&ctx.store, task));
    e.hard(
        "task-identity",
        (|| {
            ensure!(
                !matches!(task.status, Status::Done | Status::Dropped),
                "terminal task cannot be recovered"
            );
            Ok(())
        })(),
    );
    e.hard(
        "owner-questions",
        task.current_run().map_or(Ok(()), |run| {
            ensure!(
                run.obligations.pending_decision().is_none()
                    || matches!(input.action, RecoveryAction::AcceptPlan { .. }),
                "unresolved owner question"
            );
            Ok(())
        }),
    );
    let policy = policy::capture(&ctx.store.config, task, &input.action)?;
    let (identity, effect) = match &input.action {
        RecoveryAction::UseReviewer { stage, reviewer } => {
            e.hard(
                "reviewer-identity",
                policy::reviewer(&ctx.store.config, &reviewer.profile).and_then(|actual| {
                    ensure!(
                        actual == *reviewer,
                        "selected reviewer profile/model/vendor changed"
                    );
                    Ok(())
                }),
            );
            let (identity, attempt, author) = match stage {
                RecoveryReviewStage::PlanReview => {
                    let plan = task.plans.last().context("submit a plan before recovery")?;
                    e.hard("plan-freshness", crate::plan::fresh(ctx, task, plan));
                    e.hard(
                        "review-pending",
                        (|| {
                            ensure!(
                                plan.reviews
                                    .last()
                                    .is_none_or(|r| r.verdict.is_some() || r.cancelled),
                                "plan review already pending"
                            );
                            Ok(())
                        })(),
                    );
                    let author = crate::plan::planner(task)?
                        .map(|p| (p.profile.clone(), p.model.clone(), p.vendor.clone()));
                    (
                        plan_identity(plan),
                        format!("PR{}", plan.reviews.len() + 1),
                        author,
                    )
                }
                RecoveryReviewStage::CodeReview => {
                    let run = task.current_run().context("no code run")?;
                    e.hard(
                        "review-stage",
                        (|| {
                            ensure!(task.status == Status::Review, "code review requires Review");
                            Ok(())
                        })(),
                    );
                    e.hard(
                        "review-pending",
                        (|| {
                            ensure!(
                                run.review
                                    .as_ref()
                                    .is_none_or(|r| r.verdict.is_some() || r.cancelled),
                                "code review already pending"
                            );
                            Ok(())
                        })(),
                    );
                    let head = RunHead::of(run)?;
                    let candidate = code_candidate(ctx, task, &head)?;
                    (
                        RecoveryIdentity::Code {
                            candidate,
                            gate_attempt: run
                                .gate
                                .as_ref()
                                .and_then(|g| g.identity.as_ref().map(|i| i.attempt.clone())),
                            review_attempt: run
                                .review
                                .as_ref()
                                .and_then(crate::reviewer::review_callback_token),
                        },
                        format!("{}-review{}", run.id, run.review_runs.len() + 1),
                        Some((run.agent.clone(), run.model.clone(), run.vendor.clone())),
                    )
                }
            };
            e.hard("recovery-model-floor", floor(author.as_ref(), reviewer));
            let family = author
                .as_ref()
                .and_then(|(_, model, vendor)| model_family(vendor.as_deref(), model.as_deref()));
            let actual = model_family(Some(&reviewer.vendor), Some(&reviewer.model));
            let effective = ctx.store.config.task_review_policy(task);
            let family_result = effective.check_reviewer_family(family.as_ref(), actual.as_ref());
            e.predicate(
                RecoveryCheck::ReviewerFamilyPolicy,
                family_result.is_ok(),
                family_result
                    .err()
                    .map(|e| format!("{e:#}"))
                    .into_iter()
                    .collect(),
                false,
            );
            e.predicate(
                RecoveryCheck::ConfiguredReviewerBinding,
                policy.expected_reviewer.as_ref() == Some(reviewer),
                vec![format!(
                    "expected={:?}; selected={reviewer:?}; resolution={:?}",
                    policy.expected_reviewer, policy.resolution_failure
                )],
                false,
            );
            e.hard(
                "resource-pressure",
                crate::resources::read_admission(ctx).and_then(|reason| {
                    ensure!(reason.is_none(), "{}", reason.unwrap_or_default());
                    Ok(())
                }),
            );
            (
                identity,
                RecoveryEffect::ReviewAttempt {
                    stage: *stage,
                    attempt,
                    reviewer: reviewer.clone(),
                },
            )
        }
        RecoveryAction::AcceptPlan {
            waive_independent_review,
            resolution_notes_sha256,
        } => {
            let plan = task.plans.last().context("no plan to accept")?;
            e.hard("plan-freshness", crate::plan::fresh(ctx, task, plan));
            e.hard(
                "plan-attachment",
                crate::plan_decompose::check_attachment(&ctx.store, task, &plan.id),
            );
            e.hard(
                "owner-decisions",
                (|| {
                    ensure!(
                        crate::plan::decisions_resolved(task, plan, &input.notes),
                        "resolve every owner decision explicitly"
                    );
                    Ok(())
                })(),
            );
            e.hard(
                "resolution-notes",
                (|| {
                    ensure!(
                        !input.notes.trim().is_empty()
                            && *resolution_notes_sha256
                                == crate::verification::digest(input.notes.as_bytes()),
                        "resolution notes do not match request"
                    );
                    Ok(())
                })(),
            );
            if let Some(review) = plan.reviews.last()
                && let Some(record) = plan_review_record(ctx, task, &plan.id, &review.id)?
            {
                e.inherit(&record);
            }
            let reviewed = plan
                .reviews
                .last()
                .is_some_and(|r| r.verdict == Some(Verdict::Approve) && r.reviewer != "owner")
                && plan_review_valid(ctx, task, plan).is_ok();
            e.observation(
                RecoveryCheck::IndependentPlanReview,
                plan.reviews
                    .last()
                    .map_or(RecoveryObservation::Missing, |r| {
                        RecoveryObservation::Review { verdict: r.verdict }
                    }),
                reviewed,
                vec![format!("plan {}", plan.id)],
                true,
            );
            e.hard(
                "review-waiver",
                (|| {
                    ensure!(
                        reviewed || *waive_independent_review,
                        "missing independent approval needs its separate waiver"
                    );
                    Ok(())
                })(),
            );
            (
                plan_identity(plan),
                RecoveryEffect::PlanAcceptance {
                    plan: plan.id.clone(),
                    dispatch_holds: vec![task.id.clone()],
                },
            )
        }
        RecoveryAction::AcceptCode { merge } => {
            let run = task.current_run().context("no code run")?;
            let head = RunHead::of(run)?;
            let candidate = code_candidate(ctx, task, &head)?;
            if let Some(review) = &run.review
                && let Some(record) = code_review_record(ctx, task, review)?
            {
                e.inherit(&record);
            }
            e.hard(
                "acceptance-stage",
                (|| {
                    ensure!(
                        crate::review::awaiting_owner(task)?,
                        "code acceptance requires owner review"
                    );
                    Ok(())
                })(),
            );
            e.hard(
                "manual-receipts",
                crate::review::require_manual_receipts(ctx, task),
            );
            e.hard(
                "candidate-clean",
                (|| {
                    ensure!(
                        corral_git::is_clean(&run.worktree)?,
                        "candidate has uncommitted changes"
                    );
                    Ok(())
                })(),
            );
            e.hard(
                "required-evidence",
                (|| {
                    let failures = crate::completion::evidence_failures(ctx, task, &head)?;
                    ensure!(failures.is_empty(), "{}", failures.join("; "));
                    Ok(())
                })(),
            );
            let effective = ctx.store.config.task_review_policy(task);
            if !effective.gate.is_empty() {
                match &run.gate {
                    Some(gate)
                        if matches!(gate.result, GateResult::Pass | GateResult::Fail)
                            && crate::review::landing_gate_fresh(ctx, task, &head, gate)?
                            && crate::gate::gate_log_valid(ctx, task, &head)? =>
                    {
                        e.observation(
                            RecoveryCheck::GateOutcome,
                            RecoveryObservation::Gate {
                                result: gate.result,
                            },
                            gate.result == GateResult::Pass,
                            vec![format!("gate {:?}", gate.identity)],
                            false,
                        )
                    }
                    gate => e.unavailable(
                        RecoveryCheck::GateOutcome,
                        gate.as_ref().map_or(RecoveryObservation::Missing, |g| {
                            RecoveryObservation::Gate { result: g.result }
                        }),
                    ),
                }
            }
            if crate::review::review_needed(&effective, task) {
                match &run.review {
                    Some(review)
                        if review.verdict.is_some()
                            && crate::verification::review_fresh(ctx, task, &head)? =>
                    {
                        e.observation(
                            RecoveryCheck::ReviewOutcome,
                            RecoveryObservation::Review {
                                verdict: review.verdict,
                            },
                            review.verdict == Some(Verdict::Approve),
                            vec![format!("review {:?}", review.identity)],
                            false,
                        )
                    }
                    review => e.unavailable(
                        RecoveryCheck::ReviewOutcome,
                        review.as_ref().map_or(RecoveryObservation::Missing, |r| {
                            RecoveryObservation::Review { verdict: r.verdict }
                        }),
                    ),
                }
            }
            (
                RecoveryIdentity::Code {
                    candidate: candidate.clone(),
                    gate_attempt: run
                        .gate
                        .as_ref()
                        .and_then(|g| g.identity.as_ref().map(|i| i.attempt.clone())),
                    review_attempt: run
                        .review
                        .as_ref()
                        .and_then(crate::reviewer::review_callback_token),
                },
                RecoveryEffect::CodeAcceptance {
                    candidate,
                    merge: *merge,
                },
            )
        }
        RecoveryAction::ForceAdmission {
            stage,
            pressure_episode,
            thresholds_digest,
        } => {
            let pressure = crate::resources::read_status(ctx)?;
            let thresholds = hash(&ctx.store.config.resources)?;
            e.hard(
                "pressure-episode",
                (|| {
                    ensure!(
                        *pressure_episode == pressure_id(&pressure)?
                            && *thresholds_digest == thresholds,
                        "pressure episode or thresholds changed"
                    );
                    Ok(())
                })(),
            );
            e.predicate(
                RecoveryCheck::ResourcePressure,
                !pressure.held,
                pressure.reason.into_iter().collect(),
                false,
            );
            let attempt = match stage {
                RecoveryAdmissionStage::PlanReview => {
                    crate::plan_review::next_plan_attempt(ctx, task)?
                }
                RecoveryAdmissionStage::CodeReview => {
                    let run = task.current_run().context("no code run")?;
                    format!("{}-review{}", run.id, run.review_runs.len() + 1)
                }
                RecoveryAdmissionStage::Dispatch => format!("R{}", task.runs.len() + 1),
                RecoveryAdmissionStage::PlanDispatch => format!("D{}", task.plan_runs.len() + 1),
                RecoveryAdmissionStage::Gate => format!(
                    "recovery-gate-{}",
                    task.current_run().context("no code run")?.id
                ),
            };
            (
                RecoveryIdentity::Admission {
                    stage: *stage,
                    attempt: attempt.clone(),
                    run: task.current_run().map(|r| r.id.clone()),
                    plan: task.plans.last().map(|p| p.id.clone()),
                    candidate: task
                        .current_run()
                        .map(|r| RunHead::of(r).and_then(|r| code_candidate(ctx, task, &r)))
                        .transpose()?,
                },
                RecoveryEffect::Admission {
                    stage: *stage,
                    attempt,
                },
            )
        }
    };
    let exceptions: BTreeSet<_> = e
        .checks
        .iter()
        .filter(|c| {
            matches!(
                c.disposition,
                CheckDisposition::Overridden | CheckDisposition::Waived
            )
        })
        .map(|c| c.check)
        .collect();
    e.hard("exception-set", (|| { ensure!(exceptions == input.exceptions && !exceptions.is_empty(), "name exactly the observed exceptions: {exceptions:?}; use ordinary workflow when no exception is needed"); Ok(()) })());
    let remaining_blockers = e
        .blockers
        .iter()
        .filter(|b| b.class != BlockerClass::Recoverable)
        .cloned()
        .collect();
    let request = RecoveryRequest {
        version: RECOVERY_VERSION,
        target: RecoveryTarget {
            store: ctx.store.root().to_path_buf(),
            task: task.id.clone(),
            task_revision: task_revision(task)?,
            spec_revision: crate::plan::plan_spec_revision(task)?,
            identity,
        },
        action: input.action.clone(),
        exceptions: input.exceptions.clone(),
    };
    let contract_digest = hash(&(&request, &policy, &e.checks, &effect))?;
    let digest = hash(&(
        &contract_digest,
        &e.blockers,
        &remaining_blockers,
        &input.notes,
    ))?;
    Ok(RecoveryPreview {
        version: RECOVERY_VERSION,
        request,
        policy,
        checks: e.checks,
        blockers: e.blockers,
        remaining_blockers,
        proposed_effect: effect,
        affected_targets: vec![task.id.clone()],
        expires: corral_core::clock::now().checked_add(jiff::SignedDuration::from_secs(600))?,
        digest,
        contract_digest,
    })
}

pub(super) fn floor(
    author: Option<&(String, Option<String>, Option<String>)>,
    reviewer: &RecoveryReviewer,
) -> Result<()> {
    let (profile, model, vendor) = author.context("saved author identity is unknown")?;
    let model = model
        .as_deref()
        .context("saved author effective model is unknown")?;
    ensure!(vendor.is_some(), "saved author vendor is unknown");
    ensure!(
        *profile != reviewer.profile
            && model_key(vendor.as_deref(), model)
                != model_key(Some(&reviewer.vendor), &reviewer.model),
        "recovery requires a different profile and effective model"
    );
    Ok(())
}

fn plan_identity(plan: &PlanRecord) -> RecoveryIdentity {
    RecoveryIdentity::Plan {
        plan: plan.id.clone(),
        artifact_sha256: plan.artifact.sha256.clone(),
        review_attempt: plan.reviews.last().map(|r| r.id.clone()),
    }
}
pub(super) fn code_candidate(ctx: &Ctx, task: &Task, run: &RunHead) -> Result<CandidateIdentity> {
    let Some(original) = run
        .gate
        .as_ref()
        .and_then(|g| g.identity.as_ref())
        .or_else(|| run.review.as_ref().and_then(|r| r.identity.as_ref()))
    else {
        return crate::verification::capture_identity(ctx, task, run, "recovery-candidate");
    };
    let current = crate::verification::identity(ctx, task, run, original)?;
    ensure!(
        current == *original,
        "recovery_stale: candidate/base/receipt changed"
    );
    Ok(current)
}
pub(super) fn pressure_id(pressure: &ResourceState) -> Result<String> {
    hash(&json!({"held":pressure.held,"severe":pressure.severe}))
}
