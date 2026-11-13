//! Prints the `corral pi` launcher as it first opens, as a text grid, for comparison with the
//! launch scene of `docs/mockup/corral-pi-build.html`. The rows are the mock's: Build with where it
//! writes, the Assistant and the PM, then the stopped sessions it lists under Resume; the option
//! block shows the mock's project and two providers with a login.
//! Usage: capture_launcher COLS ROWS
use anyhow::{Context, Result};
use corral_tui::launcher::{self, LauncherContext, LauncherEntry, Login, ProviderLogin, Section};
use ratatui::{Terminal, backend::TestBackend};
use unicode_width::UnicodeWidthStr;

/// The directory the mock's header names.
const PLACE: &str = "~/miti-workspace/mitiplay";

/// The mock's option block: project `mitiplay`, `openai-codex ✓   anthropic ✓`.
fn context() -> LauncherContext {
    let present = |id: &str| ProviderLogin {
        id: id.into(),
        login: Login::Present,
    };
    LauncherContext {
        project: Some("mitiplay".into()),
        providers: vec![present("openai-codex"), present("anthropic")],
    }
}

fn start(id: &str, key: char, name: &str, detail: &str) -> LauncherEntry {
    LauncherEntry {
        id: id.into(),
        section: Section::Start,
        key: Some(key),
        name: name.into(),
        detail: detail.into(),
        options: Vec::new(),
    }
}

fn resume(id: &str, name: &str, detail: &str) -> LauncherEntry {
    LauncherEntry {
        id: id.into(),
        section: Section::Resume,
        key: None,
        name: name.into(),
        detail: detail.into(),
        options: Vec::new(),
    }
}

fn entries() -> Vec<LauncherEntry> {
    let mut build = start(
        "build",
        'b',
        "Build",
        "untethered · parallel agents · nothing reaches the board until you ship",
    );
    build.options = vec![
        "new worktree  .worktrees/build-<name>".into(),
        "here  the current checkout and branch".into(),
    ];
    vec![
        build,
        start(
            "assistant",
            'a',
            "Assistant",
            "board-aware · scratch worktrees · owner actions on your instruction",
        ),
        start(
            "pm",
            'p',
            "PM",
            "workspace coordinator · continues the previous conversation",
        ),
        resume(
            "B-checkout-copy",
            "build",
            "checkout-copy · mitiplay · 4 agents · 12 files changed · 23m ago",
        ),
        resume(
            "A-31",
            "A-31",
            "Cache spike · corral · scratch spike-cache · 2h ago",
        ),
        resume(
            "A-29",
            "A-29",
            "Arrival layout notes · mitiplay · suspended yesterday",
        ),
    ]
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let cols: u16 = args
        .get(1)
        .context("missing COLS")?
        .parse()
        .context("parsing COLS")?;
    let rows: u16 = args
        .get(2)
        .context("missing ROWS")?
        .parse()
        .context("parsing ROWS")?;
    let list = entries();
    let context = context();
    let mut terminal = Terminal::new(TestBackend::new(cols, rows))?;
    terminal.draw(|f| launcher::preview(f, &list, PLACE, &context))?;
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..buffer.area.height {
        let mut skip = 0;
        for x in 0..buffer.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let symbol = buffer[(x, y)].symbol();
            skip = symbol.width().saturating_sub(1);
            text.push_str(symbol);
        }
        text.push('\n');
    }
    print!("{text}");
    Ok(())
}
