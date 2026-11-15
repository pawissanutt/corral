//! The inherited-pipe launch contract, including generation-bound summary configuration.
use anyhow::{Result, bail};
use serde_json::json;

use super::{Bootstrap, Ready, Secrets, capabilities_payload};

const MESSAGE_MAX_BYTES: usize = 32 * 1024;

/// The JSON the host reads from its inherited descriptor (`src/host/config.ts`).
pub fn launch_message(bootstrap: &Bootstrap, ready: &Ready, secrets: &Secrets) -> Result<Vec<u8>> {
    let message = json!({
        "version": 1, "store": bootstrap.store, "project": bootstrap.project,
        "role": bootstrap.role.as_str(), "task": bootstrap.task, "run": bootstrap.run,
        "review": bootstrap.review, "callback": bootstrap.callback,
        "assistant": bootstrap.assistant,
        "generation": bootstrap.generation,
        "secrets": {"host_cap": secrets.host_cap, "ctl_cap": secrets.ctl_cap,
                    "bind_nonce": bootstrap.bind_nonce},
        "pane": (!bootstrap.daemon_hosted).then_some(&bootstrap.pane),
        "daemon_hosted": bootstrap.daemon_hosted, "cwd": bootstrap.cwd, "root": bootstrap.root,
        "corral": bootstrap.corral, "assets": ready.assets,
        "runtime_dir": ready.generation_dir, "session_dir": bootstrap.session_dir,
        "resume_session": bootstrap.resume_session,
        "fresh_session": bootstrap.fresh_session, "resume_leaf": bootstrap.resume_leaf,
        "model": bootstrap.model, "provider": bootstrap.provider, "effort": bootstrap.effort,
        "aws": bootstrap.aws, "binding": bootstrap.binding,
        "quick_summary": bootstrap.quick_summary, "summary": bootstrap.summary,
        "prompt_file": bootstrap.prompt_file, "ui": bootstrap.ui.as_str(),
        "memory_file": crate::agent::hooks::memory::role(bootstrap.role)
            .map(|_| ready.generation_dir.join(crate::agent::hooks::memory::FILE)),
        "todo": bootstrap.todo,
        "capabilities": bootstrap.capabilities.as_ref().map(|snapshot|
            capabilities_payload(snapshot, bootstrap.project_dir.as_deref())),
    });
    let bytes = serde_json::to_vec(&message)?;
    if bytes.len() > MESSAGE_MAX_BYTES {
        bail!("the launch message exceeds {MESSAGE_MAX_BYTES} bytes");
    }
    Ok(bytes)
}
