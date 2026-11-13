//! Executes a command in this process, opening the store the way the CLI always has.
use std::path::Path;

use anyhow::Result;
use corral_core::Store;

use super::{Command, Invoker, OpenSpec, Outcome, Output};
use crate::Ctx;

/// Opens per `open_spec` (`Store::open_with` + apply, or `Store::open_unobserved` [+ apply]),
/// then `execute_in`. Open failures settle like executor failures, in the command's style.
/// An unknown command fails without opening the store.
pub fn run(root: &Path, invoker: &Invoker, command: Command) -> Outcome {
    let _span = corral_core::timing::span("command.local");
    if !command.is_known() {
        return super::unknown(&command, Output::default());
    }
    match open(root, invoker, super::open_spec(&command)) {
        Ok(store) => super::execute_in(Ctx::from_store(store), invoker, command),
        Err(error) => super::settle(&super::error_style(&command), Err(error), Output::default()),
    }
}

/// `attribute: false` still applies an owner action's attribution, as unobserved CLI opens do.
fn open(root: &Path, invoker: &Invoker, spec: OpenSpec) -> Result<Store> {
    let attribute = spec.attribute || invoker.owner_action.is_some();
    if spec.observe {
        return Store::open_with(root, |store| {
            if attribute {
                invoker.apply(store)?;
            }
            Ok(())
        });
    }
    let store = Store::open_unobserved(root)?;
    if attribute {
        invoker.apply(&store)?;
    }
    Ok(store)
}
