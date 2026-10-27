#![cfg(unix)] // The fixture invokes an executable POSIX fake engine.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use corral_core::{
    GateRules, Harness, OperatingAuthority, OperatingSelection, Profile, ProfileEligibility,
    ProfileMetadata, ProfileRole, ReviewContractKind, ReviewRecord, SelectionGuidance, Status,
    Store, TierPolicy,
};
use jiff::Timestamp;
use serde_json::json;

use super::*;
use crate::operating::Origin;
use crate::review_budget::{Kind, ReviewLaunch};
use crate::test_support as support;

const FAKE: &str = r#"#!/usr/bin/env bash
cd "$(dirname "$0")" || exit 1
if [ "${1:-}" = "--session" ]; then shift 2; fi
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create"|"tab create")
    n=1
    while [ -f "created-$n" ]; do n=$((n+1)); done
    touch "created-$n" "w1:p$n"
    ok "{\"workspace\":{\"workspace_id\":\"w1\"},\"root_pane\":{\"workspace_id\":\"w1\",\"tab_id\":\"w1:t$n\",\"pane_id\":\"w1:p$n\"}}" ;;
  "pane get")
    if [ -f "$3" ]; then ok "{\"pane\":{\"pane_id\":\"$3\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}}"; else ok '{}'; fi ;;
  "pane close") rm -f "$3"; ok '{}' ;;
  *) ok '{}' ;;
esac
"#;
const CLEAN: &str = "Check naming.";
const TEMPLATE_DIRECTIVE: &str =
    "# Review {{id}}\n\n{{gate}}\nRun the full gate before deciding.\n";
const PROFILE_DIRECTIVE: &str = "Check naming.\nAlso run scripts/gate.sh first.";

struct Fx {
    tmp: tempfile::TempDir,
    ctx: Ctx,
    id: String,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = support::command("git", dir)
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}");
    String::from_utf8_lossy(&out.stdout).trim().into()
}

fn reviewer(vendor: &str) -> Profile {
    Profile {
        metadata: Some(ProfileMetadata {
            display_name: "Reviewer".into(),
            purpose: "Review".into(),
            guidance: SelectionGuidance::default(),
            instructions: CLEAN.into(),
            eligibility: ProfileEligibility {
                enabled: true,
                automatic: true,
                roles: BTreeSet::from([ProfileRole::ImplementationReview, ProfileRole::PlanReview]),
                projects: BTreeSet::new(),
            },
            revision: "r1".into(),
            evidence: None,
            candidates: vec![],
            fallback: None,
        }),
        harness: "h".into(),
        model: Some("review-model".into()),
        vendor: Some(vendor.into()),
        effort: None,
        args: vec![],
        escalate_to: None,
    }
}

/// A Review task on `main` whose run has `rules`, in a project with a quick tier.
fn fixture(rules: GateRules) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "test"]);
    git(&app, &["config", "user.email", "test@example.invalid"]);
    std::fs::write(app.join("tracked"), "initial\n").unwrap();
    git(&app, &["add", "."]);
    git(&app, &["commit", "-qm", "initial"]);
    let bin = tmp.path().join("herdr");
    std::fs::write(&bin, FAKE).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    let config = &mut store.config;
    config.limits.count_reviews = false;
    config.limits.count_gates = false;
    config.review.gate = Some(vec!["full-gate".into()]);
    config.review.gate_tiers = Some(BTreeMap::from([(
        "quick".to_owned(),
        vec!["quick-gate".to_owned()],
    )]));
    config.review.tier_policy = Some(TierPolicy {
        plan_member: Some("quick".into()),
        review_rounds: Some("quick".into()),
    });
    config.projects.insert(
        "app".into(),
        serde_json::from_value(json!({"path": "app"})).unwrap(),
    );
    config.harnesses.insert(
        "h".into(),
        Harness {
            command: "true {model_args}".into(),
            flags: Some("claude".into()),
            model_flag: None,
            effort_flag: None,
            efforts: vec![],
            prompt: None,
        },
    );
    config.profiles.insert("rev".into(), reviewer("openai"));
    for kind in [ReviewContractKind::Plan, ReviewContractKind::Implementation] {
        config.routing.review_profiles.insert(kind, "rev".into());
    }
    store.save_config().unwrap();
    let ctx = Ctx {
        resource_probe: crate::resources::FixedResourceProbe::arc(),
        store,
        herdr: corral_herdr::Herdr::with_bin(bin, None),
    };
    let task = ctx
        .store
        .create("Review", Some("app"), Status::Review, "")
        .unwrap();
    let run: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "author", "vendor": "anthropic", "worktree": app,
        "branch": "main", "base": "main", "base_sha": git(&app, &["rev-parse", "HEAD"]),
        "started": "2026-10-23T00:00:00Z", "gate_rules": rules,
    }))
    .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    Fx {
        tmp,
        ctx,
        id: task.id,
    }
}

impl Fx {
    fn template(&self, text: &str) {
        let dir = self.ctx.store.prompts_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("review.md"), text).unwrap();
    }

    fn clear_template(&self) {
        std::fs::remove_file(self.ctx.store.prompts_dir().join("review.md")).unwrap();
    }

    fn instructions(&mut self, profile: &str, text: &str) {
        let metadata = self
            .ctx
            .store
            .config
            .profiles
            .get_mut(profile)
            .unwrap()
            .metadata
            .as_mut()
            .unwrap();
        metadata.instructions = text.into();
        self.ctx.store.save_config().unwrap();
    }

    fn configure(&mut self, edit: impl FnOnce(&mut corral_core::Config)) {
        edit(&mut self.ctx.store.config);
        self.ctx.store.save_config().unwrap();
    }

    fn created(&self) -> usize {
        std::fs::read_dir(self.tmp.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("created-")
            })
            .count()
    }

    fn run(&self) -> Run {
        self.ctx
            .store
            .get(&self.id)
            .unwrap()
            .current_run()
            .unwrap()
            .clone()
    }

    fn events(&self, kind: &str) -> Vec<serde_json::Value> {
        self.ctx
            .store
            .events()
            .unwrap()
            .into_iter()
            .filter(|e| e.task.as_deref() == Some(self.id.as_str()) && e.kind == kind)
            .map(|e| e.detail)
            .collect()
    }

    fn prompt_file(&self) -> PathBuf {
        self.ctx
            .store
            .prompts_dir()
            .join(format!("{}-R1-review.md", self.id))
    }

    /// The conflict, with nothing started: no pane, review record, child, reservation,
    /// prompt or review_started event.
    fn assert_refused(
        &self,
        error: &anyhow::Error,
        surface: &str,
        path: &str,
        line: usize,
        phrase: &str,
    ) {
        let conflict = error
            .downcast_ref::<GateInstructionConflict>()
            .unwrap_or_else(|| panic!("not a gate instruction conflict: {error:#}"));
        assert_eq!(conflict.surface, surface);
        assert!(conflict.path.ends_with(path), "{}", conflict.path);
        assert_eq!((conflict.line, conflict.phrase.as_str()), (line, phrase));
        assert_eq!(self.created(), 0);
        let task = self.ctx.store.get(&self.id).unwrap();
        assert!(task.review_reservation.is_none());
        let run = task.current_run().unwrap();
        assert!(run.review.is_none());
        assert!(run.review_runs.is_empty());
        assert!(!self.prompt_file().exists());
        assert!(self.events("review_started").is_empty());
    }

    fn grant(&mut self) {
        self.configure(|config| config.dispatch.paused = false);
        let selection = OperatingSelection {
            tasks: [self.id.clone()].into_iter().collect(),
            authority: OperatingAuthority {
                dispatch: true,
                accept: false,
                plan_accept: false,
                aggregate_accept: false,
            },
            ..OperatingSelection::default()
        };
        let preview = self.ctx.store.preview_operating_policy(&selection).unwrap();
        self.ctx
            .store
            .confirm_operating_policy(&preview, "owner", Timestamp::now())
            .unwrap();
        self.ctx.store.reload_config_if_changed().unwrap();
    }
}

#[test]
fn explicit_launch_refuses_the_review_template_and_profile_instructions_before_any_state() {
    // Given a tiered main-bound run under review.
    let mut fx = fixture(GateRules::Tiered);

    // When the review template override demands the full gate.
    fx.template(TEMPLATE_DIRECTIVE);
    let error = crate::reviewer::review_run(&fx.ctx, &fx.id, Some("rev")).unwrap_err();
    // Then the explicit launch is refused naming the file, line and phrase.
    fx.assert_refused(
        &error,
        "review_template",
        "review.md",
        4,
        "Run the full gate",
    );

    // When the profile's instructions name the gate script instead.
    fx.clear_template();
    fx.instructions("rev", PROFILE_DIRECTIVE);
    let error = crate::reviewer::review_run(&fx.ctx, &fx.id, Some("rev")).unwrap_err();
    // Then the refusal names the profile key.
    fx.assert_refused(
        &error,
        "reviewer_instructions",
        "profiles.rev.metadata.instructions",
        2,
        "scripts/gate.sh",
    );
}

#[test]
fn routed_and_cross_vendor_launches_take_the_same_guard_and_clean_profiles_launch() {
    // Given a routed reviewer whose instructions demand the full gate.
    let mut fx = fixture(GateRules::Tiered);
    fx.configure(|config| {
        config.review.reviewer = Some("auto".into());
        config
            .routing
            .reviewer_by_vendor
            .insert("openai".into(), "rev".into());
    });
    fx.instructions("rev", "Run the full gate.");

    // When the routed auto and the cross-vendor reviews launch.
    for reviewer in [None, Some("cross-vendor")] {
        let error = crate::reviewer::review_run(&fx.ctx, &fx.id, reviewer).unwrap_err();
        // Then each is refused at the shared boundary.
        fx.assert_refused(
            &error,
            "reviewer_instructions",
            "profiles.rev.metadata.instructions",
            1,
            "Run the full gate",
        );
    }

    // When the same profile has clean instructions.
    fx.instructions("rev", CLEAN);
    let record = crate::reviewer::review_run(&fx.ctx, &fx.id, None).unwrap();

    // Then it launches normally with one authoritative Gate contract section.
    assert_eq!(record.reviewer, "rev");
    assert_eq!(fx.created(), 1);
    let prompt = std::fs::read_to_string(fx.prompt_file()).unwrap();
    assert_eq!(
        prompt
            .matches("## Gate contract (review, authoritative)")
            .count(),
        1
    );
    assert!(prompt.contains("- Tier: quick"), "{prompt}");
    assert!(!prompt.contains("full-gate"), "{prompt}");
}

#[test]
fn automatic_and_fallback_launches_report_the_conflict_once() {
    // Given an automatic grant and a reviewer profile that names the gate script.
    let mut fx = fixture(GateRules::Tiered);
    fx.grant();
    fx.instructions("rev", PROFILE_DIRECTIVE);

    // When autopilot launches the review twice.
    for _ in 0..2 {
        let error =
            crate::reviewer::review_run_automatic(&fx.ctx, &fx.id, Some("rev")).unwrap_err();
        fx.assert_refused(
            &error,
            "reviewer_instructions",
            "profiles.rev.metadata.instructions",
            2,
            "scripts/gate.sh",
        );
    }

    // Then the block is reported once.
    let blocked = fx.events("automation_blocked");
    assert_eq!(blocked.len(), 1, "{blocked:?}");
    assert_eq!(blocked[0]["conflict"]["surface"], "reviewer_instructions");

    // When availability fallback continues an interrupted attempt of that profile.
    let task = fx.ctx.store.get(&fx.id).unwrap();
    let run = task.current_run().unwrap().clone();
    let mut child = run.clone();
    child.id = "R1-review1".into();
    child.agent = "rev".into();
    crate::reviewer::continue_review(
        &fx.ctx,
        &task,
        &run,
        &child,
        "provider failed",
        Timestamp::now(),
    )
    .unwrap();

    // Then the fallback is refused before any pane and escalates naming the conflict.
    assert_eq!(fx.created(), 0);
    let task = fx.ctx.store.get(&fx.id).unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    let note = task.current_run().unwrap().blocked_excerpt.clone().unwrap();
    assert!(note.contains("gate instruction conflict"), "{note}");
    assert!(
        note.contains("profiles.rev.metadata.instructions"),
        "{note}"
    );
    assert_eq!(fx.events("automation_blocked").len(), 1);
}

#[test]
fn restored_launch_is_refused_before_resume_and_its_preparation_is_retained() {
    // Given a never-submitted reviewer attempt prepared while the bound profile's
    // instructions already named the gate script (a preparation this guard did not see).
    let mut fx = fixture(GateRules::Tiered);
    fx.instructions("rev", PROFILE_DIRECTIVE);
    let mut launch =
        ReviewLaunch::reserve_origin(&fx.ctx, &fx.id, Kind::Reviewer, false, Origin::Owner)
            .unwrap();
    let task = fx.ctx.store.get(&fx.id).unwrap();
    let run = task.current_run().unwrap().clone();
    let candidate =
        crate::verification::capture_identity(&fx.ctx, &task, &run, "candidate").unwrap();
    launch
        .bind(&candidate, "R1-review1", Some("R1-review1"))
        .unwrap();
    launch.attach("w1:prepared", true).unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            let run = task.current_run_mut().unwrap();
            let record = ReviewRecord {
                route: None,
                identity: Some(candidate.clone()),
                evidence_digest: Some(crate::verification::evidence_digest(run)?),
                verdict: None,
                process_lost: false,
                reviewer: "rev".into(),
                notes: None,
                sha: candidate.head.clone(),
                at: Timestamp::now(),
                pane: Some("w1:prepared".into()),
            };
            let mut child = run.clone();
            child.id = "R1-review1".into();
            child.agent = "rev".into();
            child.review = Some(record.clone());
            child.review_runs.clear();
            run.review = Some(record);
            run.review_runs.push(child);
            Ok(())
        })
        .unwrap();
    std::fs::create_dir_all(fx.ctx.store.prompts_dir()).unwrap();
    std::fs::write(fx.prompt_file(), "prepared").unwrap();
    std::fs::write(fx.tmp.path().join("w1:prepared"), "").unwrap();
    drop(launch);
    let retained = fx
        .ctx
        .store
        .state_dir()
        .join("review-prepared")
        .join(format!("{}-R1-review.json", fx.id));
    assert!(retained.is_file());

    // When the launch is resumed.
    let error = crate::reviewer::review_run(&fx.ctx, &fx.id, Some("rev")).unwrap_err();

    // Then it is refused before resubmission, and the exact preparation stays retained.
    let conflict = error
        .downcast_ref::<GateInstructionConflict>()
        .unwrap_or_else(|| panic!("not a gate instruction conflict: {error:#}"));
    assert_eq!(
        (conflict.surface.as_str(), conflict.path.as_str()),
        (
            "reviewer_instructions",
            "profiles.rev.metadata.instructions"
        )
    );
    assert!(retained.is_file());
    assert_eq!(
        std::fs::read_to_string(fx.prompt_file()).unwrap(),
        "prepared"
    );
    assert!(fx.events("review_started").is_empty());
    assert!(
        fx.ctx
            .store
            .get(&fx.id)
            .unwrap()
            .review_reservation
            .is_none()
    );
}

#[test]
fn legacy_run_keeps_todays_review_gate_text_and_is_never_refused() {
    // Given a legacy run in a tiered project whose template and profile demand the full gate.
    let mut fx = fixture(GateRules::Legacy);
    fx.template(TEMPLATE_DIRECTIVE);
    fx.instructions("rev", PROFILE_DIRECTIVE);

    // When its review launches.
    let record = crate::reviewer::review_run(&fx.ctx, &fx.id, Some("rev")).unwrap();

    // Then it launches with today's gate text and no Gate contract section or packet object.
    assert_eq!(record.reviewer, "rev");
    assert_eq!(fx.created(), 1);
    let prompt = std::fs::read_to_string(fx.prompt_file()).unwrap();
    let run = fx.run();
    let today =
        super::super::prompt_text::gate_text(&fx.ctx, &fx.ctx.store.get(&fx.id).unwrap(), &run);
    assert!(
        prompt.contains(&format!("\n\n{today}\nRun the full gate")),
        "{prompt}"
    );
    assert!(!prompt.contains("## Gate contract"), "{prompt}");
    let child = run.review_runs.last().unwrap();
    let packet = fx
        .ctx
        .store
        .prompts_dir()
        .join(format!("{}-R1-{}-review-packet.json", fx.id, child.id));
    let packet: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(packet).unwrap()).unwrap();
    assert!(packet["bound"].get("gate_contract").is_none(), "{packet}");
    assert!(fx.events("automation_blocked").is_empty());
}

#[test]
fn implementation_family_contracts_are_v3_and_carry_no_full_gate_directive() {
    use ReviewContractKind as K;
    for kind in [
        K::Implementation,
        K::ComplexImplementation,
        K::VisualImplementation,
        K::TestOnly,
    ] {
        assert_eq!(corral_core::review_contract_version(kind), 3, "{kind:?}");
        let text = crate::review_contracts::contract_text(kind);
        assert_eq!(
            scan_gate_instructions(text, &["full-gate".to_owned()]),
            None
        );
    }
}
