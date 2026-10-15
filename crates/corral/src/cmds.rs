use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use corral_core::{
    ArtifactKind, GateRecord, GateResult, ProfileChoice, ReviewRecord, Run, RunMode, Status, Store,
    Task, Verdict,
};
use corral_ops::{ApproveStrategy, Ctx, DispatchOpts, DispatchOutcome};
use corral_tui::Mode;

use crate::{
    ArtifactCmd, Cli, Cmd, HarnessCmd, PluginCmd, ProfileArgs, SkillCmd, config_cmd, harness, pins,
    plugin, skill,
};

impl ProfileArgs {
    fn choice(self) -> ProfileChoice {
        ProfileChoice {
            profile: self.profile,
            model: self.model,
            effort: self.effort,
        }
    }
}

/// "profile omo, harness omo, model opus, effort high" (absent parts omitted).
fn run_profile(r: &Run) -> String {
    let mut parts = vec![format!("profile {}", r.agent)];
    for (label, value) in [
        ("harness", &r.harness),
        ("model", &r.model),
        ("effort", &r.effort),
    ] {
        if let Some(v) = value {
            parts.push(format!("{label} {v}"));
        }
    }
    parts.join(", ")
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// "pass at 1a2b3c4", "fail at 1a2b3c4: cargo test", "running in pane w1:p2 at 1a2b3c4", plus
/// " (stale: HEAD is now 5d6e7f8)" when the worktree moved past the checked commit.
fn gate_line(g: &GateRecord, run: &Run) -> String {
    let sha = short_sha(&g.sha);
    let mut line = match g.result {
        GateResult::Running => format!(
            "running in pane {} at {sha}",
            g.pane.as_deref().unwrap_or("?")
        ),
        GateResult::Pass => format!("pass at {sha} ({})", g.at),
        GateResult::Fail => format!(
            "fail at {sha} ({}): {}",
            g.at,
            g.failed_cmd.as_deref().unwrap_or("?")
        ),
    };
    if let Some(head) = corral_ops::worktree_head(run)
        && head != g.sha
    {
        line.push_str(&format!(" (stale: HEAD is now {})", short_sha(&head)));
    }
    line
}

fn gate(ctx: &Ctx, id: &str, wait: bool) -> Result<()> {
    let started = corral_ops::gate_start(ctx, id)?;
    let task = ctx.store.get(id)?;
    println!(
        "{} gate started in pane {} at {}",
        task.id,
        started.pane.as_deref().unwrap_or("?"),
        short_sha(&started.sha)
    );
    if !wait {
        return Ok(());
    }
    let record = corral_ops::gate_wait(ctx, id)?;
    let task = ctx.store.get(id)?;
    let run = task
        .current_run()
        .ok_or_else(|| anyhow!("{} has no run", task.id))?;
    println!("{} gate: {}", task.id, gate_line(&record, run));
    if record.result == GateResult::Fail {
        bail!("gate failed for {}", task.id);
    }
    Ok(())
}

/// "approve by opus-rev at 1a2b3c4 (<at>): <first notes line>" or "in progress by pm in pane w1:p3 at 1a2b3c4".
fn review_line(r: &ReviewRecord) -> String {
    let sha = short_sha(&r.sha);
    match r.verdict {
        None => format!(
            "in progress by {} in pane {} at {sha}",
            r.reviewer,
            r.pane.as_deref().unwrap_or("?")
        ),
        Some(v) => {
            let first = r
                .notes
                .as_deref()
                .and_then(|n| n.lines().find(|l| !l.trim().is_empty()))
                .unwrap_or("");
            format!(
                "{} by {} at {sha} ({}): {first}",
                v.as_str(),
                r.reviewer,
                r.at
            )
        }
    }
}

fn review_run(ctx: &Ctx, id: &str, reviewer: Option<&str>) -> Result<()> {
    let record = corral_ops::review_run(ctx, id, reviewer)?;
    let task = ctx.store.get(id)?;
    println!(
        "{} review by {} started in pane {} at {}",
        task.id,
        record.reviewer,
        record.pane.as_deref().unwrap_or("?"),
        short_sha(&record.sha)
    );
    Ok(())
}

fn verdict(
    ctx: &Ctx,
    id: &str,
    verdict: &str,
    notes: Option<String>,
    notes_file: Option<PathBuf>,
) -> Result<()> {
    let verdict = Verdict::parse(verdict)
        .ok_or_else(|| anyhow!("unknown verdict '{verdict}' (approve, changes, escalate)"))?;
    let notes = match (notes, notes_file) {
        (Some(n), _) => n,
        (None, Some(f)) => {
            std::fs::read_to_string(&f).with_context(|| format!("reading {}", f.display()))?
        }
        (None, None) => bail!("pass --notes or --notes-file"),
    };
    let record = corral_ops::verdict(ctx, id, verdict, &notes)?;
    let task = ctx.store.get(id)?;
    println!("{} review: {}", task.id, review_line(&record));
    Ok(())
}

pub(crate) fn start_dir(root: &Option<PathBuf>) -> Result<PathBuf> {
    match root {
        Some(r) => Ok(r.clone()),
        None => std::env::current_dir().context("reading the current directory"),
    }
}

fn open(root: &Option<PathBuf>) -> Result<Ctx> {
    Ctx::open(&start_dir(root)?)
}

fn task_json(t: &Task, with_body: bool) -> serde_json::Value {
    let mut v = serde_json::to_value(t).unwrap_or(serde_json::Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("path".into(), t.path.to_string_lossy().into_owned().into());
        if with_body {
            o.insert("body".into(), t.body.clone().into());
        }
    }
    v
}

fn parse_status(s: &str) -> Result<Status> {
    Status::parse(s).ok_or_else(|| {
        anyhow!(
            "unknown status '{s}' (idea, backlog, ready, running, needs_me, review, done, dropped)"
        )
    })
}

fn print_outcome(o: DispatchOutcome) {
    match o {
        DispatchOutcome::Started {
            task,
            run_id,
            pane_id,
        } => {
            println!("{} started as {run_id} in pane {pane_id}", task.id)
        }
        DispatchOutcome::Queued { task, reason } => println!("{} queued: {reason}", task.id),
    }
}

fn find_cwd(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(m) => {
            for key in ["cwd", "foreground_cwd"] {
                if let Some(serde_json::Value::String(s)) = m.get(key)
                    && !s.is_empty()
                {
                    return Some(s.clone());
                }
            }
            m.values().find_map(find_cwd)
        }
        serde_json::Value::Array(a) => a.iter().find_map(find_cwd),
        _ => None,
    }
}

fn tui(ctx: Ctx, mode: Mode, no_mouse: bool) -> Result<()> {
    let env = std::env::var("CORRAL_MOUSE").ok();
    let mouse = corral_tui::mouse_wanted(no_mouse, env.as_deref(), ctx.store.config.ui.mouse);
    corral_tui::run(ctx, mode, mouse)
}

fn capture_cwd(arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(c) = arg {
        return Ok(c);
    }
    if let Ok(text) = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
        && let Some(c) = find_cwd(&v)
    {
        return Ok(PathBuf::from(c));
    }
    std::env::current_dir().context("reading the current directory")
}

fn init(root: &Option<PathBuf>) -> Result<()> {
    let dir = start_dir(root)?;
    let store = Store::init(&dir)?;
    println!("initialized {}", store.dir().display());
    if store.config.projects.is_empty() {
        println!("no git projects found under {}", store.root().display());
    }
    for (k, p) in &store.config.projects {
        println!("project {k}: {}", p.path);
    }
    println!("tip: `corral skill install` teaches your agents the corral commands");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn new_task(
    ctx: &Ctx,
    title: &[String],
    project: Option<&str>,
    status: Status,
    body: Option<&str>,
    body_file: Option<&Path>,
    profile: Option<String>,
    depends: &[String],
    priority: Option<String>,
) -> Result<()> {
    let body = match (body_file, body) {
        (Some(f), _) => {
            std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?
        }
        (None, Some(b)) => b.to_string(),
        (None, None) => String::new(),
    };
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let task = corral_ops::new_task(ctx, &title.join(" "), project, &cwd, status, &body)?;
    let deps = depends
        .iter()
        .map(|d| Store::normalize_id(d))
        .collect::<Result<Vec<_>>>()?;
    if profile.is_some() || !deps.is_empty() || priority.is_some() {
        ctx.store.update(&task.id, |t| {
            if profile.is_some() {
                t.profile = profile.clone();
            }
            if !deps.is_empty() {
                t.depends_on = deps.clone();
            }
            if let Some(p) = &priority {
                t.priority = p.to_ascii_uppercase();
            }
            Ok(())
        })?;
    }
    println!("{}", task.id);
    Ok(())
}

fn list(
    ctx: &Ctx,
    status: Option<&str>,
    project: Option<&str>,
    all: bool,
    json: bool,
) -> Result<()> {
    let want = status.map(parse_status).transpose()?;
    let tasks: Vec<Task> = ctx
        .store
        .list()?
        .into_iter()
        .filter(|t| match want {
            Some(s) => t.status == s,
            None => all || !matches!(t.status, Status::Done | Status::Dropped),
        })
        .filter(|t| project.is_none_or(|p| t.project.as_deref() == Some(p)))
        .collect();
    if json {
        let v: Vec<_> = tasks.iter().map(|t| task_json(t, false)).collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    for t in &tasks {
        println!(
            "{:<7} {:<9} {:<3} {:<14} {}",
            t.id,
            t.status.label(),
            t.priority,
            t.project.as_deref().unwrap_or("-"),
            t.title
        );
    }
    Ok(())
}

fn show(ctx: &Ctx, id: &str, json: bool) -> Result<()> {
    let t = ctx.store.get(id)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&task_json(&t, true))?);
        return Ok(());
    }
    println!("{}  {}", t.id, t.title);
    println!("status:   {}", t.status.label());
    println!("project:  {}", t.project.as_deref().unwrap_or("-"));
    println!("priority: {}", t.priority);
    if let Some(p) = &t.profile {
        println!("profile:  {p}");
    }
    if !t.depends_on.is_empty() {
        println!("depends:  {}", t.depends_on.join(", "));
    }
    if t.queued {
        println!("queued:   yes");
    }
    for r in &t.runs {
        println!(
            "run {} [{}] {} branch {} worktree {}{}",
            r.id,
            run_profile(r),
            r.outcome
                .map(|o| format!("{o:?}"))
                .unwrap_or_else(|| "active".into()),
            r.branch,
            r.worktree.display(),
            r.summary
                .as_deref()
                .map(|s| format!("\n    summary: {s}"))
                .unwrap_or_default()
        );
        if let Some(g) = &r.gate {
            println!("    gate: {}", gate_line(g, r));
        }
        if let Some(rv) = &r.review {
            println!("    review: {}", review_line(rv));
        }
    }
    for a in &t.artifacts {
        println!(
            "artifact {} [{}] {} -> {}",
            a.run,
            a.kind.as_str(),
            a.title,
            a.path
        );
    }
    if !t.body.trim().is_empty() {
        println!("\n{}", t.body.trim_end());
    }
    Ok(())
}

fn edit(ctx: &Ctx, id: &str) -> Result<()> {
    let t = ctx.store.get(id)?;
    let editor = std::env::var("EDITOR")
        .ok()
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| "vi".into());
    let mut parts = editor.split_whitespace();
    let prog = parts.next().ok_or_else(|| anyhow!("empty $EDITOR"))?;
    let status = std::process::Command::new(prog)
        .args(parts)
        .arg(&t.path)
        .status()
        .with_context(|| format!("running {editor}"))?;
    if !status.success() {
        bail!("{editor} exited with {status}");
    }
    Ok(())
}

fn artifact(ctx: &Ctx, cmd: ArtifactCmd) -> Result<()> {
    match cmd {
        ArtifactCmd::Add {
            id,
            path,
            kind,
            title,
        } => {
            let kind = kind
                .as_deref()
                .map(|k| {
                    ArtifactKind::parse(k).ok_or_else(|| anyhow!("unknown artifact kind '{k}'"))
                })
                .transpose()?;
            let path = std::path::absolute(&path).unwrap_or(path);
            let a = corral_ops::add_artifact(ctx, &id, &path, kind, title.as_deref())?;
            println!("{} {} {}", a.run, a.kind.as_str(), a.path);
        }
        ArtifactCmd::List { task, json } => {
            let want = task.as_deref().map(Store::normalize_id).transpose()?;
            let rows: Vec<_> = corral_ops::all_artifacts(ctx)?
                .into_iter()
                .filter(|r| want.as_ref().is_none_or(|w| &r.task_id == w))
                .collect();
            if json {
                let v: Vec<_> = rows
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "task": r.task_id, "task_title": r.task_title, "project": r.project,
                            "artifact": r.artifact,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                for r in &rows {
                    println!(
                        "{:<7} {:<3} {:<10} {}  {}",
                        r.task_id,
                        r.artifact.run,
                        r.artifact.kind.as_str(),
                        r.artifact.title,
                        r.artifact.path
                    );
                }
            }
        }
    }
    Ok(())
}

fn review(ctx: &Ctx, id: &str, diff: bool) -> Result<()> {
    let info = corral_ops::review_info(ctx, id)?;
    println!("{}  {}", info.task.id, info.task.title);
    println!(
        "run {} branch {} base {}",
        info.run.id, info.run.branch, info.run.base
    );
    if let Some(s) = &info.run.summary {
        println!("summary: {s}");
    }
    println!(
        "diff: {} files, +{} -{}",
        info.stat.files.len(),
        info.stat.added,
        info.stat.removed
    );
    for f in &info.stat.files {
        println!(
            "  +{:<5} -{:<5} {}{}",
            f.added,
            f.removed,
            f.path,
            if f.untracked { " (new)" } else { "" }
        );
    }
    for a in &info.artifacts {
        println!("artifact {} [{}] {}", a.run, a.kind.as_str(), a.title);
    }
    println!(
        "main checkout ready to merge: {}",
        if info.main_ready { "yes" } else { "no" }
    );
    if diff {
        println!("\n{}", info.diff);
    }
    Ok(())
}

fn sync_once(ctx: &Ctx) -> Result<()> {
    for c in corral_ops::sync(ctx)? {
        println!(
            "{}: {} -> {} ({})",
            c.task,
            c.from.label(),
            c.to.label(),
            c.note
        );
    }
    Ok(())
}

fn hook(root: &Option<PathBuf>) {
    let result = open(root).and_then(|ctx| sync_once(&ctx));
    if let Err(e) = result {
        eprintln!("corral hook: {e:#}");
    }
}

pub fn run(cli: Cli) -> Result<()> {
    let root = cli.root;
    match cli.cmd {
        Cmd::Init => init(&root),
        Cmd::Hook => {
            hook(&root);
            Ok(())
        }
        Cmd::New {
            title,
            project,
            idea,
            ready,
            body,
            body_file,
            profile,
            depends,
            priority,
        } => {
            let status = if idea {
                Status::Idea
            } else if ready {
                Status::Ready
            } else {
                Status::Backlog
            };
            new_task(
                &open(&root)?,
                &title,
                project.as_deref(),
                status,
                body.as_deref(),
                body_file.as_deref(),
                profile,
                &depends,
                priority,
            )
        }
        Cmd::List {
            status,
            project,
            all,
            json,
        } => list(
            &open(&root)?,
            status.as_deref(),
            project.as_deref(),
            all,
            json,
        ),
        Cmd::Show { id, json } => show(&open(&root)?, &id, json),
        Cmd::Move { id, status, force } => {
            let t = corral_ops::move_task(&open(&root)?, &id, parse_status(&status)?, force)?;
            println!("{} -> {}", t.id, t.status.label());
            Ok(())
        }
        Cmd::Edit { id } => edit(&open(&root)?, &id),
        Cmd::Dispatch {
            id,
            profile,
            headless,
            base,
            queue,
        } => {
            let choice = profile.choice();
            let opts = DispatchOpts {
                profile: choice.profile,
                model: choice.model,
                effort: choice.effort,
                mode: if headless {
                    RunMode::Headless
                } else {
                    RunMode::Interactive
                },
                base,
                queue_if_blocked: queue,
            };
            print_outcome(corral_ops::dispatch(&open(&root)?, &id, &opts)?);
            Ok(())
        }
        Cmd::Reply { id, text } => {
            let t = corral_ops::reply(&open(&root)?, &id, &text.join(" "))?;
            println!("{} replied ({})", t.id, t.status.label());
            Ok(())
        }
        Cmd::Done { id, summary } => {
            let t = corral_ops::done(&open(&root)?, &id, &summary)?;
            println!("{} -> {}", t.id, t.status.label());
            Ok(())
        }
        Cmd::Gate { id, wait } => gate(&open(&root)?, &id, wait),
        Cmd::ReviewRun { id, reviewer } => review_run(&open(&root)?, &id, reviewer.as_deref()),
        Cmd::Verdict {
            id,
            verdict: v,
            notes,
            notes_file,
        } => verdict(&open(&root)?, &id, &v, notes, notes_file),
        Cmd::Artifact { cmd } => artifact(&open(&root)?, cmd),
        Cmd::Pin {
            target,
            label,
            follow_latest,
        } => pins::pin(&open(&root)?, &target, label.as_deref(), follow_latest),
        Cmd::Unpin { target } => pins::unpin(&open(&root)?, &target),
        Cmd::Pins { json } => pins::list(&open(&root)?, json),
        Cmd::Open { target } => pins::open(&open(&root)?, &target),
        Cmd::Review { id, diff } => review(&open(&root)?, &id, diff),
        Cmd::Approve {
            id,
            merge,
            keep_worktree,
            keep_branch,
        } => {
            let strategy = match merge.as_str() {
                "ff" => ApproveStrategy::FastForward,
                "keep" => ApproveStrategy::KeepBranch,
                _ => ApproveStrategy::Rebase,
            };
            println!(
                "{}",
                corral_ops::approve(&open(&root)?, &id, strategy, !keep_worktree, keep_branch)?
            );
            Ok(())
        }
        Cmd::Changes { id, text } => {
            let t = corral_ops::request_changes(&open(&root)?, &id, &text.join(" "))?;
            println!("{} -> {}", t.id, t.status.label());
            Ok(())
        }
        Cmd::Retry { id, profile } => {
            print_outcome(corral_ops::retry(&open(&root)?, &id, &profile.choice())?);
            Ok(())
        }
        Cmd::Discard { id } => {
            let t = corral_ops::discard(&open(&root)?, &id)?;
            println!("{} -> {}", t.id, t.status.label());
            Ok(())
        }
        Cmd::Jump { id } => corral_ops::jump(&open(&root)?, &id),
        Cmd::Stats { by, since, json } => {
            crate::stats::run(&open(&root)?, &by, since.as_deref(), json)
        }
        Cmd::Sync { watch } => {
            let ctx = open(&root)?;
            if !watch {
                return sync_once(&ctx);
            }
            loop {
                if let Err(e) = sync_once(&ctx) {
                    eprintln!("sync: {e:#}");
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        }
        Cmd::Pm => {
            let pane = corral_ops::open_pm(&open(&root)?)?;
            println!("PM pane {pane}");
            Ok(())
        }
        Cmd::Board { no_mouse } => tui(open(&root)?, Mode::Board, no_mouse),
        Cmd::Capture { cwd, no_mouse } => {
            tui(open(&root)?, Mode::Capture(capture_cwd(cwd)?), no_mouse)
        }
        Cmd::Artifacts { no_mouse } => tui(open(&root)?, Mode::Artifacts, no_mouse),
        Cmd::Settings { no_mouse } => tui(open(&root)?, Mode::Settings, no_mouse),
        Cmd::Harness {
            cmd: HarnessCmd::List { json, refresh },
        } => harness::list(&open(&root)?, json, refresh),
        Cmd::Plugin {
            cmd: PluginCmd::Install { print },
        } => plugin::install(&open(&root)?, print),
        Cmd::Skill { cmd: None } => {
            skill::print();
            Ok(())
        }
        Cmd::Skill {
            cmd:
                Some(SkillCmd::Install {
                    harness,
                    dir,
                    force,
                }),
        } => skill::install(&harness, dir, force),
        Cmd::Config { cmd } => config_cmd::run(&start_dir(&root)?, cmd),
    }
}
