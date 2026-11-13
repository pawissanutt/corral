//! One CLI invocation: the caller's entry, the store opened at most once, and routed commands.
use std::cell::OnceCell;
use std::path::PathBuf;

use anyhow::Result;
use corral_core::Store;
use corral_ops::Ctx;
use corral_ops::command::{Command, route};

use crate::entry::Entry;

pub(crate) struct Invocation<'a> {
    entry: &'a Entry,
    root: &'a Option<PathBuf>,
    ctx: OnceCell<Ctx>,
}

impl<'a> Invocation<'a> {
    pub(crate) fn new(entry: &'a Entry, root: &'a Option<PathBuf>) -> Self {
        Self {
            entry,
            root,
            ctx: OnceCell::new(),
        }
    }

    pub(crate) fn entry(&self) -> &'a Entry {
        self.entry
    }

    pub(crate) fn start(&self) -> Result<PathBuf> {
        crate::cmds::start_dir(self.root)
    }

    /// The store opened with the entry's attribution, once per process (`Entry::open`).
    pub(crate) fn ctx(&self) -> Result<&Ctx> {
        if let Some(ctx) = self.ctx.get() {
            return Ok(ctx);
        }
        let ctx = self.entry.open(&self.start()?)?;
        Ok(self.ctx.get_or_init(|| ctx))
    }

    /// Route, print stdout/stderr verbatim, Ok(()) on exit 0 else Err(Rendered).
    #[allow(
        dead_code,
        reason = "scaffold: the family units route their verbs through submit"
    )]
    pub(crate) fn submit(&self, command: Command) -> Result<()> {
        let root = Store::find_root(&self.start()?)?;
        // Keeps the process timing record without opening the store.
        corral_core::timing::register_store(&root.join(".corral"));
        let outcome = route::submit(&root, self.entry.invoker(), command);
        print!("{}", outcome.stdout);
        eprint!("{}", outcome.stderr);
        if outcome.exit == 0 {
            return Ok(());
        }
        Err(Rendered {
            exit: outcome.exit,
            failure: outcome.error.map(|error| error.message),
        }
        .into())
    }
}

/// Output already written; main.rs returns `exit` and passes `failure` to owner_action::finish.
#[derive(Debug)]
pub(crate) struct Rendered {
    pub exit: u8,
    pub failure: Option<String>,
}

impl std::fmt::Display for Rendered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.failure {
            Some(failure) => f.write_str(failure),
            None => write!(f, "command exited with status {}", self.exit),
        }
    }
}

impl std::error::Error for Rendered {}
