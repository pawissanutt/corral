mod args;
mod editor;
mod ext;
mod hook;
mod import;
mod lens;
mod output;
#[cfg(unix)]
mod pi_startup_cmd;

use std::{
    collections::BTreeMap,
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
    Actor, Applied, ArtifactKind, Config, Engine, NewTask, Op, Role, Status, Task, Usage,
    UsageToken, config::ExtensionTool, events::EventFilter, extension, refuse, store::TaskFilter,
};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::{Map, json};

use args::{
    ArtifactCommand, Cli, Command, ConfigCommand, ExtCommand, Id, Text, UsageCommand, Verdict,
};
use output::{Output, TaskView};

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().collect();
    if args
        .get(1)
        .is_some_and(|arg| arg == corral_engine::extension::ATTEMPT_ARG)
    {
        return ExitCode::from(corral_engine::extension::attempt_main(&args[2..]));
    }
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
    #[cfg(unix)]
    if let Command::PiStartup(command) = command {
        pi_startup_cmd::run(command, out)?;
        return Ok(0);
    }
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
        let config = checked_config(&root)?;
        out.value(&config, |out, _| writeln!(out, "Configuration is valid"))?;
        return Ok(0);
    }
    if let Command::Ext(ExtCommand::Check { name, project }) = command {
        return ext::check(&root, &checked_config(&root)?, &name, project, out);
    }
    if let Command::Tool {
        project,
        invocation,
    } = command
    {
        tool(&root, &cwd, project.as_deref(), &invocation, out)?;
        return Ok(0);
    }
    if let Command::Lens(command) = command {
        return lens::run(&root, &cwd, command, out);
    }
    if let Command::Ext(ExtCommand::Deliver) = command {
        return ext::deliver(&root, &checked_config(&root)?, out);
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
            difficulty,
            tags,
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
                difficulty,
                tags,
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
            let explicit_zero_tokens = [
                (UsageToken::Input, input),
                (UsageToken::Output, output),
                (UsageToken::CacheRead, cache_read),
                (UsageToken::CacheWrite, cache_write),
            ]
            .into_iter()
            .filter_map(|(token, count)| (count == Some(0)).then_some(token))
            .collect();
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
                        input: input.unwrap_or_default(),
                        output: output.unwrap_or_default(),
                        cache_read: cache_read.unwrap_or_default(),
                        cache_write: cache_write.unwrap_or_default(),
                        explicit_zero_tokens,
                        usd,
                        billing,
                    },
                },
                false,
            )?;
        }
        Command::Pipeline { task } => engine.advance(&task)?,
        #[cfg(unix)]
        Command::PiStartup(_) => unreachable!("handled before opening the engine"),
        Command::Init { .. }
        | Command::Config(_)
        | Command::Ext(_)
        | Command::Tool { .. }
        | Command::Lens(_)
        | Command::Import { .. } => unreachable!("handled before opening the engine"),
    }
    Ok(0)
}

fn apply(engine: &Engine, out: &mut Output<'_>, op: Op, force: bool) -> Result<()> {
    out.applied(&engine.apply(op, force)?)
}

fn checked_config(root: &Path) -> Result<Config> {
    let path = root.join(".corral-next/corral.toml");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read configuration {}", path.display()))?;
    Config::check(root, &text).map_err(|problems| {
        let mut message = String::from("configuration problems:");
        for problem in problems {
            message.push_str(&format!("\n  {problem}"));
        }
        anyhow::anyhow!(message)
    })
}

#[derive(Serialize)]
struct ListedTool<'a> {
    name: &'a str,
    extension: &'a str,
    about: Option<&'a str>,
    #[serde(skip)]
    definition: &'a ExtensionTool,
}

fn tool(
    root: &Path,
    cwd: &Path,
    project_flag: Option<&str>,
    invocation: &[String],
    out: &mut Output<'_>,
) -> Result<()> {
    let same_root = launch_workspace_matches(root);
    let task_id = same_root.then(|| tool_env("CORRAL_TASK")).flatten();
    let engine = task_id.as_ref().map(|_| Engine::open(root)).transpose()?;
    let owner_config;
    let (config, task) = if let Some((engine, id)) = engine.as_ref().zip(task_id.as_deref()) {
        (&engine.config, Some(engine.store.require_task(id)?))
    } else {
        owner_config = Config::load(&root.join(".corral-next/corral.toml"))?;
        (&owner_config, None)
    };
    let project = tool_project(config, root, cwd, project_flag, task.as_ref(), same_root)?;
    let mut tools = Vec::new();
    for (name, extension) in &config.extensions {
        if extension.enabled
            && project.map_or_else(
                || extension.projects.is_none(),
                |project| extension.includes_project(project),
            )
        {
            tools.extend(extension.tools.iter().map(|(tool, definition)| ListedTool {
                name: tool,
                extension: name,
                about: definition.about.as_deref(),
                definition,
            }));
        }
    }
    tools.sort_by_key(|tool| (tool.name, tool.extension));
    let Some((name, args)) = invocation.split_first() else {
        return out.value(tools.as_slice(), |out, tools| {
            if tools.is_empty() {
                return writeln!(out, "No tools configured for this project");
            }
            for group in tools.chunk_by(|a, b| a.name == b.name) {
                for tool in group {
                    write!(out, "{}", tool.name)?;
                    if group.len() > 1 {
                        write!(out, " (ambiguous: extension {:?})", tool.extension)?;
                    }
                    if let Some(about) = tool.about {
                        write!(out, " — {about}")?;
                    }
                    writeln!(out)?;
                }
            }
            Ok(())
        });
    };
    let mut matching = tools.iter().filter(|tool| tool.name == name);
    let entry = matching
        .next()
        .with_context(|| format!("unknown tool {name:?}; use corral-next tool to list tools"))?;
    let mut providers = matching.map(|tool| tool.extension).peekable();
    if providers.peek().is_some() {
        let names = std::iter::once(entry.extension)
            .chain(providers)
            .map(|name| format!("{name:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!("ambiguous tool {name:?}; provided by extensions {names}");
    }

    let run = same_root
        .then(|| tool_env("CORRAL_RUN"))
        .flatten()
        .map(|id| {
            let (engine, task) = engine
                .as_ref()
                .zip(task.as_ref())
                .context("CORRAL_RUN requires CORRAL_TASK")?;
            engine
                .store
                .get_run(&task.id, &id)?
                .with_context(|| format!("unknown run {id:?} for task {}", task.id))
        })
        .transpose()?;
    // Reviewer head_sha is the previous verdict's diff baseline, not the current submission.
    let head = match (run.as_ref(), engine.as_ref()) {
        (Some(run), Some(engine)) if run.role == Role::Reviewer => engine
            .store
            .latest_run(&run.task, Role::Worker)?
            .and_then(|worker| worker.head_sha),
        (Some(run), _) => run.head_sha.clone(),
        _ => None,
    };
    let empty_env = BTreeMap::new();
    let settings = project.and_then(|p| config.projects.get(p));
    let project_env = settings.map_or(&empty_env, |settings| &settings.env);
    let repo = run.as_ref().map_or_else(
        || settings.map_or_else(|| root.to_path_buf(), |settings| root.join(&settings.path)),
        |run| run.worktree.clone(),
    );
    let repo = repo.canonicalize().unwrap_or(repo);
    let actor = tool_env("CORRAL_ACTOR");
    let role = match actor.as_deref() {
        Some("agent") => "worker",
        Some(role) => role,
        None => "owner",
    };
    let request = Map::from_iter([
        ("args".into(), json!(args)),
        ("role".into(), json!(role)),
        (
            "repo".into(),
            json!(repo.to_str().context("tool repository path is not UTF-8")?),
        ),
        ("base".into(), json!(run.as_ref().map(|run| &run.base_sha))),
        ("head".into(), json!(head)),
        (
            "task".into(),
            json!(task.as_ref().map(|task| json!({
                "id": task.id,
                "title": task.title,
                "body": task.body,
                "parent": task.parent,
                "depends_on": task.depends_on,
            }))),
        ),
        ("run".into(), json!(run.as_ref().map(|run| &run.id))),
    ]);
    let context = extension::CommandContext {
        root,
        cwd,
        project,
        extension: entry.extension,
        env: project_env,
        hold: None,
    };
    let bytes = extension::run(
        &entry.definition.run,
        request,
        &context,
        Duration::from_secs(60),
        1024 * 1024,
    )
    .map_err(|error| anyhow::anyhow!("tool {name:?}: {}", format!("{error:#}").escape_debug()))?;
    let recorded = match (entry.definition.record, engine.as_ref(), run.as_ref()) {
        (true, Some(engine), Some(run)) => {
            engine.record_tool_output(&run.task, &run.id, name, &bytes)
        }
        _ => Ok(()),
    };
    // A tool's stdout is its interface, even under --json. Do not annotate it.
    out.stdout.write_all(&bytes).context("write tool stdout")?;
    recorded.with_context(|| format!("record output of tool {name:?}"))
}

// Run context belongs only to the workspace that launched the session.
fn launch_workspace_matches(root: &Path) -> bool {
    env::var_os("CORRAL_NEXT_ROOT").is_none_or(|value| {
        let inherited = Path::new(&value);
        inherited
            .canonicalize()
            .unwrap_or_else(|_| inherited.to_path_buf())
            == root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
    })
}

fn tool_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.is_empty())
}

fn tool_project<'a>(
    config: &'a Config,
    root: &Path,
    cwd: &Path,
    explicit: Option<&str>,
    task: Option<&Task>,
    same_root: bool,
) -> Result<Option<&'a str>> {
    if let (Some(task), Some(project)) = (task, explicit) {
        anyhow::ensure!(
            task.project.as_deref() == Some(project),
            "project {project:?} does not match the task's project"
        );
    }
    let requested = explicit
        .map(str::to_owned)
        .or_else(|| task.and_then(|task| task.project.clone()))
        .or_else(|| same_root.then(|| tool_env("CORRAL_PROJECT")).flatten());
    if let Some(name) = requested {
        return config
            .projects
            .get_key_value(&name)
            .map(|(name, _)| Some(name.as_str()))
            .with_context(|| format!("unknown project {name:?}"));
    }
    if task.is_some() || config.projects.is_empty() {
        return Ok(None);
    }
    if config.projects.len() == 1 {
        return Ok(config.projects.keys().next().map(String::as_str));
    }
    // In a workspace with nested projects, the deepest containing path wins.
    let mut candidates = config
        .projects
        .iter()
        .filter_map(|(name, project)| {
            let path = root.join(&project.path).canonicalize().ok()?;
            cwd.starts_with(&path)
                .then(|| (path.components().count(), name.as_str()))
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable();
    let mut candidates = candidates.into_iter().rev();
    if let Some((depth, project)) = candidates.next()
        && candidates
            .next()
            .is_none_or(|(other_depth, _)| other_depth != depth)
    {
        return Ok(Some(project));
    }
    Err(anyhow::anyhow!(
        "cannot determine the current project; use --project"
    ))
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
            owner,
            path,
            kind,
            title,
            replaces,
        } => {
            let kind = match kind {
                Some(kind) => kind,
                None if engine.actor == Actor::Owner => ArtifactKind::Reference,
                None => {
                    let allowed = args::artifact_kinds()
                        .map(ArtifactKind::as_str)
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(refuse(
                        format!("--kind is required; allowed values: {allowed}"),
                        vec![],
                    ));
                }
            };
            let artifact = engine.add_artifact_with_replacement(
                &owner,
                &path,
                kind,
                title.as_deref(),
                replaces.as_deref(),
            )?;
            out.applied(&Applied {
                task: engine.store.get_task(&owner)?,
                artifact: Some(artifact),
                seq: engine.store.latest_seq()?,
                ..Applied::default()
            })
        }
        ArtifactCommand::List { owner, all } => {
            let artifacts = engine.list_artifacts(owner.as_deref())?;
            out.value(&artifacts, |out, artifacts| {
                artifacts
                    .iter()
                    .filter(|artifact| all || artifact.replaced_by.is_none())
                    .try_for_each(|artifact| output::artifact_line(out, artifact))
            })
        }
        ArtifactCommand::Show { reference } if out.json => {
            let artifact = engine.require_artifact(&reference)?;
            out.value(&artifact, output::artifact_line)
        }
        ArtifactCommand::Show { reference } => out
            .stdout
            .write_all(&engine.show_artifact(&reference)?)
            .context("write artifact contents"),
    }
}
