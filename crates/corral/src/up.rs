//! One-command board startup, inside or outside the terminal engine.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corral_core::{Config, Store};
use corral_herdr::{EngineProblem, Herdr};
use corral_ops::Ctx;

use crate::doctor;

fn initialize(start: &Path, yes: bool) -> Result<()> {
    if Store::find_root(start).is_ok() {
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
    let mut child = herdr
        .command()
        .arg("server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("starting corral's terminal engine")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    // The server binds asynchronously. This bounded readiness check belongs only to startup;
    // doctor and every other operation are read-only probes and never start a server.
    loop {
        if herdr.available() {
            return Ok(());
        }
        if let Some(status) = child
            .try_wait()
            .context("checking terminal engine startup")?
        {
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
    let panes = ctx.herdr.pane_list()?;
    if let Some(board) = panes.iter().find(|pane| {
        pane.title.as_deref() == Some("corral board") && pane.cwd.as_deref() == Some(&root)
    }) {
        ctx.herdr.pane_focus(&board.pane_id)?;
    } else if ctx.herdr.workspace_list()?.is_empty() {
        // A new headless session starts empty. Its very first pane is the board.
        let created = ctx.herdr.workspace_create(&root, "corral board", true)?;
        let exe = std::env::current_exe().context("locating corral")?;
        let session_env = ctx.herdr.session().map_or(String::new(), |name| {
            format!("CORRAL_HERDR_SESSION={} ", quote(name))
        });
        let command = format!(
            "{session_env}{} --root {} board",
            quote(&exe.to_string_lossy()),
            quote(&root.to_string_lossy())
        );
        ctx.herdr.pane_run(&created.pane_id, &command)?;
        ctx.herdr
            .pane_report_metadata(&created.pane_id, "corral board", &[])?;
    } else {
        ctx.herdr.board_open(&root)?;
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
    Ok(())
}
