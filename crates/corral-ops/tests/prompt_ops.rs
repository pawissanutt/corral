//! The dispatch prompt's run contract (template, overrides, gate sources) and the dispatch-time skill install.

use std::path::{Path, PathBuf};
mod support;
#[path = "prompt_ops/tiers.rs"]
mod tiers;

use corral_core::{Harness, Run, RunMode, Status, Store, Task};
use corral_ops::Ctx;

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

fn workspace() -> (tempfile::TempDir, Ctx) {
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir_all(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    let store = Store::init(tmp.path()).unwrap();
    (
        tmp,
        Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            herdr: corral_herdr::Herdr::with_bin(
                store.root().join("unused-herdr"),
                Some("corral-test".into()),
            ),
            store,
        },
    )
}

fn task(ctx: &Ctx) -> Task {
    ctx.store
        .create("Add login", Some("app"), Status::Ready, "Spec body here.\n")
        .unwrap()
}

fn run(root: &Path, harness: Option<&str>) -> Run {
    Run {
        execution: None,
        delivery: corral_core::inbox::DeliveryMode::Legacy,
        inbox: Default::default(),
        completion: Default::default(),
        id: "R1".into(),
        progress: None,
        checkpoints: vec![],
        checkpoint_required: false,
        agent: "omp".into(),
        profile: None,
        harness: harness.map(str::to_string),
        model: None,
        effort: None,
        route: None,
        vendor: None,
        failures: 0,
        switches: vec![],
        retry: None,
        mode: RunMode::Interactive,
        worktree: root.join("app/.worktrees/T-001"),
        branch: "corral/T-001".into(),
        base: "main".into(),
        base_sha: "0".repeat(40),
        plan_target: None,
        herdr: None,
        started: jiff::Timestamp::now(),
        ended: None,
        outcome: None,
        summary: None,
        blocked_excerpt: None,
        agent_state: None,
        seen_pane: false,
        seen_working: false,
        gate: None,
        idle_since: None,
        idle_no_report: false,
        review: None,
        review_runs: Vec::new(),
        usage: None,
        usage_note: None,
        herdr_idle_since: None,
        stuck: None,
        pending_reply: None,
        obligations: Default::default(),
        autopilot: None,
        gate_rules: corral_core::GateRules::Legacy,
        promotion_origin: None,
        agent_host: None,
    }
}

fn corral() -> String {
    std::env::current_exe().unwrap().display().to_string()
}

#[test]
fn contract_renders_review_gate_artifact_command_and_placeholders() {
    let (tmp, mut ctx) = workspace();
    ctx.store.config.review.gate = Some(vec!["cargo fmt --check".into(), "cargo test".into()]);
    let task = task(&ctx);
    let prompt = corral_ops::task_prompt(&ctx, &task, &run(tmp.path(), Some("omp"))).unwrap();
    let id = &task.id;
    let corral = corral();
    assert!(
        prompt.starts_with(&format!("# {id} Add login\n")),
        "{prompt}"
    );
    assert!(prompt.contains("Spec body here."));
    assert!(prompt.contains("branch corral/T-001 from main"));
    assert!(prompt.contains(&tmp.path().join("app/AGENTS.md").display().to_string()));
    assert!(prompt.contains("corral's [review] gate"));
    assert!(prompt.contains("       cargo fmt --check\n       cargo test"));
    assert!(prompt.contains(&format!(
        "{corral} artifact add {id} <path> --kind <report|plan|evidence|screenshot|log> --title \"<title>\""
    )));
    assert!(prompt.contains(&format!(
        "{corral} done {id} --summary \"<activity and verification>\" --result \"<cumulative task outcome>\""
    )));
    assert!(prompt.contains("`.omo/evidence/**`"));
    assert!(prompt.contains("`Verified:`") && prompt.contains("`Not verified:"));
    assert!(!prompt.contains("{{"), "{prompt}");
}

#[test]
fn run_contract_gate_commands_execute_with_project_environment() {
    // Given a minimal template and project-local environment with shell-sensitive values.
    let (tmp, mut ctx) = workspace();
    let value = "project's $input; $(false)";
    ctx.store.config.review.gate = Some(vec![
        r#"printf '%s\0%s' "$GATE_INPUT" "$CORRAL_HERDR_SESSION" > local.env"#.into(),
        r#"printf '%s' "$CORRAL_SIZE_LIMITS" > limits.json"#.into(),
    ]);
    ctx.store.config.projects.get_mut("app").unwrap().review.env = Some(
        [
            ("GATE_INPUT".into(), value.into()),
            ("CORRAL_HERDR_SESSION".into(), "corral-test".into()),
        ]
        .into(),
    );
    std::fs::create_dir_all(ctx.store.prompts_dir()).unwrap();
    std::fs::write(
        ctx.store.prompts_dir().join("run.md"),
        "{{gate}}\nEND_GATE\n",
    )
    .unwrap();
    let task = task(&ctx);

    // When an agent copies the run contract's shell block into its local gate shell.
    let prompt = corral_ops::task_prompt(&ctx, &task, &run(tmp.path(), None)).unwrap();
    let contract = prompt.split("\nEND_GATE\n").next().unwrap();
    let commands = contract.split_once("\n\n").unwrap().1;
    let output = support::command("bash", &tmp.path().join("app"))
        .current_dir(tmp.path().join("app"))
        .args(["-c", commands])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();

    // Then the shell uses the same bytes as the configured automated gate.
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read(tmp.path().join("app/local.env")).unwrap(),
        format!("{value}\0corral-test").as_bytes()
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(tmp.path().join("app/limits.json")).unwrap()
        )
        .unwrap(),
        serde_json::json!({"rs": 800})
    );
}

#[test]
fn contract_falls_back_to_the_gate_quoted_in_agents_md() {
    let (tmp, ctx) = workspace();
    std::fs::write(
        tmp.path().join("app/AGENTS.md"),
        "# app\n\nSee `docs/`.\n\n- Full gate: `just check && just test`.\n",
    )
    .unwrap();
    let task = task(&ctx);
    let prompt = corral_ops::task_prompt(&ctx, &task, &run(tmp.path(), None)).unwrap();
    assert!(prompt.contains("quoted in AGENTS.md"), "{prompt}");
    assert!(
        prompt.contains("       just check && just test\n"),
        "{prompt}"
    );

    std::fs::write(
        tmp.path().join("app/AGENTS.md"),
        "Full CHECK gates:\n\n```bash\ncargo fmt --all -- --check\n# comment\ncargo nextest run\n```\n",
    )
    .unwrap();
    let prompt = corral_ops::task_prompt(&ctx, &task, &run(tmp.path(), None)).unwrap();
    assert!(
        prompt.contains("       cargo fmt --all -- --check\n       cargo nextest run\n"),
        "{prompt}"
    );

    std::fs::remove_file(tmp.path().join("app/AGENTS.md")).unwrap();
    let prompt = corral_ops::task_prompt(&ctx, &task, &run(tmp.path(), None)).unwrap();
    assert!(prompt.contains("No gate is configured"), "{prompt}");
}

#[test]
fn project_and_harness_templates_override_the_builtin() {
    let (tmp, mut ctx) = workspace();
    let task = task(&ctx);
    let prompts = ctx.store.prompts_dir();
    std::fs::create_dir_all(&prompts).unwrap();
    std::fs::write(
        prompts.join("run.md"),
        "{\"source\":\"project\",\"task\":\"{{id}}\",\"branch\":\"{{branch}}\"}\n",
    )
    .unwrap();
    let r = run(tmp.path(), Some("omp"));
    let prompt = corral_ops::task_prompt(&ctx, &task, &r).unwrap();
    let header = |prompt: &str| -> serde_json::Value {
        serde_json::from_str(prompt.lines().next().unwrap()).unwrap()
    };
    assert_eq!(
        header(&prompt),
        serde_json::json!({
            "source":"project", "task":task.id, "branch":"corral/T-001"
        })
    );

    std::fs::write(
        tmp.path().join("omp-run.md"),
        "{\"source\":\"omp\",\"task\":\"{{id}}\",\"binary\":\"{{corral}}\"}\n",
    )
    .unwrap();
    ctx.store.config.harnesses.insert(
        "omp".into(),
        Harness {
            command: "omp {model_args} \"$(cat '{prompt_file}')\"".into(),
            flags: None,
            model_flag: None,
            effort_flag: None,
            efforts: Vec::new(),
            prompt: Some("omp-run.md".into()),
        },
    );
    let prompt = corral_ops::task_prompt(&ctx, &task, &r).unwrap();
    assert_eq!(
        header(&prompt),
        serde_json::json!({
            "source":"omp", "task":task.id, "binary":corral()
        })
    );
    // Another harness still gets the project template.
    let prompt = corral_ops::task_prompt(&ctx, &task, &run(tmp.path(), Some("claude"))).unwrap();
    assert_eq!(header(&prompt)["source"], "project");
}

fn skill_file(home: &Path, skills: &str, role: &str) -> PathBuf {
    home.join(skills).join(role).join("SKILL.md")
}

fn actions(writes: &[corral_ops::SkillWrite]) -> Vec<(&'static str, corral_ops::SkillAction)> {
    writes.iter().map(|w| (w.role, w.action)).collect()
}

#[test]
fn fresh_install_writes_every_role_for_installed_harnesses_only() {
    use corral_ops::{ROLE_SKILLS, SKILL_HARNESSES, SkillAction};
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::create_dir_all(home.join(".omp")).unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    for (harness, marker, skills) in SKILL_HARNESSES {
        let writes = corral_ops::ensure_skills(home, harness).unwrap();
        if home.join(marker).is_dir() {
            let expected: Vec<_> = ROLE_SKILLS
                .iter()
                .map(|r| (*r, SkillAction::Wrote))
                .collect();
            assert_eq!(actions(&writes), expected, "{harness}");
            for role in ROLE_SKILLS {
                let text = std::fs::read_to_string(skill_file(home, skills, role)).unwrap();
                assert_eq!(Some(text), corral_ops::role_skill_markdown(role));
                assert!(
                    writes
                        .iter()
                        .any(|w| w.path == skill_file(home, skills, role))
                );
            }
        } else {
            assert!(writes.is_empty(), "{harness}");
            assert!(!home.join(marker).exists(), "{harness}");
        }
    }
    assert!(
        corral_ops::ensure_skills(home, "generic")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn role_skills_carry_their_name_version_and_digest() {
    for role in corral_ops::ROLE_SKILLS {
        let text = corral_ops::role_skill_markdown(role).unwrap();
        assert!(text.starts_with(&format!("---\nname: {role}\n")), "{role}");
        assert!(text.contains(corral_ops::SKILL_MARKER));
        let stamp = format!(
            "<!-- corral-skill: {role} version={} sha256=",
            env!("CARGO_PKG_VERSION")
        );
        assert!(text.contains(&stamp), "{role}");
        assert!(!text.contains("{{"), "{role} has an unfilled placeholder");
    }
    assert_eq!(
        corral_ops::skill_markdown(),
        corral_ops::role_skill_markdown(corral_ops::PM_SKILL).unwrap()
    );
}

#[test]
fn stale_managed_skill_is_refreshed_and_current_and_unmanaged_are_kept() {
    use corral_ops::{SKILL_MARKER, SkillAction, WORKER_SKILL};
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::create_dir_all(home.join(".omp")).unwrap();
    corral_ops::ensure_skills(home, "omp").unwrap();
    let skills = ".omp/agent/skills";
    // An older binary's managed copy, an unmanaged PM file, and current copies of the rest.
    let stale = format!(
        "---\nname: {WORKER_SKILL}\n---\n{SKILL_MARKER}\n<!-- corral-skill: {WORKER_SKILL} version=0.0.1 sha256=00 -->\nold\n"
    );
    std::fs::write(skill_file(home, skills, WORKER_SKILL), &stale).unwrap();
    std::fs::write(skill_file(home, skills, "corral"), b"my own \xff skill").unwrap();
    let planner = skill_file(home, skills, corral_ops::PLANNER_SKILL);
    let before = std::fs::metadata(&planner).unwrap().modified().unwrap();

    let writes = corral_ops::ensure_skills(home, "omp").unwrap();
    assert_eq!(
        actions(&writes),
        vec![
            ("corral", SkillAction::Unmanaged),
            (WORKER_SKILL, SkillAction::Refreshed),
            (corral_ops::PLANNER_SKILL, SkillAction::Current),
            (corral_ops::SUBMIT_SKILL, SkillAction::Current),
        ]
    );
    assert_eq!(
        std::fs::read(skill_file(home, skills, "corral")).unwrap(),
        b"my own \xff skill"
    );
    assert_eq!(
        Some(std::fs::read_to_string(skill_file(home, skills, WORKER_SKILL)).unwrap()),
        corral_ops::role_skill_markdown(WORKER_SKILL)
    );
    assert_eq!(
        std::fs::metadata(&planner).unwrap().modified().unwrap(),
        before
    );

    // Forcing replaces the foreign file.
    let forced = corral_ops::install_role_skills(&home.join(skills), true).unwrap();
    assert_eq!(forced[0].action, SkillAction::Refreshed);
    assert_eq!(
        Some(std::fs::read_to_string(skill_file(home, skills, "corral")).unwrap()),
        corral_ops::role_skill_markdown("corral")
    );
}

#[test]
fn run_prompt_names_the_worker_skill() {
    let (tmp, ctx) = workspace();
    let task = task(&ctx);
    let prompt = corral_ops::task_prompt(&ctx, &task, &run(tmp.path(), Some("omp"))).unwrap();
    assert!(
        prompt.contains(&format!("`{}`", corral_ops::WORKER_SKILL)),
        "{prompt}"
    );
}
