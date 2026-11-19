mod args;
mod editor;
mod hook;
mod import;
mod output;

use std::{
    env,
    io::{self, Write},
    path::Path,
    process::ExitCode,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use clap::{Parser, error::ErrorKind};
use corral_engine::{
    Config, Engine, NewTask, Op, Status, Usage, events::EventFilter, refuse, store::TaskFilter,
};
use jiff::Timestamp;

use args::{ArtifactCommand, Cli, Command, ConfigCommand, Id, Text, UsageCommand, Verdict};
use output::{Output, TaskView};

fn main() -> ExitCode {
    let program = env::args_os()
        .next()
        .filter(|arg| !arg.is_empty())
        .map_or_else(
            || "corral-next".into(),
            |arg| arg.to_string_lossy().into_owned(),
        );
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let help = matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            );
            return match error.print() {
                Ok(()) if help => ExitCode::SUCCESS,
                _ => ExitCode::FAILURE,
            };
        }
    };
    let hook = matches!(cli.command, Command::Hook);
    let mut output = Output {
        program: &program,
        root: cli.root.as_deref(),
        json: cli.json,
        stdout: io::stdout().lock(),
    };
    let code = match run(cli.command, &mut output) {
        Ok(code) => code,
        Err(error) => output.report(&error),
    };
    ExitCode::from(if hook { 0 } else { code })
}

fn run(command: Command, out: &mut Output<'_>) -> Result<u8> {
    let cwd = env::current_dir().context("read current directory")?;
    if let Command::Init { first_id } = command {
        let root = if out.root.is_some() || env::var_os("CORRAL_NEXT_ROOT").is_some() {
            Engine::discover_root(&cwd, out.root)?
        } else {
            cwd
        };
        let engine = Engine::init(root, first_id)?;
        out.value(&engine.config, |out, _| {
            writeln!(out, "Initialized {}", engine.state_dir.display())
        })?;
        return Ok(0);
    }
    let root = Engine::discover_root(&cwd, out.root)?;
    if let Command::Config(ConfigCommand::Check) = command {
        check_config(&root, out)?;
        return Ok(0);
    }
    if let Command::Import {
        dir,
        all,
        events,
        dry_run,
    } = command
    {
        let report = import::run(&root, &dir, events.as_deref(), all, dry_run)?;
        out.value(&report, |out, report| report.write(out))?;
        return Ok(0);
    }
    if matches!(command, Command::Hook) && hook::nudge(&root)? {
        return Ok(0);
    }
    let engine = Engine::open(root)?;
    match command {
        Command::New {
            title,
            project,
            idea,
            ready,
            priority,
            kind,
            size,
            base,
            parent,
            depends_on,
        } => {
            let task = NewTask {
                title,
                project,
                priority,
                kind,
                size,
                base,
                parent,
                status: if idea {
                    Status::Idea
                } else if ready {
                    Status::Ready
                } else {
                    Status::Backlog
                },
                depends_on: depends_on
                    .as_deref()
                    .map(editor::dependencies)
                    .unwrap_or_default(),
                ..NewTask::default()
            };
            apply(&engine, out, Op::New { task }, false)?;
        }
        Command::List {
            status,
            project,
            all,
        } => {
            let statuses = match status {
                Some(status) => vec![status],
                None if all => vec![],
                None => vec![
                    Status::Idea,
                    Status::Backlog,
                    Status::Ready,
                    Status::Running,
                    Status::Blocked,
                    Status::Review,
                ],
            };
            let tasks = engine.store.list_tasks(&TaskFilter {
                statuses,
                project,
                parent: None,
            })?;
            out.value(tasks.as_slice(), output::tasks)?;
        }
        Command::Show(Id { id }) => out.show(&TaskView {
            task: engine.store.require_task(&id)?,
            runs: engine.store.list_runs(&id)?,
            checks: engine.store.list_latest_checks(&id)?,
            artifacts: engine.list_artifacts(Some(&id))?,
        })?,
        Command::Log(Id { id }) => {
            engine.store.require_task(&id)?;
            let events = engine.store.list_events(&id)?;
            out.value(events.as_slice(), output::log)?;
        }
        Command::Edit(flags) => {
            let (id, edit) = editor::prepare(&engine, flags)?;
            apply(&engine, out, Op::Edit { id, edit }, false)?;
        }
        Command::Comment(Text { id, text }) => {
            apply(&engine, out, Op::Comment { id, text }, false)?
        }
        Command::Move { id, status, force } => apply(&engine, out, Op::Move { id, status }, force)?,
        Command::Dispatch {
            id,
            profile,
            base,
            force,
        } => apply(&engine, out, Op::Dispatch { id, profile, base }, force)?,
        Command::Reply(Text { id, text }) => apply(&engine, out, Op::Reply { id, text }, false)?,
        Command::Stop(Id { id }) => apply(&engine, out, Op::Stop { id }, false)?,
        Command::Retry {
            id,
            profile,
            fresh,
            note,
        } => apply(
            &engine,
            out,
            Op::Retry {
                id,
                profile,
                fresh,
                note,
            },
            false,
        )?,
        Command::Done { id, summary } => apply(&engine, out, Op::Done { id, summary }, false)?,
        Command::Gate(Id { id }) => apply(&engine, out, Op::Gate { id }, false)?,
        Command::Review { id, profile } => apply(&engine, out, Op::Review { id, profile }, false)?,
        Command::Verdict {
            id,
            verdict,
            notes,
            notes_file,
        } => {
            let notes = match notes_file {
                Some(path) => Some(editor::read_text(&path)?),
                None => notes,
            };
            apply(
                &engine,
                out,
                Op::Verdict {
                    id,
                    approve: matches!(verdict, Verdict::Approve),
                    notes,
                },
                false,
            )?;
        }
        Command::Changes(Text { id, text }) => {
            apply(&engine, out, Op::Changes { id, text }, false)?
        }
        Command::Approve { id, onto, force } => {
            apply(&engine, out, Op::Approve { id, onto }, force)?
        }
        Command::Drop(Id { id }) => apply(&engine, out, Op::Drop { id }, false)?,
        Command::Reopen(Id { id }) => apply(&engine, out, Op::Reopen { id }, false)?,
        Command::Sync { watch: false } | Command::Hook => apply(&engine, out, Op::Sync, false)?,
        Command::Sync { watch: true } => watch_sync(&engine, out)?,
        Command::Wait {
            kinds,
            task,
            project,
            after,
            timeout,
        } => {
            let filter = EventFilter {
                kinds,
                task,
                project,
            };
            let after = match after {
                Some(after) => after,
                None => engine.store.latest_seq()?,
            };
            let Some(event) = engine.store.wait(&filter, after, timeout)? else {
                return Ok(124);
            };
            out.value(&event, output::event)?;
        }
        Command::Artifact(command) => artifact(&engine, out, command)?,
        Command::Usage { by, command: None } => {
            out.value(&engine.usage_summary(by)?, output::usage)?
        }
        Command::Usage {
            command:
                Some(UsageCommand::Add {
                    model,
                    task,
                    run,
                    input,
                    output,
                    cache_read,
                    cache_write,
                    usd,
                    billing,
                }),
            ..
        } => {
            apply(
                &engine,
                out,
                Op::UsageAdd {
                    usage: Usage {
                        task,
                        run,
                        at: Timestamp::now(),
                        profile: None,
                        harness: None,
                        model,
                        input,
                        output,
                        cache_read,
                        cache_write,
                        usd,
                        billing,
                    },
                },
                false,
            )?;
        }
        Command::Pipeline { task } => engine.advance(&task)?,
        Command::Init { .. } | Command::Config(_) | Command::Import { .. } => {
            unreachable!("handled before opening the engine")
        }
    }
    Ok(0)
}

fn apply(engine: &Engine, out: &mut Output<'_>, op: Op, force: bool) -> Result<()> {
    out.applied(&engine.apply(op, force)?)
}

fn check_config(root: &Path, out: &mut Output<'_>) -> Result<()> {
    let path = root.join(".corral-next/corral.toml");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read configuration {}", path.display()))?;
    let config = Config::check(root, &text).map_err(|problems| {
        let mut message = String::from("configuration problems:");
        for problem in problems {
            message.push_str(&format!("\n  {problem}"));
        }
        anyhow::anyhow!(message)
    })?;
    out.value(&config, |out, _| writeln!(out, "Configuration is valid"))
}

fn watch_sync(engine: &Engine, out: &mut Output<'_>) -> Result<()> {
    let interval = engine.config.sync.watch_secs;
    ensure!(interval > 0, "sync.watch_secs must be greater than zero");
    loop {
        match engine.apply(Op::Sync, false) {
            Ok(applied) => out.applied(&applied)?,
            Err(error) => {
                out.report(&error);
            }
        }
        thread::sleep(Duration::from_secs(interval));
    }
}

fn artifact(engine: &Engine, out: &mut Output<'_>, command: ArtifactCommand) -> Result<()> {
    match command {
        ArtifactCommand::Add {
            id,
            path,
            kind,
            title,
        } => apply(
            engine,
            out,
            Op::ArtifactAdd {
                id,
                path,
                kind,
                title,
            },
            false,
        ),
        ArtifactCommand::List { id } => {
            let artifacts = engine.list_artifacts(id.as_deref())?;
            out.value(&artifacts, |out, artifacts| {
                artifacts
                    .iter()
                    .try_for_each(|artifact| output::artifact_line(out, artifact))
            })
        }
        ArtifactCommand::Show { reference } if out.json => {
            let artifact = engine
                .store
                .get_artifact(&reference)?
                .ok_or_else(|| refuse(format!("artifact {reference} does not exist"), vec![]))?;
            out.value(&artifact, output::artifact_line)
        }
        ArtifactCommand::Show { reference } => out
            .stdout
            .write_all(&engine.show_artifact(&reference)?)
            .context("write artifact contents"),
    }
}
