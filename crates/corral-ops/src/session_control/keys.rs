use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::Target;
use crate::Ctx;

const NAMED_KEYS: &[&str] = &[
    "enter",
    "esc",
    "tab",
    "backspace",
    "up",
    "down",
    "left",
    "right",
    "pageup",
    "pagedown",
    "home",
    "end",
    "insert",
    "delete",
    "space",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
];
const CTRL_KEYS: &[(&str, &str)] = &[
    ("ctrl-a", "ctrl+a"),
    ("ctrl-b", "ctrl+b"),
    ("ctrl-c", "ctrl+c"),
    ("ctrl-d", "ctrl+d"),
    ("ctrl-e", "ctrl+e"),
    ("ctrl-f", "ctrl+f"),
    ("ctrl-g", "ctrl+g"),
    ("ctrl-h", "ctrl+h"),
    ("ctrl-i", "ctrl+i"),
    ("ctrl-j", "ctrl+j"),
    ("ctrl-k", "ctrl+k"),
    ("ctrl-l", "ctrl+l"),
    ("ctrl-m", "ctrl+m"),
    ("ctrl-n", "ctrl+n"),
    ("ctrl-o", "ctrl+o"),
    ("ctrl-p", "ctrl+p"),
    ("ctrl-q", "ctrl+q"),
    ("ctrl-r", "ctrl+r"),
    ("ctrl-s", "ctrl+s"),
    ("ctrl-t", "ctrl+t"),
    ("ctrl-u", "ctrl+u"),
    ("ctrl-v", "ctrl+v"),
    ("ctrl-w", "ctrl+w"),
    ("ctrl-x", "ctrl+x"),
    ("ctrl-y", "ctrl+y"),
    ("ctrl-z", "ctrl+z"),
];

fn named_key(key: &str) -> Option<&'static str> {
    match key {
        "escape" => Some("esc"),
        "page-up" => Some("pageup"),
        "page-down" => Some("pagedown"),
        _ => NAMED_KEYS
            .iter()
            .copied()
            .find(|name| *name == key)
            .or_else(|| {
                CTRL_KEYS
                    .iter()
                    .find_map(|(alias, name)| (*alias == key || *name == key).then_some(*name))
            }),
    }
}

fn require_keys(target: &Target) -> Result<&str> {
    target.require_task_family()?;
    ensure!(!target.ended, "ended: session {} has ended", target.id);
    ensure!(
        !target.bridge,
        "has_bridge: session {} takes session send, not keys",
        target.id
    );
    let pane = target
        .pane
        .as_deref()
        .context("no_pane: session has no pane")?;
    ensure!(
        target.owned,
        "not_owned: session pane belongs to another herdr server"
    );
    ensure!(
        !target.adopted,
        "adopted_pane: corral never types into an owner's adopted pane"
    );
    Ok(pane)
}

pub(crate) fn keys(
    ctx: &Ctx,
    target: &Target,
    keys: &[String],
    text: Option<&str>,
) -> Result<Value> {
    require_keys(target)?;
    ensure!(
        keys.len() <= 32,
        "invalid_params: at most 32 keys may be sent"
    );
    ensure!(
        !keys.is_empty() || text.is_some(),
        "invalid_params: provide named keys or text"
    );
    let keys: Vec<_> = keys
        .iter()
        .map(|key| {
            named_key(key).with_context(|| format!("invalid_params: unsupported named key {key:?}"))
        })
        .collect::<Result<_>>()?;
    if let Some(text) = text {
        ensure!(
            text.len() <= 4 * 1024,
            "invalid_params: text exceeds 4096 bytes"
        );
        ensure!(
            !text.trim().is_empty(),
            "invalid_params: text must contain a non-whitespace character; send whitespace as named keys (space, tab, enter)"
        );
        ensure!(
            !text
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t'),
            "invalid_params: text contains a control character other than newline or tab"
        );
    }
    // Re-read ownership while lifecycle changes are fenced; never trust a pane selector
    // supplied by the client or continue after the public row points at a replacement.
    let _board = ctx
        .store
        .try_lock("dispatch")
        .context("claiming remote session keys lock")?
        .ok_or(corral_core::backend::ApplyError::Busy {
            retry_after: std::time::Duration::from_secs(1),
        })
        .context("busy: session lifecycle is being updated; retry remote keys")?;
    let current = super::resolve(ctx, &target.id)?;
    current.check_generation(target.generation)?;
    let pane = require_keys(&current)?;
    ensure!(
        current.pane == target.pane,
        "conflict: session pane changed before keys"
    );
    let text_bytes = text.map_or(0, str::len);
    ctx.store
        .log_event(
            current.task.as_deref(),
            "session_keys",
            json!({
                "id": current.id, "pane": pane, "keys": keys, "text_bytes": text_bytes,
            }),
        )
        .context("logging attributed remote session keys")?;
    if !keys.is_empty() {
        ctx.herdr
            .pane_send_keys(pane, &keys)
            .context("sending remote session keys")?;
    }
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        let delivery = ctx
            .herdr
            .pane_send_text(pane, text, false)
            .context("typing remote session text without submitting")?;
        ensure!(
            delivery.error.is_none(),
            "typing remote session text failed: {}",
            delivery
                .error
                .as_deref()
                .unwrap_or("unknown delivery error")
        );
    }
    Ok(json!({
        "id": current.id, "pane": pane, "keys": keys,
        "text_bytes": text_bytes, "text_submitted": false,
    }))
}
