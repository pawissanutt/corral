//! Instruction-surface drift: every shipped agent-facing instruction (the embedded assets and the
//! rendered skills) must hold no hardcoded full-gate directive, because a tiered run takes its gate
//! only from corral's Gate contract. The surfaces rendered for a reduced-tier task carry the tier
//! commands and none of the full-tier commands.
#![cfg(unix)] // The rendered scenario executes the symlinked POSIX fake engine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use corral_core::{Accept, AgentCmd, AutoStage, GateResult, Status, Store, TierPolicy};
use corral_herdr::Herdr;
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome, scan_gate_instructions};
use serde_json::Value;

mod support;

/// One shipped instruction surface: where a reader finds it and the text agents receive.
struct Surface {
    name: String,
    text: String,
}

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")
}

fn markdown_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            markdown_under(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            out.push(path);
        }
    }
}

/// Every Markdown file under assets/ (found by walking, so a new surface is registered the moment
/// it exists) and every role skill as installed, with its generated sections filled in.
fn registry() -> Vec<Surface> {
    let mut files = Vec::new();
    markdown_under(&assets(), &mut files);
    let mut surfaces: Vec<Surface> = files
        .iter()
        .map(|path| Surface {
            name: format!("assets/{}", path.strip_prefix(assets()).unwrap().display()),
            text: std::fs::read_to_string(path).unwrap(),
        })
        .collect();
    for role in [
        corral_ops::PM_SKILL,
        corral_ops::WORKER_SKILL,
        corral_ops::PLANNER_SKILL,
        corral_ops::SUBMIT_SKILL,
    ] {
        surfaces.push(Surface {
            name: format!("skill:{role}"),
            text: corral_ops::role_skill_markdown(role).unwrap(),
        });
    }
    surfaces
}

/// "<surface> line <n>: <phrase>" for each surface that tells an agent to run the full gate.
fn drift(surfaces: &[Surface]) -> Vec<String> {
    surfaces
        .iter()
        .filter_map(|surface| {
            scan_gate_instructions(&surface.text, &[])
                .map(|(line, phrase)| format!("{} line {line}: {phrase:?}", surface.name))
        })
        .collect()
}

fn with_appended(mut surfaces: Vec<Surface>, name: &str, line: &str) -> (Vec<Surface>, usize) {
    let surface = surfaces.iter_mut().find(|s| s.name == name).unwrap();
    if !surface.text.ends_with('\n') {
        surface.text.push('\n');
    }
    let at = surface.text.lines().count() + 1;
    surface.text.push_str(line);
    surface.text.push('\n');
    (surfaces, at)
}

#[test]
fn no_shipped_surface_hardcodes_the_full_gate() {
    let surfaces = registry();
    // The registry reaches the surfaces that tell agents about the gate.
    let names: Vec<&str> = surfaces.iter().map(|s| s.name.as_str()).collect();
    for expected in [
        "assets/run.md",
        "assets/review.md",
        "assets/SKILL.md",
        "assets/pm-runbook.md",
        "assets/skills/corral-worker.md",
        "assets/skills/corral-planner.md",
        "assets/review-contracts/common.md",
        "assets/review-contracts/implementation.md",
        "skill:corral-worker",
    ] {
        assert!(
            names.contains(&expected),
            "{expected} unregistered: {names:?}"
        );
    }
    assert_eq!(drift(&surfaces), Vec::<String>::new());
}

#[test]
fn a_reintroduced_full_gate_directive_fails_naming_the_surface_and_line() {
    // Given 'Run the full gate' put back in run.md and 'scripts/gate.sh' in a skill.
    let (surfaces, run_line) = with_appended(registry(), "assets/run.md", "Run the full gate.");
    let (surfaces, skill_line) = with_appended(
        surfaces,
        "assets/skills/corral-worker.md",
        "Before done, run scripts/gate.sh.",
    );

    // Then the drift check names each surface and the line.
    assert_eq!(
        drift(&surfaces),
        [
            format!("assets/run.md line {run_line}: \"Run the full gate\""),
            format!("assets/skills/corral-worker.md line {skill_line}: \"scripts/gate.sh\""),
        ]
    );
}

#[test]
fn an_explicitly_reduced_gate_invocation_is_not_drift() {
    let (surfaces, _) = with_appended(
        registry(),
        "assets/skills/corral-worker.md",
        "Corral runs scripts/gate.sh --tier quick for you.",
    );
    assert_eq!(drift(&surfaces), Vec::<String>::new());
}

const QUICK_MARKER: &str = "quick-tier-command";
const FULL_MARKER: &str = "full-tier-command";
const SUMMARY: &str = "Implemented the widget.\nVerified: corral gate\nNot verified: none\n";

fn git(dir: &Path, args: &[&str]) {
    let out = support::command("git", dir)
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A main-bound task dispatched in a project whose review rounds run the quick tier.
struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    out: PathBuf,
    id: String,
}

impl Fx {
    fn new() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("fake-herdr");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/autopilot_ops/herdr.sh"),
            bin_dir.join("herdr"),
        )
        .unwrap();
        let app = tmp.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        git(&app, &["init", "-q", "-b", "main"]);
        git(&app, &["config", "user.name", "corral test"]);
        git(&app, &["config", "user.email", "corral@test.invalid"]);
        std::fs::write(app.join("README.md"), "hello\n").unwrap();
        git(&app, &["add", "-A"]);
        git(&app, &["commit", "-q", "-m", "init"]);
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let mut store = Store::init(tmp.path()).unwrap();
        for name in ["fake", "rev"] {
            store.config.agents.insert(
                name.into(),
                AgentCmd {
                    command: "true".into(),
                },
            );
        }
        let review = &mut store.config.review;
        review.reviewer = Some("rev".into());
        review.accept = Some(Accept::Owner);
        review.remove_worktree = Some(false);
        review.gate = Some(vec![format!(
            "echo {FULL_MARKER} >> {}/runs",
            out.display()
        )]);
        review.gate_tiers = Some(BTreeMap::from([(
            "quick".to_owned(),
            vec![format!(
                "echo {QUICK_MARKER} >> {out}/runs && test ! -e {out}/quick-fails",
                out = out.display()
            )],
        )]));
        review.tier_policy = Some(TierPolicy {
            plan_member: None,
            review_rounds: Some("quick".into()),
        });
        store.save_config().unwrap();
        let ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin_dir.join("herdr"), None),
        };
        let task = ctx
            .store
            .create(
                "Ship the widget",
                Some("app"),
                Status::Ready,
                "## Goal\nShip it.\n",
            )
            .unwrap();
        let preview = ctx
            .store
            .preview_operating_policy(&corral_core::OperatingSelection {
                tasks: [task.id.clone()].into(),
                authority: corral_core::OperatingAuthority {
                    dispatch: true,
                    accept: true,
                    plan_accept: true,
                    aggregate_accept: false,
                },
                ..Default::default()
            })
            .unwrap();
        let mut fx = Fx {
            _tmp: tmp,
            ctx,
            out,
            id: task.id.clone(),
        };
        fx.ctx.store.config = fx
            .ctx
            .store
            .confirm_operating_policy(&preview, "fixture owner", jiff::Timestamp::now())
            .unwrap();
        let opts = DispatchOpts {
            profile: Some("fake".into()),
            ..DispatchOpts::default()
        };
        let started = corral_ops::dispatch(&fx.ctx, &task.id, &opts).unwrap();
        assert!(matches!(started, DispatchOutcome::Started { .. }));
        fx
    }

    fn task(&self) -> corral_core::Task {
        self.ctx.store.get(&self.id).unwrap()
    }

    fn sync(&self) {
        corral_ops::sync_at(&self.ctx, "2026-10-15T09:00:00Z".parse().unwrap()).unwrap();
    }

    /// Commits a change and submits it.
    fn submit(&self, file: &str) {
        let task = self.task();
        let run = task.current_run().unwrap();
        std::fs::write(run.worktree.join(file), "w\n").unwrap();
        corral_git::commit_all(&run.worktree, file).unwrap();
        let identity = corral_ops::progress_readback(&self.ctx, &self.id).unwrap();
        corral_ops::save_checkpoint(
            &self.ctx,
            &self.id,
            &corral_core::CheckpointUpdate {
                task: identity.task,
                run: identity.run,
                spec_revision: identity.spec_revision,
                expected_revision: identity
                    .checkpoints
                    .last()
                    .map_or(0, |r| r.checkpoint.revision),
                progress_revision: identity.progress.as_ref().map_or(0, |p| p.revision),
                head: identity.head,
                base: identity.base,
                tree: identity.tree,
                criterion_ids: vec![],
                evidence_refs: vec![],
                next_action: "Review the candidate".into(),
                decisions: vec![],
                waits: vec![],
                resources: vec![],
            },
        )
        .unwrap();
        corral_ops::done(&self.ctx, &self.id, SUMMARY).unwrap();
    }

    fn prompts_matching(&self, suffix: &str) -> String {
        let prefix = format!("{}-", self.id);
        let mut found: Vec<PathBuf> = std::fs::read_dir(self.ctx.store.prompts_dir())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                let name = p.file_name().unwrap().to_string_lossy();
                name.starts_with(&prefix) && name.ends_with(suffix)
            })
            .collect();
        found.sort();
        let path = found
            .last()
            .unwrap_or_else(|| panic!("no *{suffix} prompt file for {}", self.id));
        std::fs::read_to_string(path).unwrap()
    }
}

/// A rendered surface names the tier's commands and none of the full tier's.
fn assert_reduced(surface: &str, text: &str) {
    assert!(
        text.contains(QUICK_MARKER),
        "{surface} lacks the tier commands:\n{text}"
    );
    assert!(
        !text.contains(FULL_MARKER),
        "{surface} leaks a full-tier command:\n{text}"
    );
}

#[test]
fn surfaces_rendered_for_a_reduced_tier_task_carry_the_tier_commands_and_no_full_command() {
    // Given a main-bound task dispatched with quick review rounds, so its worker prompt is rendered.
    let fx = Fx::new();
    let worker = fx.prompts_matching("-R1.md");
    assert_reduced("worker prompt", &worker);

    // When its quick gate fails and autopilot sends the change request.
    std::fs::write(fx.out.join("quick-fails"), "").unwrap();
    fx.submit("round-1.txt");
    fx.sync();
    fx.sync();
    let task = fx.task();
    assert_eq!(
        task.current_run()
            .unwrap()
            .autopilot
            .as_ref()
            .map(|a| a.stage),
        Some(AutoStage::Changes)
    );
    let request = task
        .artifacts
        .iter()
        .rev()
        .find(|a| a.title == "Autopilot changes (round 1)")
        .expect("change request artifact");
    let request = std::fs::read_to_string(fx.ctx.store.dir().join(&request.path)).unwrap();
    assert_reduced("change request", &request);

    // When its review goes stale, the PM send-back.
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.status = Status::Review;
            t.current_run_mut().unwrap().review = Some(
                serde_json::from_value(serde_json::json!({
                    "reviewer":"rev","sha":"old","at":"2026-10-15T12:00:00Z","verdict":"approve"
                }))
                .unwrap(),
            );
            Ok(())
        })
        .unwrap();
    let card = corral_ops::pm_next(&fx.ctx).unwrap();
    assert_eq!(
        card.kind,
        corral_ops::PmActionKind::StaleVerdict,
        "{card:?}"
    );
    assert_reduced("PM send-back", &card.evidence.join("\n"));
}

#[test]
fn the_review_prompt_and_packet_of_a_reduced_tier_task_carry_the_tier_commands_and_no_full_command()
{
    // Given a quick-gated submission that autopilot sends to review.
    let fx = Fx::new();
    fx.submit("round-1.txt");
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    let gate = corral_ops::gate_wait(&fx.ctx, &fx.id).unwrap();
    assert_eq!(gate.result, GateResult::Pass, "{:?}", gate.failed_cmd);
    fx.sync();

    // Then the review prompt and the packet's gate contract name the tier commands only.
    assert_reduced("review prompt", &fx.prompts_matching("-review.md"));
    let packet: Value = serde_json::from_str(&fx.prompts_matching("-review-packet.json")).unwrap();
    assert!(packet["bound"]["gate_contract"].is_object(), "{packet}");
    assert_reduced(
        "review packet",
        &packet["bound"]["gate_contract"].to_string(),
    );
}
