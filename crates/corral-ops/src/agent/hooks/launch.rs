//! Launch and bootstrap seam: the pane command of a pi-native worker run.
//! Owner: T-252 child host-bridge (writes the bootstrap and renders `{corral} agent launch
//! {delivery_args}`).
use anyhow::Result;
use corral_core::inbox::Recipient;
use corral_core::{Run, Task};

use super::consulted;
use crate::Ctx;

pub fn worker_command(_ctx: &Ctx, task: &Task, run: &Run) -> Result<String> {
    consulted("launch");
    super::super::launch::command(&Recipient::Run {
        task: task.id.clone(),
        run: run.id.clone(),
    })
}
