//! `queue` command executors.
use anyhow::Result;
use corral_protocol::cmd::queue::QueueCommand;

use super::{Exec, Verb};

impl Verb for QueueCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        match self {
            Self::Add {
                id,
                profile,
                model,
                effort,
                base,
                headless,
            } => {
                let opts = crate::DispatchOpts {
                    plan: false,
                    profile,
                    model,
                    effort,
                    mode: if headless {
                        corral_core::RunMode::Headless
                    } else {
                        corral_core::RunMode::Interactive
                    },
                    base,
                    queue_if_blocked: true,
                    force_resources: false,
                };
                let task = crate::enqueue(&cx.ctx, &id, &opts)?;
                cx.out
                    .line(format_args!("{} Ready queued (enqueue only)", task.id));
                Ok(())
            }
            Self::Remove { id } => {
                let task = crate::dequeue(&cx.ctx, &id)?;
                cx.out.line(format_args!("{} Backlog (dequeued)", task.id));
                Ok(())
            }
            Self::Pause {} => {
                crate::dispatch_pause(&cx.ctx, true)?;
                cx.out.line(format_args!(
                    "coding admission paused only; `corral mode pause` pauses all automatic actions"
                ));
                Ok(())
            }
            Self::Resume {} => {
                crate::dispatch_pause(&cx.ctx, false)?;
                cx.out.line(format_args!(
                    "coding admission resumed only; operating pause and scoped authority are unchanged"
                ));
                Ok(())
            }
            Self::Admit { project, off } => {
                crate::admit_ready(&cx.ctx, &project, !off)?;
                cx.out.line(format_args!(
                    "{project}: Ready admission {}",
                    if off { "off" } else { "on" }
                ));
                Ok(())
            }
            Self::Other => Err(super::unknown_command("other")),
        }
    }

    fn touches_runtime(&self) -> bool {
        matches!(self, Self::Remove { .. })
    }
}
