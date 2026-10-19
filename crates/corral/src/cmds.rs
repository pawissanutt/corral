mod dispatch_status;
mod reply_cmd;
pub(crate) mod route;
mod session_cmd;
pub(crate) mod task_output;

use task_output::{list, show, task_json};

use std::path::PathBuf;
use std::time::Duration;

use anyhow::anyhow;
use anyhow::bail;
use anyhow::{Context, Result};
use corral_core::{AutopilotState, GateRecord, GateResult, ReviewRecord, Run, Status, Verdict};
use corral_ops::{ApproveStrategy, Ctx};
use corral_tui::Mode;

use crate::{Cli, Cmd, HarnessCmd, PluginCmd, SkillCmd, config_cmd, harness, pins, plugin, skill};

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

fn gate(ctx: &Ctx, id: &str, wait: bool, force: bool) -> Result<()> {
    if let Some(reason) = corral_ops::resources::admission(ctx)? {
        eprintln!("warning: {reason}; use --force to override machine pressure");
    }
    let started = corral_ops::gate_start_with_force(ctx, id, force)?;
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

/// "reviewing", "changes (round 1 of 2): the gate failed at 1a2b3c4: `cargo test`", "escalated: <reason>".
fn autopilot_line(a: &AutopilotState, rounds: u32, max_rounds: u32) -> String {
    let mut line = a.stage.as_str().to_string();
    if rounds > 0 {
        line.push_str(&format!(" (round {rounds} of {max_rounds})"));
    }
    if let Some(reason) = a
        .reason
        .as_deref()
        .and_then(|r| r.lines().find(|l| !l.trim().is_empty()))
    {
        line.push_str(&format!(": {reason}"));
    }
    line
}

fn autopilot(mut ctx: Ctx, action: &str) -> Result<()> {
    match action {
        "on" => ctx.store.set_autopilot(true)?,
        "off" => ctx.store.set_autopilot(false)?,
        _ => {}
    }
    let config = &ctx.store.config;
    dispatch_status::print(&ctx)?;
    let resource_reason = match corral_ops::resources::status(&ctx) {
        Ok(state) => {
            let snapshot = &state.snapshot;
            println!(
                "resources: load {:.2}/core ({:.2} / {} cores), mem {} bytes available",
                snapshot.load_per_core, snapshot.load_one, snapshot.cores, snapshot.mem_available
            );
            for disk in &snapshot.disks {
                println!(
                    "  disk {}: {} bytes free",
                    disk.path.display(),
                    disk.free_bytes
                );
            }
            if let Some(psi) = snapshot.cpu_psi {
                println!("  cpu pressure: {psi:.2}%");
            }
            if let Some(psi) = snapshot.mem_psi {
                println!("  memory pressure: {psi:.2}%");
            }
            if let Some(reason) = &state.reason {
                println!("  waiting: machine busy ({reason})");
            }
            state.reason
        }
        Err(error) => {
            println!("resources unavailable: {error:#}");
            None
        }
    };
    for t in ctx.store.list()? {
        if matches!(t.status, Status::Done | Status::Dropped) {
            continue;
        }
        if let Some(reason) = corral_ops::automation_blocker(&ctx, &t)? {
            println!("{:<7} {:<9} {reason}", t.id, t.status.label());
            continue;
        }
        if let Some(hold) = resource_reason
            .as_deref()
            .and_then(|reason| corral_tui::resources::hold(&t, config, reason))
        {
            println!("{:<7} {:<9} {hold}", t.id, t.status.label());
            continue;
        }
        let Some(a) = t.current_run().and_then(|r| r.autopilot.as_ref()) else {
            continue;
        };
        let max = config
            .review_policy(t.project.as_deref(), t.review)
            .max_rounds;
        println!(
            "{:<7} {:<9} {}",
            t.id,
            t.status.label(),
            autopilot_line(a, t.rounds, max)
        );
    }
    Ok(())
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
    attempt: Option<&str>,
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
    let record = corral_ops::verdict_bound(ctx, id, verdict, &notes, attempt)?;
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

fn parse_status(s: &str) -> Result<Status> {
    Status::parse(s).ok_or_else(|| {
        anyhow!(
            "unknown status '{s}' (idea, backlog, ready, running, needs_me, review, done, dropped)"
        )
    })
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
    ctx.herdr.require_running()?;
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

fn init(root: &Option<PathBuf>, entry: &crate::entry::Entry) -> Result<()> {
    let dir = start_dir(root)?;
    let store = entry.init(&dir)?;
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

/// `list --done`: Done tasks, newest finished first, optionally only those finished after `since` (N h/d/w).
fn list_done(
    ctx: &Ctx,
    project: Option<&str>,
    since: Option<&str>,
    json: bool,
    query: Option<&str>,
) -> Result<()> {
    let cutoff = since.map(crate::stats::since_cutoff).transpose()?;
    let matches = query
        .map(|input| {
            corral_ops::search_tasks(&ctx.store, input).map(|tasks| {
                tasks
                    .into_iter()
                    .map(|t| t.id)
                    .collect::<std::collections::HashSet<_>>()
            })
        })
        .transpose()?;
    let done: Vec<_> = corral_ops::finished_tasks(&ctx.store)?
        .into_iter()
        .filter(|(t, _)| t.status == Status::Done)
        .filter(|(t, _)| matches.as_ref().is_none_or(|ids| ids.contains(&t.id)))
        .filter(|(t, _)| project.is_none_or(|p| t.project.as_deref() == Some(p)))
        .filter(|(_, f)| cutoff.is_none_or(|c| f.at >= c))
        .collect();
    if json {
        let v: Vec<_> = done
            .iter()
            .map(|(t, f)| {
                let mut v = task_json(t, false);
                if let Some(o) = v.as_object_mut() {
                    o.insert("finished".into(), f.at.to_string().into());
                    o.insert(
                        "landed".into(),
                        serde_json::to_value(&f.landed).unwrap_or_default(),
                    );
                }
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let tz = jiff::tz::TimeZone::system();
    for (t, f) in &done {
        let landed = f.landed.as_ref().map_or_else(
            || "-".to_string(),
            |l| format!("{} {}", l.short_sha(), l.strategy_label()),
        );
        println!(
            "{:<7} {} {:<14} {:<14} {}",
            t.id,
            f.at.to_zoned(tz.clone()).strftime("%Y-%m-%d"),
            landed,
            t.project.as_deref().unwrap_or("-"),
            t.title
        );
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
    crate::artifact_show::print_grouped(&info.artifacts, false);
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

pub fn run(cli: Cli, matches: &clap::ArgMatches) -> Result<()> {
    let root = cli.root;
    let cmd = cli.cmd.unwrap_or(Cmd::Up {
        session: None,
        yes: false,
    });
    let entry = crate::entry::Entry::new(&cmd, matches)?;
    let open = |root: &Option<PathBuf>| entry.open(&start_dir(root)?);
    match cmd {
        Cmd::Automation(args) => crate::automation_cmd::run(&open(&root)?, args),
        Cmd::Setup(args) => crate::setup_cmd::run(&start_dir(&root)?, args),
        Cmd::Up { session, yes } => crate::up::run(&start_dir(&root)?, session, yes, &entry),
        Cmd::Doctor { fix, json } => crate::doctor::run(&start_dir(&root)?, fix, json, &entry),
        Cmd::Pricing { cmd } => crate::pricing_cmd::run(&open(&root)?.store, cmd),
        Cmd::Wait(args) => crate::wait_cmd::run(&open(&root)?.store, args),
        Cmd::BuildIdentity => crate::hooks_cmd::build_identity(),
        Cmd::RuntimeProbe { pid } => {
            println!("{}", corral_ops::runtime_probe(pid)?);
            Ok(())
        }
        Cmd::Hooks { cmd } => crate::hooks_cmd::run(&open(&root)?, cmd),
        Cmd::Run { cmd } => crate::run_cmd::run(&open(&root)?, cmd),
        Cmd::Plan { cmd } => crate::plan_cmd::run(&open(&root)?, cmd),
        Cmd::Lessons(args) => crate::lessons_cmd::run(&open(&root)?, args),
        Cmd::Init => init(&root, &entry),
        Cmd::Import(args) => crate::import_cmd::run(&root, args, &entry),
        Cmd::ReviewSprint(args) => crate::sprint_cmd::run(&open(&root)?, args),
        Cmd::Digest(args) => crate::digest_cmd::run(&open(&root)?, args),
        Cmd::Revert { id } => {
            let result = corral_ops::revert(&open(&root)?, &id)?;
            println!(
                "{} reopened in Backlog; reverted {} commit(s) at {}; dispatch held",
                result.task.id,
                result.original_shas.len(),
                result.revert_shas.join(", ")
            );
            Ok(())
        }
        Cmd::Idea(mut args) => {
            let ctx = open(&root)?;
            args.author = entry.decision_author(&ctx.store)?.into();
            crate::idea_cmd::run(&ctx, args)
        }
        Cmd::Hook => {
            session_cmd::hook(&root, &entry);
            Ok(())
        }
        Cmd::BindSession {
            socket,
            launch_record,
        } => session_cmd::bind(&root, &socket, &launch_record, &entry),
        Cmd::New {
            origin,
            title,
            project,
            idea,
            ready,
            body,
            body_file,
            profile,
            depends,
            priority,
            kind,
            size,
            design,
            labels,
        } => {
            let status = if idea {
                Status::Idea
            } else if ready {
                Status::Ready
            } else {
                Status::Backlog
            };
            crate::idea_cmd::new_task(
                &open(&root)?,
                &title,
                project.as_deref(),
                status,
                body.as_deref(),
                body_file.as_deref(),
                profile,
                &depends,
                priority,
                kind,
                size,
                origin,
                design,
                &labels,
            )
        }
        Cmd::List {
            done: true,
            project,
            since,
            json,
            query,
            ..
        } => list_done(
            &open(&root)?,
            project.as_deref(),
            since.as_deref(),
            json,
            query.as_deref(),
        ),
        Cmd::List {
            status,
            project,
            all,
            json,
            columns,
            query,
            ..
        } => list(
            &open(&root)?,
            status.as_deref(),
            project.as_deref(),
            all,
            json,
            &columns,
            query.as_deref(),
        ),
        Cmd::Search(args) => crate::search_cmd::run(&open(&root)?, args),
        Cmd::Log(args) => crate::log_cmd::run(&open(&root)?, args),
        Cmd::Show { id, json } => show(&open(&root)?, &id, json),
        Cmd::Comment {
            id,
            text,
            author,
            send,
        } => {
            let task = corral_ops::comment(&open(&root)?, &id, &text, author, send)?;
            println!("{} commented", task.id);
            Ok(())
        }
        Cmd::Move {
            id,
            status,
            force,
            owner_ok,
            reason,
            author: _,
        } => {
            let ctx = open(&root)?;
            let to = parse_status(&status)?;
            let t = corral_ops::move_task_with_decision(
                &ctx,
                &id,
                to,
                force,
                reason.as_deref(),
                entry.decision_author(&ctx.store)?,
                owner_ok,
            )?;
            println!("{} -> {}", t.id, t.status.label());
            Ok(())
        }
        Cmd::Edit { id, owner_ok } => crate::idea_cmd::edit(&open(&root)?, &id, owner_ok),
        Cmd::Route {
            id,
            set_reviewer,
            clear_reviewer,
            profile,
            json,
        } => {
            let ctx = open(&root)?;
            if set_reviewer.is_some() || clear_reviewer {
                corral_ops::reviewer_set(&ctx, &id, set_reviewer.as_deref())?;
            }
            route::explain(&ctx, &id, &profile.choice(), json)
        }
        Cmd::Queue { cmd } => crate::queue::run(&open(&root)?, cmd),
        Cmd::Dispatch {
            id,
            plan,
            profile,
            headless,
            base,
            queue,
            force,
            explain,
            json,
        } => {
            let mut opts = profile.dispatch_opts(headless, base, queue);
            opts.force_resources = force;
            opts.plan = plan;
            route::dispatch(&open(&root)?, &id, opts, explain, json)
        }
        Cmd::Reply {
            id,
            text,
            resend,
            mark_delivered,
            reason,
            author,
        } => reply_cmd::run(
            &open(&root)?,
            &id,
            &text,
            resend,
            mark_delivered,
            reason.as_deref(),
            author,
        ),
        Cmd::Unstick { id } => {
            let ctx = open(&root)?;
            crate::unstick::run(&ctx, &id)
        }
        Cmd::Done {
            id,
            summary,
            check: true,
            json,
        } => crate::completion_cmd::check(&open(&root)?, &id, summary.as_deref(), json),
        Cmd::Done {
            id,
            summary,
            check: false,
            ..
        } => {
            let t = corral_ops::done(&open(&root)?, &id, &summary.context("pass --summary")?)?;
            println!("{} -> {}", t.id, t.status.label());
            Ok(())
        }
        Cmd::Gate { id, wait, force } => gate(&open(&root)?, &id, wait, force),
        Cmd::Criteria { cmd } => crate::completion_cmd::criteria(&open(&root)?, cmd),
        Cmd::Evidence { cmd } => crate::completion_cmd::evidence(&open(&root)?, cmd),
        Cmd::ReviewRun { id, reviewer } => review_run(&open(&root)?, &id, reviewer.as_deref()),
        Cmd::Verdict {
            id,
            verdict: v,
            notes,
            notes_file,
            attempt,
        } => verdict(
            &open(&root)?,
            &id,
            &v,
            notes,
            notes_file,
            attempt.as_deref(),
        ),
        Cmd::Artifact { cmd } => crate::artifact_cmd::run(&open(&root)?, cmd),
        Cmd::Pin {
            target,
            label,
            follow_latest,
        } => pins::pin(&open(&root)?, &target, label.as_deref(), follow_latest),
        Cmd::Unpin { target } => pins::unpin(&open(&root)?, &target),
        Cmd::Pins { json } => pins::list(&open(&root)?, json),
        Cmd::Dashboard { cmd } => crate::dashboard_cmd::run(&open(&root)?, cmd),
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
        Cmd::Changes { id, text, escalate } => {
            reply_cmd::changes(&open(&root)?, &id, &text, escalate)
        }
        Cmd::Retry { id, run, profile } => {
            route::retry(&open(&root)?, &id, run.as_deref(), &profile.choice())
        }
        Cmd::Discard {
            id,
            keep_branch,
            owner_ok,
            reason,
            author: _,
        } => {
            let ctx = open(&root)?;
            if owner_ok {
                ctx.store.owner_decision(
                    &id,
                    "drop",
                    reason.as_deref().unwrap_or("Owner approved discarding run"),
                    "flag",
                )?;
            }
            let t = corral_ops::discard_with_decision(
                &ctx,
                &id,
                keep_branch,
                reason.as_deref(),
                entry.decision_author(&ctx.store)?,
            )?;
            println!("{} -> {}", t.id, t.status.label());
            Ok(())
        }
        Cmd::Jump { id } => corral_ops::jump(&open(&root)?, &id),
        Cmd::Stats {
            storage,
            by,
            since,
            json,
        } => {
            let ctx = open(&root)?;
            if storage {
                crate::artifact_cmd::storage(&ctx, json)
            } else {
                crate::stats::run(&ctx, &by, since.as_deref(), json)
            }
        }
        Cmd::Sync { watch } => {
            let ctx = Ctx::open_for_sync(&start_dir(&root)?)?;
            entry.apply(&ctx.store)?;
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
        Cmd::Autopilot { action } => autopilot(open(&root)?, &action),
        Cmd::Pm { cmd } => crate::pm_cmd::run(&start_dir(&root)?, cmd, &entry),
        Cmd::FocusBoard => crate::up::focus_board(&open(&root)?),
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
        Cmd::Config { cmd } => config_cmd::run(&start_dir(&root)?, cmd, &entry),
    }
}
