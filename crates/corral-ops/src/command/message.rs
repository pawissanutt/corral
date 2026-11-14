//! Filing and withdrawal act on canonical durable messages, never client-supplied replacement words.
use super::{Exec, Verb};
use anyhow::Result;
use corral_protocol::cmd::message::MessageCommand;

impl Verb for MessageCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        let device = cx.invoker.device()?;
        match self {
            Self::FileAsIdea { entry } => {
                let task = crate::agent::app_runtime::file_as_idea(&cx.ctx, device, &entry)?;
                cx.out.line(format_args!("{task}"));
            }
            Self::Withdraw { entry } => {
                crate::agent::app_runtime::withdraw(&cx.ctx, device, &entry)?;
                cx.out.line(format_args!("{entry} withdrawn"));
            }
            Self::Other => return Err(super::unknown_command("other")),
        }
        Ok(())
    }
}
