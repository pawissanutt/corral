//! One-command board startup, inside or outside the terminal engine.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corral_core::{Config, Store};
use corral_herdr::{EngineProblem, ForegroundProcess, Herdr};
use corral_ops::Ctx;

use crate::doctor;

fn initialize(start: &Path, yes: bool) -> Result<()> {
    let root = Store::find_root(start).unwrap_or_else(|_| start.to_path_buf());
    let start = root.as_path();
    if start.join(".corral/corral.toml").is_file() {
        return Ok(());
    }
    let config = Config::default_for(start);
    println!("No corral workspace found. `corral init` will detect these repositories:");
    for (name, project) in &config.projects {
        println!("  {name}: {}", start.join(&project.path).display());
    }
    if config.projects.is_empty() {
        bail!("no git repositories found; run `corral up` in a repository or its parent directory");
    }
    let accepted = if yes {
        true
    } else if std::io::stdin().is_terminal() {
        print!("Initialize {} with `corral init`? [y/N] ", start.display());
        std::io::stdout().flush().context("showing init prompt")?;
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .context("reading init choice")?;
        matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    } else {
        false
    };
    if !accepted {
        bail!("run `corral init` here, or `corral up --yes` to accept detected repositories");
    }
    let store = Store::init(start)?;
    println!("initialized {}", store.dir().display());
    Ok(())
}

fn start_server(herdr: &Herdr) -> Result<()> {
    let child = herdr
        .command()
        .arg("server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("starting corral's terminal engine")?;
    wait_for_server(child, || herdr.available())
}

fn wait_for_server(mut child: Child, mut available: impl FnMut() -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    // The server binds asynchronously. This bounded readiness check belongs only to startup;
    // doctor and every other operation are read-only probes and never start a server.
    loop {
        if available() {
            return Ok(());
        }
        if let Some(status) = child
            .try_wait()
            .context("checking terminal engine startup")?
        {
            // Readiness may have changed after the failed probe but before exit.
            // Refresh it now that the launcher can no longer publish new state.
            if available() {
                return Ok(());
            }
            bail!("corral's terminal engine exited ({status}). Run `corral doctor` for fixes.");
        }
        if Instant::now() >= deadline {
            child
                .kill()
                .context("stopping an unresponsive new terminal engine")?;
            child
                .wait()
                .context("reaping the stopped terminal engine")?;
            return Err(EngineProblem::Down.into());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The shell command that runs the board for `root` in a pane, pinned to this engine session.
fn board_command(ctx: &Ctx, root: &Path) -> Result<String> {
    let exe = std::env::current_exe().context("locating corral")?;
    let session_env = ctx.herdr.session().map_or(String::new(), |name| {
        format!("CORRAL_HERDR_SESSION={} ", quote(name))
    });
    Ok(format!(
        "{session_env}{} --root {} board",
        quote(&exe.to_string_lossy()),
        quote(&root.to_string_lossy())
    ))
}

fn is_board_process(process: &ForegroundProcess) -> bool {
    let named_corral = process.argv.first().is_some_and(|exe| {
        Path::new(exe)
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("corral"))
    });
    named_corral && process.argv.iter().any(|arg| arg == "board")
}

/// Shows this workspace's board pane, restarting the board in it when closing the board left only its shell
/// behind (the first board runs inside a shell that outlives it). A pane busy with anything else is left
/// alone. False when no board pane can be shown.
fn show_existing(ctx: &Ctx, root: &Path) -> Result<bool> {
    let panes = ctx.herdr.pane_list()?;
    let boards = panes.iter().filter(|pane| {
        pane.title.as_deref() == Some("corral board") && pane.cwd.as_deref() == Some(root)
    });
    for pane in boards {
        let info = ctx
            .herdr
            .pane_process_info(&pane.pane_id)
            .with_context(|| format!("checking what board pane {} runs", pane.pane_id))?;
        let running = info.foreground_processes.iter().any(is_board_process);
        let idle_shell = info
            .foreground_processes
            .iter()
            .all(|process| process.pid == info.shell_pid);
        if !running && !idle_shell {
            continue;
        }
        if !running {
            ctx.herdr
                .pane_run(&pane.pane_id, &board_command(ctx, root)?)?;
        }
        ctx.herdr.pane_focus(&pane.pane_id)?;
        return Ok(true);
    }
    Ok(false)
}

/// The back-to-board key (`corral.board` action): return to the open board instead of starting a second one.
pub fn focus_board(ctx: &Ctx) -> Result<()> {
    let root = ctx
        .store
        .root()
        .canonicalize()
        .context("resolving workspace root")?;
    if !show_existing(ctx, &root)? {
        open_board(ctx, &root)?;
    }
    Ok(())
}

/// Starts the board in a `corral board` workspace of its own, in every session: the sidebar entry is the mouse
/// route back, and the titled single pane is what `show_existing` finds and focuses on later returns.
fn open_board(ctx: &Ctx, root: &Path) -> Result<()> {
    let created = ctx.herdr.workspace_create(root, "corral board", true)?;
    ctx.herdr
        .pane_run(&created.pane_id, &board_command(ctx, root)?)?;
    ctx.herdr
        .pane_report_metadata(&created.pane_id, "corral board", &[])
}

pub fn run(start: &Path, requested: Option<String>, yes: bool) -> Result<()> {
    initialize(start, yes)?;
    let store = Store::discover(start)?;
    let session = doctor::session(&store.config, requested);
    let report = doctor::inspect(start, session.clone(), true)?;
    report.print();
    if !report.startup_ready() {
        bail!("cannot open the board; use the fixes listed above");
    }
    let ctx = Ctx {
        store,
        herdr: Herdr::new(session),
    };
    let inside = std::env::var("HERDR_ENV").as_deref() == Ok("1");
    if !ctx.herdr.available() {
        if inside {
            return Err(EngineProblem::Down.into());
        }
        start_server(&ctx.herdr)?;
    }
    let root = ctx
        .store
        .root()
        .canonicalize()
        .context("resolving workspace root")?;
    if !show_existing(&ctx, &root)? {
        open_board(&ctx, &root)?;
    }
    if inside || !std::io::stdin().is_terminal() {
        println!("corral board is ready. Run `corral up` from a terminal to attach.");
        return Ok(());
    }
    let status = ctx
        .herdr
        .command()
        .stdin(Stdio::inherit())
        .status()
        .context("attaching corral's terminal engine")?;
    if !status.success() {
        bail!("could not attach the terminal engine. Run `corral doctor` for fixes.");
    }
    println!("Left corral. Agents keep running in the background; run `corral up` to come back.");
    Ok(())
}

#[cfg(test)]
#[path = "up_tests.rs"]
mod tests;
